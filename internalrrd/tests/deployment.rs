use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use internalrrd::config::{
    ApplicationConfig, ApplicationHealthConfig, ApplicationPortConfig, ProxyProtocol, TrustConfig,
    TrustedKeyConfig,
};
use internalrrd::deployment::{Deployment, DeploymentError};
use internalrrd::ports::PortAllocator;
use internalrrd::release::ReleaseManager;
use internalrrd::supervisor::Supervisor;
use internalrrd::trust::TrustStore;
use protocol::{
    Artifact, Deployment as ReleaseDeployment, Outcome, RejectionCode, SignedDeployment, SigningKey,
};
use tokio::sync::Mutex;

mod common;

use common::create_artifact;

const KEY_ID: &str = "release-2026";

fn application(directory: &Path) -> ApplicationConfig {
    ApplicationConfig {
        name: "example".to_string(),
        command: PathBuf::from("{release_dir}/run.sh"),
        args: vec![
            "--dns-port".to_string(),
            "{port:dns}".to_string(),
            "--http-port".to_string(),
            "{port:http}".to_string(),
        ],
        working_directory: None,
        environment: BTreeMap::new(),
        shutdown_timeout_secs: 1,
        instances: 2,
        startup_grace_secs: 0,
        health: ApplicationHealthConfig {
            enabled: false,
            ..ApplicationHealthConfig::default()
        },
        release_directory: directory.join("releases"),
        max_artifact_bytes: 1024 * 1024,
        ports: vec![
            ApplicationPortConfig {
                name: "dns".to_string(),
                protocols: vec![ProxyProtocol::Udp, ProxyProtocol::Tcp],
            },
            ApplicationPortConfig {
                name: "http".to_string(),
                protocols: vec![ProxyProtocol::Tcp],
            },
        ],
    }
}

fn running_application(directory: &Path, health: ApplicationHealthConfig) -> ApplicationConfig {
    ApplicationConfig {
        name: "example".to_string(),
        command: PathBuf::from("sh"),
        args: vec!["-c".to_string(), "sleep 30".to_string()],
        working_directory: None,
        environment: BTreeMap::new(),
        shutdown_timeout_secs: 1,
        instances: 1,
        startup_grace_secs: 0,
        health,
        release_directory: directory.join("releases"),
        max_artifact_bytes: 1024 * 1024,
        ports: vec![ApplicationPortConfig {
            name: "http".to_string(),
            protocols: vec![ProxyProtocol::Tcp],
        }],
    }
}

fn trust(signing_key: &SigningKey) -> TrustStore {
    let config = TrustConfig {
        keys: vec![TrustedKeyConfig {
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
    let (archive, digest, size) =
        create_artifact(temporary.path(), "run.sh", b"#!/bin/sh\nexec sleep 30\n")?;
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
        Outcome::DeploymentRejected(protocol::DeploymentRejected {
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
    let (archive, digest, size) =
        create_artifact(temporary.path(), "run.sh", b"#!/bin/sh\nexec sleep 30\n")?;
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
        Outcome::DeploymentRejected(protocol::DeploymentRejected {
            code: RejectionCode::INVALID_SIGNATURE,
            ..
        })
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_deployments_that_never_become_ready() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let (archive, digest, size) =
        create_artifact(temporary.path(), "run.sh", b"#!/bin/sh\nexec sleep 30\n")?;
    let signing_key = SigningKey::from_bytes(&[7; 32]);
    let mut application = application(temporary.path());
    application.health = ApplicationHealthConfig {
        enabled: true,
        interval_secs: 1,
        timeout_secs: 1,
    };

    let supervisor = Arc::new(Mutex::new(Supervisor::new()));
    let deployment = Deployment::new(
        application,
        trust(&signing_key),
        ReleaseManager::new(temporary.path().join("releases"), 1024 * 1024)?,
        None,
        PortAllocator::new(None, []),
        supervisor.clone(),
    );

    let outcome = deployment
        .deploy(signed(
            &signing_key,
            &archive,
            digest,
            size,
            "1.2.3",
            1_700_000_000,
        ))
        .await;

    assert!(matches!(
        outcome,
        Outcome::DeploymentRejected(protocol::DeploymentRejected {
            code: RejectionCode::HEALTH_CHECK_FAILED,
            ..
        })
    ));
    assert!(supervisor.lock().await.running().is_empty());
    assert!(deployment.current_version().await.is_none());
    Ok(())
}

#[tokio::test]
async fn starts_instances_from_the_configuration() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let signing_key = SigningKey::from_bytes(&[7; 32]);
    let supervisor = Arc::new(Mutex::new(Supervisor::new()));
    let deployment = Deployment::new(
        running_application(
            temporary.path(),
            ApplicationHealthConfig {
                enabled: false,
                ..ApplicationHealthConfig::default()
            },
        ),
        trust(&signing_key),
        ReleaseManager::new(temporary.path().join("releases"), 1024 * 1024)?,
        None,
        PortAllocator::new(None, []),
        supervisor.clone(),
    );

    deployment.start_initial().await?;

    assert_eq!(supervisor.lock().await.running().len(), 1);
    assert!(deployment.current_version().await.is_none());
    supervisor.lock().await.stop_all().await?;
    Ok(())
}

#[tokio::test]
async fn rejects_unready_instances_at_startup() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let signing_key = SigningKey::from_bytes(&[7; 32]);
    let supervisor = Arc::new(Mutex::new(Supervisor::new()));
    let deployment = Deployment::new(
        running_application(
            temporary.path(),
            ApplicationHealthConfig {
                enabled: true,
                interval_secs: 1,
                timeout_secs: 1,
            },
        ),
        trust(&signing_key),
        ReleaseManager::new(temporary.path().join("releases"), 1024 * 1024)?,
        None,
        PortAllocator::new(None, []),
        supervisor.clone(),
    );

    let error = deployment
        .start_initial()
        .await
        .expect_err("startup should fail readiness");

    assert!(matches!(error, DeploymentError::NotReady(_)));
    assert!(supervisor.lock().await.running().is_empty());
    assert!(deployment.current_version().await.is_none());
    Ok(())
}
