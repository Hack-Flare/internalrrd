use bytes::BytesMut;
use protocol::{
    ApplicationMetrics, Artifact, Capability, CodecError, Command, ComponentMetrics, CustomMetric,
    Deployment, DeploymentAccepted, DeploymentRejected, DeploymentSignature, Envelope,
    HelloRequest, HelloResponse, InstanceMetrics, InstanceStatus, MAX_FRAME_PAYLOAD_BYTES,
    MetricsReport, MetricsRequest, Outcome, ProtocolCodec, ProtocolVersion, RejectionCode, Request,
    RequestId, ResourceMetrics, Response, SignatureAlgorithm, SignedDeployment, StatusReport,
    TrafficMetrics,
};
use tokio_util::codec::Decoder;

fn request(request_id: u64) -> Request {
    Envelope::current(RequestId::new(request_id), Command::Status)
}

fn deployment() -> SignedDeployment {
    let deployment = Deployment {
        application: "example".to_string(),
        version: "1.2.3".to_string(),
        artifact: Artifact {
            url: "https://artifacts.example/example-1.2.3".to_string(),
            sha256: [0xab; 32],
            size_bytes: 4096,
        },
        issued_at_unix: 1_700_000_000,
    };

    SignedDeployment {
        deployment,
        signature: DeploymentSignature {
            algorithm: SignatureAlgorithm::ED25519,
            key_id: "release-2026".to_string(),
            bytes: vec![0; 64],
        },
    }
}

fn component_metrics() -> ComponentMetrics {
    ComponentMetrics {
        resources: ResourceMetrics {
            cpu_basis_points: 1250,
            memory_used_bytes: 64 * 1024 * 1024,
            memory_limit_bytes: Some(128 * 1024 * 1024),
            storage_used_bytes: 512 * 1024 * 1024,
            storage_limit_bytes: None,
        },
        traffic: TrafficMetrics {
            total_requests: 1_000_000,
            requests_last_24h: 50_000,
            average_requests_per_second_milli: 1250,
            average_window_seconds: 60,
        },
        custom: vec![CustomMetric {
            name: "queue_depth".to_string(),
            value: 3.5,
            unit: "count".to_string(),
        }],
    }
}

fn metrics_report() -> MetricsReport {
    let metrics = component_metrics();

    MetricsReport {
        captured_at_unix: 1_700_000_000,
        internalrrd: metrics.clone(),
        application: Some(ApplicationMetrics {
            name: "example".to_string(),
            deployed_version: Some("1.2.3".to_string()),
            aggregate: metrics.clone(),
            instances: vec![InstanceMetrics {
                id: 0,
                pid: 42,
                healthy: true,
                uptime_secs: 3600,
                metrics,
            }],
        }),
    }
}

#[test]
fn roundtrips_a_request() -> Result<(), CodecError> {
    let original = request(7);
    let frame = original.encode()?;
    let decoded = Request::decode(&frame)?;

    assert_eq!(decoded, original);
    Ok(())
}

#[test]
fn roundtrips_a_deploy_request() -> Result<(), CodecError> {
    let original = Envelope::current(RequestId::new(8), Command::Deploy(deployment()));
    let frame = original.encode()?;
    let decoded = Request::decode(&frame)?;

    assert_eq!(decoded, original);
    Ok(())
}

#[test]
fn roundtrips_a_response() -> Result<(), CodecError> {
    let original: Response = Envelope::current(
        RequestId::new(9),
        Outcome::Status(StatusReport {
            application: "example".to_string(),
            deployed_version: Some("1.2.3".to_string()),
            instances: vec![InstanceStatus {
                id: 0,
                pid: 42,
                healthy: true,
                uptime_secs: 10,
            }],
        }),
    );
    let frame = original.encode()?;
    let decoded = Response::decode(&frame)?;

    assert_eq!(decoded, original);
    Ok(())
}

#[test]
fn roundtrips_hello_messages() -> Result<(), CodecError> {
    let request: Request = Envelope::current(
        RequestId::new(1),
        Command::Hello(HelloRequest {
            capabilities: vec![Capability::SIGNED_DEPLOYMENTS],
        }),
    );
    let frame = request.encode()?;
    assert_eq!(Request::decode(&frame)?, request);

    let response: Response = Envelope::current(
        RequestId::new(1),
        Outcome::Hello(HelloResponse {
            application: "example".to_string(),
            capabilities: vec![Capability::STATUS_REPORTING],
            deployed_version: None,
        }),
    );
    let frame = response.encode()?;
    assert_eq!(Response::decode(&frame)?, response);
    Ok(())
}

#[test]
fn roundtrips_deployment_outcomes() -> Result<(), CodecError> {
    let accepted = Envelope::current(
        RequestId::new(2),
        Outcome::DeploymentAccepted(DeploymentAccepted {
            version: "1.2.3".to_string(),
        }),
    );
    assert_eq!(Response::decode(&accepted.encode()?)?, accepted);

    let rejected = Envelope::current(
        RequestId::new(3),
        Outcome::DeploymentRejected(DeploymentRejected {
            code: RejectionCode::INVALID_SIGNATURE,
            message: "bad signature".to_string(),
        }),
    );
    assert_eq!(Response::decode(&rejected.encode()?)?, rejected);
    Ok(())
}

#[test]
fn roundtrips_a_metrics_request() -> Result<(), CodecError> {
    let original: Request = Envelope::current(
        RequestId::new(10),
        Command::Metrics(MetricsRequest { instance: Some(3) }),
    );
    let frame = original.encode()?;
    let decoded = Request::decode(&frame)?;

    assert_eq!(decoded, original);
    Ok(())
}

#[test]
fn roundtrips_a_metrics_response() -> Result<(), CodecError> {
    let original: Response = Envelope::current(
        RequestId::new(11),
        Outcome::Metrics(Box::new(metrics_report())),
    );
    let frame = original.encode()?;
    let decoded = Response::decode(&frame)?;

    assert_eq!(decoded, original);
    Ok(())
}

#[test]
fn decodes_frames_delivered_in_parts() -> Result<(), CodecError> {
    let original = request(3);
    let encoded = original.encode()?;
    let mut codec: ProtocolCodec<Command> = ProtocolCodec::new();
    let mut buffer = BytesMut::new();

    buffer.extend_from_slice(&encoded[..encoded.len() - 1]);
    assert!(codec.decode(&mut buffer)?.is_none());

    buffer.extend_from_slice(&encoded[encoded.len() - 1..]);
    assert_eq!(codec.decode(&mut buffer)?, Some(original));
    Ok(())
}

#[test]
fn decodes_multiple_frames() -> Result<(), CodecError> {
    let first = request(4);
    let second = request(5);
    let mut codec: ProtocolCodec<Command> = ProtocolCodec::new();
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&first.encode()?);
    buffer.extend_from_slice(&second.encode()?);

    assert_eq!(codec.decode(&mut buffer)?, Some(first));
    assert_eq!(codec.decode(&mut buffer)?, Some(second));
    assert!(codec.decode(&mut buffer)?.is_none());
    Ok(())
}

#[test]
fn rejects_short_frames() {
    assert!(matches!(
        Request::decode(&[0, 1]),
        Err(CodecError::FrameTooShort {
            actual: 2,
            prefix: 4
        })
    ));
}

#[test]
fn rejects_oversized_frames() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&((MAX_FRAME_PAYLOAD_BYTES + 1) as u32).to_be_bytes());
    let mut codec: ProtocolCodec<Command> = ProtocolCodec::new();

    assert!(matches!(
        codec.decode(&mut buffer),
        Err(CodecError::FrameTooLarge { .. })
    ));
}

#[test]
fn rejects_length_mismatches() -> Result<(), CodecError> {
    let mut frame = request(6).encode()?;
    frame.push(0);

    assert!(matches!(
        Request::decode(&frame),
        Err(CodecError::FrameLengthMismatch { .. })
    ));
    Ok(())
}

#[test]
fn reports_incompatible_versions() {
    let envelope: Request = Envelope::new(
        ProtocolVersion::new(2, 0),
        RequestId::new(1),
        Command::Status,
    );

    assert!(!envelope.is_compatible());
}
