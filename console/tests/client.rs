use std::error::Error;
use std::time::Duration;

use bytes::BytesMut;
use console::client::{Client, ClientError};
use console::config::SigningConfig;
use console::signing::Signer;
use futures_util::{SinkExt, StreamExt};
use protocol::{
    ApplicationMetrics, Artifact, Capability, CodecError, Command, ComponentMetrics, Deployment,
    DeploymentAccepted, DeploymentRejected, HelloResponse, InstanceMetrics, InstanceStatus,
    MetricsReport, Outcome, RejectionCode, Request, RequestCodec, ResourceMetrics, Response,
    ResponseCodec, StatusReport, TrafficMetrics, VerifyingKey,
};
use tokio::net::TcpListener;
use tokio_util::codec::{Decoder, Encoder, Framed};

fn deployment() -> Deployment {
    Deployment {
        application: "example".to_string(),
        version: "1.2.3".to_string(),
        artifact: Artifact {
            url: "https://artifacts.example/example-1.2.3.tar.gz".to_string(),
            sha256: [0xab; 32],
            size_bytes: 4096,
        },
        issued_at_unix: 1_700_000_000,
    }
}

fn component_metrics() -> ComponentMetrics {
    ComponentMetrics {
        resources: ResourceMetrics {
            cpu_basis_points: 100,
            memory_used_bytes: 1024,
            memory_limit_bytes: None,
            storage_used_bytes: 2048,
            storage_limit_bytes: Some(4096),
        },
        traffic: TrafficMetrics {
            total_requests: 10,
            requests_last_24h: 10,
            average_requests_per_second_milli: 100,
            average_window_seconds: 100,
        },
        custom: Vec::new(),
    }
}

fn metrics_report(instance: Option<u64>) -> MetricsReport {
    let metrics = component_metrics();
    let mut instances = vec![InstanceMetrics {
        id: 0,
        pid: 42,
        healthy: true,
        uptime_secs: 10,
        metrics: metrics.clone(),
    }];
    if let Some(instance) = instance {
        instances.retain(|metrics| metrics.id == instance);
    }

    MetricsReport {
        captured_at_unix: 1_700_000_000,
        internalrrd: metrics.clone(),
        application: Some(ApplicationMetrics {
            name: "example".to_string(),
            deployed_version: Some("1.2.3".to_string()),
            aggregate: metrics,
            instances,
        }),
    }
}

#[derive(Default)]
struct ServerCodec {
    requests: RequestCodec,
    responses: ResponseCodec,
}

impl Encoder<Response> for ServerCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Response, destination: &mut BytesMut) -> Result<(), Self::Error> {
        self.responses.encode(item, destination)
    }
}

impl Decoder for ServerCodec {
    type Item = Request;
    type Error = CodecError;

    fn decode(&mut self, source: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.requests.decode(source)
    }
}

async fn serve(
    listener: TcpListener,
    verifying_key: VerifyingKey,
    application: &'static str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (stream, _) = listener.accept().await?;
    let mut framed = Framed::new(stream, ServerCodec::default());

    while let Some(request) = framed.next().await {
        let request = request?;
        let version = request.version;
        let request_id = request.request_id;

        let response = match request.body {
            Command::Hello(_) => Outcome::Hello(HelloResponse {
                application: application.to_string(),
                capabilities: vec![
                    Capability::SIGNED_DEPLOYMENTS,
                    Capability::STATUS_REPORTING,
                    Capability::METRICS_REPORTING,
                ],
                deployed_version: Some("1.2.3".to_string()),
            }),
            Command::Deploy(signed) => match signed.verify(&verifying_key) {
                Ok(()) => Outcome::DeploymentAccepted(DeploymentAccepted {
                    version: signed.deployment.version.clone(),
                }),
                Err(error) => Outcome::DeploymentRejected(DeploymentRejected {
                    code: RejectionCode::INVALID_SIGNATURE,
                    message: error.to_string(),
                }),
            },
            Command::Status => Outcome::Status(StatusReport {
                application: application.to_string(),
                deployed_version: Some("1.2.3".to_string()),
                instances: vec![InstanceStatus {
                    id: 0,
                    pid: 42,
                    healthy: true,
                    uptime_secs: 10,
                }],
            }),
            Command::Metrics(request) => {
                Outcome::Metrics(Box::new(metrics_report(request.instance)))
            }
        };

        framed
            .send(Response::new(version, request_id, response))
            .await?;
    }

    Ok(())
}

#[tokio::test]
async fn drives_a_management_session() -> Result<(), Box<dyn Error>> {
    let key = protocol::SigningKey::from_bytes(&[7; 32]);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(serve(listener, key.verifying_key(), "example"));

    let signer = Signer::from_config(&SigningConfig {
        key_id: "release-2026".to_string(),
        private_key: hex::encode(key.to_bytes()),
    })?;
    let signed = signer.sign(deployment())?;

    let mut client = Client::connect(address, "example", Duration::from_secs(2)).await?;
    assert_eq!(client.hello().deployed_version.as_deref(), Some("1.2.3"));
    assert!(
        client
            .hello()
            .capabilities
            .contains(&Capability::METRICS_REPORTING)
    );

    let accepted = client.deploy(signed).await?;
    assert_eq!(accepted.version, "1.2.3");

    let status = client.status().await?;
    assert_eq!(status.instances.len(), 1);
    assert_eq!(status.deployed_version.as_deref(), Some("1.2.3"));

    let report = client.metrics(Some(0)).await?;
    let application = report
        .application
        .ok_or("application metrics were not reported")?;
    assert_eq!(application.instances.len(), 1);

    drop(client);
    server
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tokio::test]
async fn reports_rejected_deployments() -> Result<(), Box<dyn Error>> {
    let key = protocol::SigningKey::from_bytes(&[7; 32]);
    let other_key = protocol::SigningKey::from_bytes(&[9; 32]);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(serve(listener, other_key.verifying_key(), "example"));

    let signer = Signer::from_config(&SigningConfig {
        key_id: "release-2026".to_string(),
        private_key: hex::encode(key.to_bytes()),
    })?;
    let signed = signer.sign(deployment())?;

    let mut client = Client::connect(address, "example", Duration::from_secs(2)).await?;
    let result = client.deploy(signed).await;

    assert!(matches!(
        result,
        Err(ClientError::Rejected {
            code,
            ..
        }) if code == RejectionCode::INVALID_SIGNATURE.raw()
    ));

    drop(client);
    server
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tokio::test]
async fn rejects_a_different_server_application() -> Result<(), Box<dyn Error>> {
    let key = protocol::SigningKey::from_bytes(&[7; 32]);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(serve(listener, key.verifying_key(), "other"));

    let result = Client::connect(address, "example", Duration::from_secs(2)).await;

    assert!(matches!(
        result,
        Err(ClientError::ApplicationMismatch { .. })
    ));

    server
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(())
}
