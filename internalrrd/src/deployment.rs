use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use protocol::{DeploymentAccepted, DeploymentRejected, Outcome, RejectionCode, SignedDeployment};
use tokio::sync::Mutex;

use crate::config::ApplicationConfig;
use crate::ports::{PortAllocator, PortError};
use crate::process::ProcessSpec;
use crate::proxy::Proxy;
use crate::release::ReleaseManager;
use crate::supervisor::{InstanceId, Supervisor, SupervisorError};
use crate::template::{self, TemplateError, TemplateValues};
use crate::trust::TrustStore;

/// Coordinates releases, application instances, and the L4 proxy.
pub struct Deployment {
    application: ApplicationConfig,
    trust: TrustStore,
    releases: ReleaseManager,
    proxy: Option<Arc<Proxy>>,
    ports: Mutex<PortAllocator>,
    supervisor: Arc<Mutex<Supervisor>>,
    state: Mutex<DeploymentState>,
    deploy_lock: Mutex<()>,
}

#[derive(Debug, Default)]
struct DeploymentState {
    version: Option<String>,
    release: Option<PathBuf>,
    issued_at_unix: u64,
    instances: Vec<ActiveInstance>,
}

#[derive(Debug, Clone, Copy)]
struct ActiveInstance {
    id: InstanceId,
    port: u16,
}

impl Deployment {
    pub fn new(
        application: ApplicationConfig,
        trust: TrustStore,
        releases: ReleaseManager,
        proxy: Option<Arc<Proxy>>,
        ports: PortAllocator,
        supervisor: Arc<Mutex<Supervisor>>,
    ) -> Self {
        Self {
            application,
            trust,
            releases,
            proxy,
            ports: Mutex::new(ports),
            supervisor,
            state: Mutex::new(DeploymentState::default()),
            deploy_lock: Mutex::new(()),
        }
    }

    /// Start instances from the current release, if one is available.
    ///
    /// When the command requires `{release_dir}` + no release is installed, no instances are started.
    /// Server waits for first deployment in this case
    pub async fn start_initial(&self) -> Result<(), DeploymentError> {
        let release = self.releases.current().await;
        let command_template = self.application.command.to_string_lossy();
        let requires_release = command_template.contains("{release_dir}");

        if requires_release && release.is_none() {
            tracing::info!("no release is installed, waiting for a deployment");
            return Ok(());
        }

        let metadata = match &release {
            Some(directory) => self.releases.metadata_of(directory).await,
            None => None,
        };

        let mut state = self.state.lock().await;
        let mut instances = Vec::new();
        let mut ports = Vec::new();

        for _ in 0..self.application.instances {
            let allocated = {
                let mut allocator = self.ports.lock().await;
                allocator.allocate().await
            };
            let port = match allocated {
                Ok(port) => port,
                Err(error) => {
                    self.rollback(&mut instances, &mut ports).await;
                    return Err(error.into());
                }
            };
            ports.push(port);

            let spec = match self.spec_for(release.as_deref(), port) {
                Ok(spec) => spec,
                Err(error) => {
                    self.rollback(&mut instances, &mut ports).await;
                    return Err(error);
                }
            };

            let started = {
                let mut supervisor = self.supervisor.lock().await;
                supervisor.start_with(spec)
            };
            match started {
                Ok(id) => instances.push(ActiveInstance { id, port }),
                Err(error) => {
                    self.rollback(&mut instances, &mut ports).await;
                    return Err(error.into());
                }
            }
        }

        if let Some(proxy) = &self.proxy {
            proxy.switch(backend_addresses(&instances));
        }
        state.instances = instances;
        state.release = release;
        if let Some(metadata) = metadata {
            state.version = Some(metadata.version);
            state.issued_at_unix = metadata.issued_at_unix;
        }
        Ok(())
    }

    /// Verify, install, start, and switch to a signed deployment.
    pub async fn deploy(&self, signed: SignedDeployment) -> Outcome {
        match self.try_deploy(signed).await {
            Ok(version) => Outcome::DeploymentAccepted(DeploymentAccepted { version }),
            Err(rejection) => Outcome::DeploymentRejected(rejection),
        }
    }

    async fn try_deploy(&self, signed: SignedDeployment) -> Result<String, DeploymentRejected> {
        self.trust
            .verify(&signed)
            .map_err(|error| reject(RejectionCode::INVALID_SIGNATURE, error))?;

        let deployment = signed.deployment;
        if deployment.application != self.application.name {
            return Err(reject(
                RejectionCode::INVALID_SIGNATURE,
                format!(
                    "deployment targets application '{}', expected '{}'",
                    deployment.application, self.application.name
                ),
            ));
        }
        if deployment.version.trim().is_empty() {
            return Err(reject(
                RejectionCode::INVALID_SIGNATURE,
                "deployment version must not be empty",
            ));
        }

        let _deploying = self.deploy_lock.lock().await;

        {
            let state = self.state.lock().await;
            if state.version.as_deref() == Some(deployment.version.as_str()) {
                return Err(reject(
                    RejectionCode::ALREADY_DEPLOYED,
                    format!("version {} is already deployed", deployment.version),
                ));
            }
            if state.issued_at_unix >= deployment.issued_at_unix {
                return Err(reject(
                    RejectionCode::OUTDATED_RELEASE,
                    format!(
                        "deployment issued at {} is not newer than the active deployment",
                        deployment.issued_at_unix
                    ),
                ));
            }
        }

        let release = self
            .releases
            .install(&deployment)
            .await
            .map_err(|error| reject(RejectionCode::ARTIFACT_UNAVAILABLE, error))?;

        let mut new_instances = Vec::new();
        let mut new_ports = Vec::new();

        for _ in 0..self.application.instances {
            let allocated = {
                let mut allocator = self.ports.lock().await;
                allocator.allocate().await
            };
            let port = match allocated {
                Ok(port) => port,
                Err(error) => {
                    self.rollback(&mut new_instances, &mut new_ports).await;
                    return Err(reject(RejectionCode::INTERNAL, error));
                }
            };
            new_ports.push(port);

            let spec = match self.spec_for(Some(&release.directory), port) {
                Ok(spec) => spec,
                Err(error) => {
                    self.rollback(&mut new_instances, &mut new_ports).await;
                    return Err(reject(RejectionCode::INTERNAL, error));
                }
            };

            let started = {
                let mut supervisor = self.supervisor.lock().await;
                supervisor.start_with(spec)
            };
            match started {
                Ok(id) => new_instances.push(ActiveInstance { id, port }),
                Err(error) => {
                    self.rollback(&mut new_instances, &mut new_ports).await;
                    return Err(reject(RejectionCode::INTERNAL, error));
                }
            }
        }

        if let Err(error) = self.wait_until_healthy(&new_instances).await {
            self.rollback(&mut new_instances, &mut new_ports).await;
            return Err(reject(RejectionCode::HEALTH_CHECK_FAILED, error));
        }

        if let Some(proxy) = &self.proxy {
            proxy.switch(backend_addresses(&new_instances));
        }

        let old_instances = {
            let mut state = self.state.lock().await;
            let old = std::mem::replace(&mut state.instances, new_instances);
            state.version = Some(deployment.version.clone());
            state.release = Some(release.directory.clone());
            state.issued_at_unix = deployment.issued_at_unix;
            old
        };

        if let Err(error) = self.releases.set_current(&release.directory).await {
            tracing::warn!(error = %error, "failed to update the current release link");
        }

        self.stop_instances(old_instances).await;

        tracing::info!(
            version = %deployment.version,
            release = %release.directory.display(),
            "deployment is serving"
        );
        Ok(deployment.version)
    }

    fn spec_for(&self, release: Option<&Path>, port: u16) -> Result<ProcessSpec, DeploymentError> {
        let values = TemplateValues {
            port,
            release_dir: release,
        };

        let command = template::expand(&self.application.command.to_string_lossy(), &values)?;
        if command.trim().is_empty() {
            return Err(DeploymentError::EmptyCommand);
        }

        let args = self
            .application
            .args
            .iter()
            .map(|argument| template::expand(argument, &values))
            .collect::<Result<Vec<_>, _>>()?;

        let environment = self
            .application
            .environment
            .iter()
            .map(|(name, value)| Ok((name.clone(), template::expand(value, &values)?)))
            .collect::<Result<BTreeMap<_, _>, TemplateError>>()?;

        let working_directory = match &self.application.working_directory {
            Some(directory) => Some(PathBuf::from(template::expand(
                &directory.to_string_lossy(),
                &values,
            )?)),
            None => release.map(Path::to_path_buf),
        };

        Ok(ProcessSpec {
            command: PathBuf::from(command),
            args,
            working_directory,
            environment,
            shutdown_timeout: self.application.shutdown_timeout(),
        })
    }

    async fn wait_until_healthy(&self, instances: &[ActiveInstance]) -> Result<(), String> {
        tokio::time::sleep(self.application.startup_grace()).await;

        let mut supervisor = self.supervisor.lock().await;
        supervisor.poll().map_err(|error| error.to_string())?;

        let exited = instances
            .iter()
            .filter(|instance| !supervisor.is_running(instance.id))
            .count();
        if exited > 0 {
            return Err(format!(
                "{exited} new instance(s) exited during the startup grace period"
            ));
        }

        Ok(())
    }

    async fn rollback(&self, instances: &mut Vec<ActiveInstance>, ports: &mut Vec<u16>) {
        {
            let mut supervisor = self.supervisor.lock().await;
            for instance in instances.drain(..) {
                if let Err(error) = supervisor.stop(instance.id).await {
                    tracing::warn!(
                        instance = %instance.id,
                        error = %error,
                        "failed to stop a rolled back instance"
                    );
                }
            }
        }

        let mut allocator = self.ports.lock().await;
        for port in ports.drain(..) {
            allocator.release(port);
        }
    }

    async fn stop_instances(&self, instances: Vec<ActiveInstance>) {
        {
            let mut supervisor = self.supervisor.lock().await;
            for instance in &instances {
                if let Err(error) = supervisor.stop(instance.id).await {
                    tracing::warn!(
                        instance = %instance.id,
                        error = %error,
                        "failed to stop an old instance"
                    );
                }
            }
        }

        let mut allocator = self.ports.lock().await;
        for instance in instances {
            allocator.release(instance.port);
        }
    }

    pub async fn current_version(&self) -> Option<String> {
        self.state.lock().await.version.clone()
    }

    pub async fn current_release(&self) -> Option<PathBuf> {
        self.state.lock().await.release.clone()
    }

    pub fn configured_working_directory(&self) -> Option<&Path> {
        self.application.working_directory.as_deref()
    }
}

fn backend_addresses(instances: &[ActiveInstance]) -> Vec<SocketAddr> {
    instances
        .iter()
        .map(|instance| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), instance.port))
        .collect()
}

fn reject(code: RejectionCode, error: impl std::fmt::Display) -> DeploymentRejected {
    DeploymentRejected {
        code,
        message: error.to_string(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeploymentError {
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error(transparent)]
    Port(#[from] PortError),
    #[error(transparent)]
    Supervisor(#[from] SupervisorError),
    #[error("application command template expanded to an empty command")]
    EmptyCommand,
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use protocol::{
        Artifact, Deployment as ReleaseDeployment, Outcome, RejectionCode, SignedDeployment,
        SigningKey,
    };
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::config::TrustConfig;

    const KEY_ID: &str = "release-2026";

    fn create_artifact(directory: &Path) -> Result<(PathBuf, [u8; 32], u64), Box<dyn Error>> {
        let archive_path = directory.join("example.tar.gz");
        let file = std::fs::File::create(&archive_path)?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);

        let contents = b"#!/bin/sh\nexec sleep 30\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, "run.sh", &contents[..])?;
        builder.finish()?;

        let encoder = builder.into_inner()?;
        encoder.finish()?;

        let bytes = std::fs::read(&archive_path)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        Ok((archive_path, digest, bytes.len() as u64))
    }

    fn application(directory: &Path) -> ApplicationConfig {
        ApplicationConfig {
            name: "example".to_string(),
            command: PathBuf::from("{release_dir}/run.sh"),
            args: Vec::new(),
            working_directory: None,
            environment: BTreeMap::new(),
            shutdown_timeout_secs: 1,
            instances: 2,
            startup_grace_secs: 0,
            release_directory: directory.join("releases"),
            max_artifact_bytes: 1024 * 1024,
        }
    }

    fn trust(signing_key: &SigningKey) -> TrustStore {
        let config = TrustConfig {
            keys: vec![crate::config::TrustedKeyConfig {
                id: KEY_ID.to_string(),
                algorithm: 1,
                public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            }],
        };
        TrustStore::from_config(&config).expect("trust store")
    }

    fn signed(
        signing_key: &SigningKey,
        archive: &Path,
        digest: [u8; 32],
        size: u64,
        version: &str,
        issued_at_unix: u64,
    ) -> SignedDeployment {
        SignedDeployment::sign(
            ReleaseDeployment {
                application: "example".to_string(),
                version: version.to_string(),
                artifact: Artifact {
                    url: format!("file://{}", archive.display()),
                    sha256: digest,
                    size_bytes: size,
                },
                issued_at_unix,
            },
            KEY_ID,
            signing_key,
        )
        .expect("signed deployment")
    }

    #[tokio::test]
    async fn deploys_and_replaces_instances() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let (archive, digest, size) = create_artifact(temporary.path())?;
        let signing_key = SigningKey::from_bytes(&[7; 32]);

        let supervisor = Arc::new(Mutex::new(Supervisor::new()));
        let deployment = Deployment::new(
            application(temporary.path()),
            trust(&signing_key),
            ReleaseManager::new(temporary.path().join("releases"), 1024 * 1024)?,
            None,
            PortAllocator::new(None, []),
            supervisor.clone(),
        );

        deployment.start_initial().await?;
        assert!(supervisor.lock().await.running().is_empty());

        let first = deployment
            .deploy(signed(
                &signing_key,
                &archive,
                digest,
                size,
                "1.2.3",
                1_700_000_000,
            ))
            .await;
        match first {
            Outcome::DeploymentAccepted(accepted) => assert_eq!(accepted.version, "1.2.3"),
            other => panic!("unexpected deployment outcome: {other:?}"),
        }
        assert_eq!(deployment.current_version().await.as_deref(), Some("1.2.3"));
        assert_eq!(supervisor.lock().await.running().len(), 2);

        let duplicate = deployment
            .deploy(signed(
                &signing_key,
                &archive,
                digest,
                size,
                "1.2.3",
                1_700_000_100,
            ))
            .await;
        assert!(matches!(
            duplicate,
            Outcome::DeploymentRejected(DeploymentRejected {
                code: RejectionCode::ALREADY_DEPLOYED,
                ..
            })
        ));

        let second = deployment
            .deploy(signed(
                &signing_key,
                &archive,
                digest,
                size,
                "1.2.4",
                1_700_000_200,
            ))
            .await;
        match second {
            Outcome::DeploymentAccepted(accepted) => assert_eq!(accepted.version, "1.2.4"),
            other => panic!("unexpected deployment outcome: {other:?}"),
        }
        assert_eq!(deployment.current_version().await.as_deref(), Some("1.2.4"));
        assert_eq!(supervisor.lock().await.running().len(), 2);

        supervisor.lock().await.stop_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_deployments_from_untrusted_keys() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let (archive, digest, size) = create_artifact(temporary.path())?;
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let other_key = SigningKey::from_bytes(&[9; 32]);

        let supervisor = Arc::new(Mutex::new(Supervisor::new()));
        let deployment = Deployment::new(
            application(temporary.path()),
            trust(&signing_key),
            ReleaseManager::new(temporary.path().join("releases"), 1024 * 1024)?,
            None,
            PortAllocator::new(None, []),
            supervisor,
        );

        let outcome = deployment
            .deploy(signed(
                &other_key,
                &archive,
                digest,
                size,
                "1.2.3",
                1_700_000_000,
            ))
            .await;

        assert!(matches!(
            outcome,
            Outcome::DeploymentRejected(DeploymentRejected {
                code: RejectionCode::INVALID_SIGNATURE,
                ..
            })
        ));
        Ok(())
    }
}
