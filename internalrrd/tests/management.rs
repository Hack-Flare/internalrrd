use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use futures_util::{SinkExt, StreamExt};
use internalrrd::config::{
    ApplicationConfig, ApplicationHealthConfig, TrustConfig, TrustedKeyConfig,
};
use internalrrd::deployment::Deployment as DeploymentManager;
use internalrrd::management::{ManagementContext, serve};
use internalrrd::ports::PortAllocator;
use internalrrd::process::ProcessSpec;
use internalrrd::release::ReleaseManager;
use internalrrd::supervisor::Supervisor;
use internalrrd::trust::TrustStore;
use protocol::{
    Artifact, Capability, CodecError, Command, Deployment, DeploymentSignature, HelloRequest,
    MetricsRequest, Outcome, ProtocolErrorCode, ProtocolVersion, RejectionCode, Request,
    RequestCodec, RequestId, Response, ResponseCodec, SignatureAlgorithm, SignedDeployment,
    SigningKey,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_util::codec::{Decoder, Encoder, Framed};

mod common;

use common::create_artifact;

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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn deploys_through_the_management_protocol() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let (archive, digest, size) =
        create_artifact(temporary.path(), "run.sh", b"#!/bin/sh\nexec sleep 30\n")?;
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
        health: ApplicationHealthConfig {
            enabled: false,
            ..ApplicationHealthConfig::default()
        },
        release_directory: temporary.path().join("releases"),
        max_artifact_bytes: 1024 * 1024,
        ports: Vec::new(),
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
