use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::BytesMut;
use futures_util::{SinkExt, StreamExt};
use protocol::{
    Capability, CodecError, Command, DeploymentRejected, HelloRequest, HelloResponse,
    InstanceStatus, MetricsRequest, Outcome, ProtocolError, ProtocolErrorCode, ProtocolVersion,
    RejectionCode, Request, RequestCodec, Response, ResponseCodec, SignedDeployment, StatusReport,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_util::codec::{Decoder, Encoder, Framed};

use crate::deployment::Deployment;
use crate::metrics;
use crate::supervisor::Supervisor;

/// Shared state used by management connections.
#[derive(Clone)]
pub struct ManagementContext {
    application: Arc<ApplicationInfo>,
    supervisor: Arc<Mutex<Supervisor>>,
    deployment: Option<Arc<Deployment>>,
}

struct ApplicationInfo {
    name: String,
    working_directory: Option<PathBuf>,
}

impl ManagementContext {
    pub fn new(
        application_name: impl Into<String>,
        working_directory: Option<PathBuf>,
        supervisor: Arc<Mutex<Supervisor>>,
    ) -> Self {
        Self {
            application: Arc::new(ApplicationInfo {
                name: application_name.into(),
                working_directory,
            }),
            supervisor,
            deployment: None,
        }
    }

    pub fn with_deployment(mut self, deployment: Arc<Deployment>) -> Self {
        self.deployment = Some(deployment);
        self
    }

    fn application_name(&self) -> &str {
        &self.application.name
    }

    fn configured_working_directory(&self) -> Option<&Path> {
        self.application.working_directory.as_deref()
    }

    async fn current_version(&self) -> Option<String> {
        match &self.deployment {
            Some(deployment) => deployment.current_version().await,
            None => None,
        }
    }

    async fn active_working_directory(&self) -> Option<PathBuf> {
        if let Some(deployment) = &self.deployment {
            if let Some(release) = deployment.current_release().await {
                return Some(release);
            }
            if let Some(directory) = deployment.configured_working_directory() {
                return Some(directory.to_path_buf());
            }
        }
        self.configured_working_directory().map(Path::to_path_buf)
    }
}

/// Accept management connections until the listener fails.
pub async fn serve(listener: TcpListener, context: ManagementContext) -> io::Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let context = context.clone();

        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, context).await {
                tracing::warn!(%peer, error = %error, "management connection failed");
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    context: ManagementContext,
) -> Result<(), ConnectionError> {
    let peer = stream.peer_addr()?;
    tracing::debug!(%peer, "management connection opened");
    let mut framed = Framed::new(stream, ServerCodec::default());

    while let Some(request) = framed.next().await {
        let request = request?;
        let response = handle_request(request, &context).await;
        framed.send(response).await?;
    }

    tracing::debug!(%peer, "management connection closed");
    Ok(())
}

async fn handle_request(request: Request, context: &ManagementContext) -> Response {
    let version = request.version;
    let request_id = request.request_id;

    let outcome = if version.is_compatible(ProtocolVersion::CURRENT) {
        match request.body {
            Command::Hello(hello) => handle_hello(hello, context).await,
            Command::Status => handle_status(context).await,
            Command::Metrics(metrics_request) => handle_metrics(metrics_request, context).await,
            Command::Deploy(deployment) => handle_deploy(deployment, context).await,
        }
    } else {
        Outcome::Error(ProtocolError {
            code: ProtocolErrorCode::UNSUPPORTED_VERSION,
            message: format!("unsupported protocol version {version}"),
        })
    };

    Response::new(version, request_id, outcome)
}

async fn handle_hello(_request: HelloRequest, context: &ManagementContext) -> Outcome {
    Outcome::Hello(HelloResponse {
        application: context.application_name().to_string(),
        capabilities: vec![
            Capability::SIGNED_DEPLOYMENTS,
            Capability::STATUS_REPORTING,
            Capability::METRICS_REPORTING,
        ],
        deployed_version: context.current_version().await,
    })
}

async fn handle_status(context: &ManagementContext) -> Outcome {
    let instances = match refresh_instances(context).await {
        Ok(instances) => instances,
        Err(error) => return protocol_error(error),
    };
    let deployed_version = context.current_version().await;

    Outcome::Status(StatusReport {
        application: context.application_name().to_string(),
        deployed_version,
        instances: instances
            .into_iter()
            .map(|instance| InstanceStatus {
                id: instance.id.get(),
                pid: instance.pid,
                healthy: true,
                uptime_secs: instance.uptime.as_secs(),
            })
            .collect(),
    })
}

async fn handle_metrics(request: MetricsRequest, context: &ManagementContext) -> Outcome {
    let instances = match refresh_instances(context).await {
        Ok(instances) => instances,
        Err(error) => return protocol_error(error),
    };

    let application_name = context.application_name().to_string();
    let working_directory = context.active_working_directory().await;
    let deployed_version = context.current_version().await;
    let instance = request.instance;
    let result = tokio::task::spawn_blocking(move || {
        metrics::collect(
            &application_name,
            deployed_version,
            working_directory.as_deref(),
            &instances,
            instance,
        )
    })
    .await;

    match result {
        Ok(report) => Outcome::Metrics(Box::new(report)),
        Err(error) => protocol_error(format!("metrics collection failed: {error}")),
    }
}

async fn handle_deploy(signed: SignedDeployment, context: &ManagementContext) -> Outcome {
    tracing::info!(
        application = %signed.deployment.application,
        version = %signed.deployment.version,
        "received a deployment"
    );

    match &context.deployment {
        Some(deployment) => deployment.deploy(signed).await,
        None => Outcome::DeploymentRejected(DeploymentRejected {
            code: RejectionCode::INTERNAL,
            message: "deployment handling is not configured".to_string(),
        }),
    }
}

async fn refresh_instances(
    context: &ManagementContext,
) -> Result<Vec<crate::supervisor::InstanceInfo>, String> {
    let mut supervisor = context.supervisor.lock().await;
    supervisor.poll().map_err(|error| error.to_string())?;
    Ok(supervisor.running())
}

fn protocol_error(message: String) -> Outcome {
    Outcome::Error(ProtocolError {
        code: ProtocolErrorCode::INTERNAL,
        message,
    })
}

#[derive(Debug, thiserror::Error)]
enum ConnectionError {
    #[error("management connection I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("management protocol error: {0}")]
    Codec(#[from] CodecError),
}

#[derive(Default)]
struct ServerCodec {
    requests: RequestCodec,
    responses: ResponseCodec,
}

impl Decoder for ServerCodec {
    type Item = Request;
    type Error = CodecError;

    fn decode(&mut self, source: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.requests.decode(source)
    }
}

impl Encoder<Response> for ServerCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Response, destination: &mut BytesMut) -> Result<(), Self::Error> {
        self.responses.encode(item, destination)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use bytes::BytesMut;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use futures_util::{SinkExt, StreamExt};
    use protocol::{
        Artifact, Capability, CodecError, Command, Deployment, DeploymentSignature, HelloRequest,
        MetricsRequest, Outcome, ProtocolErrorCode, ProtocolVersion, RejectionCode, Request,
        RequestCodec, RequestId, Response, ResponseCodec, SignatureAlgorithm, SignedDeployment,
        SigningKey,
    };
    use sha2::{Digest, Sha256};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Mutex;
    use tokio_util::codec::{Decoder, Encoder, Framed};

    use super::{ManagementContext, serve};
    use crate::config::{ApplicationConfig, TrustConfig, TrustedKeyConfig};
    use crate::deployment::Deployment as DeploymentManager;
    use crate::ports::PortAllocator;
    use crate::process::ProcessSpec;
    use crate::release::ReleaseManager;
    use crate::supervisor::Supervisor;
    use crate::trust::TrustStore;

    #[derive(Default)]
    struct ClientCodec {
        requests: RequestCodec,
        responses: ResponseCodec,
    }

    impl Encoder<Request> for ClientCodec {
        type Error = CodecError;

        fn encode(&mut self, item: Request, destination: &mut BytesMut) -> Result<(), Self::Error> {
            self.requests.encode(item, destination)
        }
    }

    impl Decoder for ClientCodec {
        type Item = Response;
        type Error = CodecError;

        fn decode(&mut self, source: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
            self.responses.decode(source)
        }
    }

    fn sleeping_spec() -> ProcessSpec {
        ProcessSpec {
            command: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "sleep 30".to_string()],
            working_directory: None,
            environment: BTreeMap::new(),
            shutdown_timeout: Duration::from_secs(5),
        }
    }

    fn signed_deployment() -> SignedDeployment {
        SignedDeployment {
            deployment: Deployment {
                application: "example".to_string(),
                version: "1.2.3".to_string(),
                artifact: Artifact {
                    url: "https://artifacts.example/example-1.2.3".to_string(),
                    sha256: [0xab; 32],
                    size_bytes: 4096,
                },
                issued_at_unix: 1_700_000_000,
            },
            signature: DeploymentSignature {
                algorithm: SignatureAlgorithm::ED25519,
                key_id: "release-2026".to_string(),
                bytes: vec![0; 64],
            },
        }
    }

    async fn send(
        client: &mut Framed<TcpStream, ClientCodec>,
        request: Request,
    ) -> Result<Response, Box<dyn Error>> {
        client.send(request).await?;
        let response = client.next().await.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "management server closed the connection",
            )
        })??;
        Ok(response)
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn serves_management_requests() -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;

        let supervisor = Arc::new(Mutex::new(Supervisor::new()));
        let instance = supervisor.lock().await.start_with(sleeping_spec())?;
        let context = ManagementContext::new("example", None, supervisor.clone());
        let server = tokio::spawn(serve(listener, context));

        let stream = TcpStream::connect(address).await?;
        let mut client = Framed::new(stream, ClientCodec::default());

        let hello = send(
            &mut client,
            Request::current(
                RequestId::new(1),
                Command::Hello(HelloRequest {
                    capabilities: Vec::new(),
                }),
            ),
        )
        .await?;
        match hello.body {
            Outcome::Hello(response) => {
                assert_eq!(response.application, "example");
                assert!(
                    response
                        .capabilities
                        .contains(&Capability::SIGNED_DEPLOYMENTS)
                );
                assert!(
                    response
                        .capabilities
                        .contains(&Capability::METRICS_REPORTING)
                );
            }
            _ => panic!("unexpected hello response"),
        }

        let status = send(
            &mut client,
            Request::current(RequestId::new(2), Command::Status),
        )
        .await?;
        match status.body {
            Outcome::Status(response) => {
                assert_eq!(response.instances.len(), 1);
                assert_eq!(response.instances[0].id, instance.get());
            }
            _ => panic!("unexpected status response"),
        }

        let metrics = send(
            &mut client,
            Request::current(
                RequestId::new(3),
                Command::Metrics(MetricsRequest {
                    instance: Some(instance.get()),
                }),
            ),
        )
        .await?;
        match metrics.body {
            Outcome::Metrics(response) => {
                assert!(response.internalrrd.resources.memory_used_bytes > 0);
                let Some(application) = response.application else {
                    panic!("application metrics were not reported");
                };
                assert_eq!(application.instances.len(), 1);
                assert_eq!(application.instances[0].id, instance.get());
            }
            _ => panic!("unexpected metrics response"),
        }

        let deployment = send(
            &mut client,
            Request::current(RequestId::new(4), Command::Deploy(signed_deployment())),
        )
        .await?;
        match deployment.body {
            Outcome::DeploymentRejected(response) => {
                assert_eq!(response.code, RejectionCode::INTERNAL);
            }
            _ => panic!("unexpected deployment response"),
        }

        let incompatible = send(
            &mut client,
            Request::new(
                ProtocolVersion::new(2, 0),
                RequestId::new(5),
                Command::Status,
            ),
        )
        .await?;
        match incompatible.body {
            Outcome::Error(response) => {
                assert_eq!(response.code, ProtocolErrorCode::UNSUPPORTED_VERSION);
            }
            _ => panic!("unexpected incompatible version response"),
        }

        server.abort();
        supervisor.lock().await.stop_all().await?;
        Ok(())
    }

    fn create_release_artifact(
        directory: &Path,
    ) -> Result<(PathBuf, [u8; 32], u64), Box<dyn Error>> {
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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn deploys_through_the_management_protocol() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let (archive, digest, size) = create_release_artifact(temporary.path())?;
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let trust = TrustStore::from_config(&TrustConfig {
            keys: vec![TrustedKeyConfig {
                id: "release-2026".to_string(),
                algorithm: 1,
                public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            }],
        })?;

        let supervisor = Arc::new(Mutex::new(Supervisor::new()));
        let application = ApplicationConfig {
            name: "example".to_string(),
            command: PathBuf::from("{release_dir}/run.sh"),
            args: Vec::new(),
            working_directory: None,
            environment: BTreeMap::new(),
            shutdown_timeout_secs: 1,
            instances: 1,
            startup_grace_secs: 0,
            release_directory: temporary.path().join("releases"),
            max_artifact_bytes: 1024 * 1024,
        };
        let deployment = Arc::new(DeploymentManager::new(
            application,
            trust,
            ReleaseManager::new(temporary.path().join("releases"), 1024 * 1024)?,
            None,
            PortAllocator::new(None, []),
            supervisor.clone(),
        ));
        deployment.start_initial().await?;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let context = ManagementContext::new("example", None, supervisor.clone())
            .with_deployment(deployment.clone());
        let server = tokio::spawn(serve(listener, context));

        let stream = TcpStream::connect(address).await?;
        let mut client = Framed::new(stream, ClientCodec::default());

        let signed = SignedDeployment::sign(
            Deployment {
                application: "example".to_string(),
                version: "1.2.3".to_string(),
                artifact: Artifact {
                    url: format!("file://{}", archive.display()),
                    sha256: digest,
                    size_bytes: size,
                },
                issued_at_unix: 1_700_000_000,
            },
            "release-2026",
            &signing_key,
        )?;

        let deploy = send(
            &mut client,
            Request::current(RequestId::new(1), Command::Deploy(signed)),
        )
        .await?;
        match deploy.body {
            Outcome::DeploymentAccepted(accepted) => assert_eq!(accepted.version, "1.2.3"),
            other => panic!("unexpected deployment response: {other:?}"),
        }

        let status = send(
            &mut client,
            Request::current(RequestId::new(2), Command::Status),
        )
        .await?;
        match status.body {
            Outcome::Status(response) => {
                assert_eq!(response.deployed_version.as_deref(), Some("1.2.3"));
                assert_eq!(response.instances.len(), 1);
            }
            other => panic!("unexpected status response: {other:?}"),
        }

        let metrics = send(
            &mut client,
            Request::current(
                RequestId::new(3),
                Command::Metrics(MetricsRequest { instance: None }),
            ),
        )
        .await?;
        match metrics.body {
            Outcome::Metrics(response) => {
                let application = response
                    .application
                    .ok_or("application metrics were not reported")?;
                assert_eq!(application.deployed_version.as_deref(), Some("1.2.3"));
                assert_eq!(application.instances.len(), 1);
            }
            other => panic!("unexpected metrics response: {other:?}"),
        }

        server.abort();
        supervisor.lock().await.stop_all().await?;
        Ok(())
    }
}
