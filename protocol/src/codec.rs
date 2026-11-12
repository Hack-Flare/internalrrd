use std::marker::PhantomData;

use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use tokio_util::codec::{Decoder, Encoder};

use crate::error::CodecError;
use crate::message::{Command, MessageBody, MessageType, Outcome, RequestId};
use crate::version::ProtocolVersion;

/// Number of bytes in the big-endian frame length prefix.
pub const LENGTH_PREFIX_BYTES: usize = 4;

/// Maximum size of one encoded frame payload.
pub const MAX_FRAME_PAYLOAD_BYTES: usize = 1024 * 1024;

/// A versioned protocol message with a request identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope<B> {
    pub version: ProtocolVersion,
    pub request_id: RequestId,
    pub body: B,
}

impl<B> Envelope<B> {
    pub const fn new(version: ProtocolVersion, request_id: RequestId, body: B) -> Self {
        Self {
            version,
            request_id,
            body,
        }
    }

    pub const fn current(request_id: RequestId, body: B) -> Self {
        Self::new(ProtocolVersion::CURRENT, request_id, body)
    }

    /// Whether this envelope uses a protocol version compatible with this build.
    pub fn is_compatible(&self) -> bool {
        self.version.is_compatible(ProtocolVersion::CURRENT)
    }
}

impl<B: MessageBody> Envelope<B> {
    /// Encode this envelope into a complete length-prefixed frame.
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let payload = self.body.encode_payload()?;
        let wire = WireFrame {
            version: self.version,
            request_id: self.request_id,
            message_type: self.body.message_type(),
            payload,
        };
        encode_wire_frame(&wire)
    }

    /// Decode a complete length-prefixed frame into an envelope.
    pub fn decode(frame: &[u8]) -> Result<Self, CodecError> {
        let wire = decode_wire_frame(frame)?;
        let body = B::decode_payload(wire.message_type, &wire.payload)?;
        Ok(Self {
            version: wire.version,
            request_id: wire.request_id,
            body,
        })
    }
}

/// A control plane request.
pub type Request = Envelope<Command>;

/// A server response.
pub type Response = Envelope<Outcome>;

/// A tokio codec for control plane requests.
pub type RequestCodec = ProtocolCodec<Command>;

/// A tokio codec for server responses.
pub type ResponseCodec = ProtocolCodec<Outcome>;

#[derive(Debug, Serialize, Deserialize)]
struct WireFrame {
    version: ProtocolVersion,
    request_id: RequestId,
    message_type: MessageType,
    payload: Vec<u8>,
}

fn encode_wire_frame(frame: &WireFrame) -> Result<Vec<u8>, CodecError> {
    let encoded = postcard::to_allocvec(frame).map_err(CodecError::Encode)?;
    if encoded.len() > MAX_FRAME_PAYLOAD_BYTES {
        return Err(CodecError::FrameTooLarge {
            actual: encoded.len(),
            max: MAX_FRAME_PAYLOAD_BYTES,
        });
    }

    let length = u32::try_from(encoded.len()).map_err(|_| CodecError::FrameTooLarge {
        actual: encoded.len(),
        max: MAX_FRAME_PAYLOAD_BYTES,
    })?;

    let mut bytes = Vec::with_capacity(LENGTH_PREFIX_BYTES + encoded.len());
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

fn decode_wire_frame(frame: &[u8]) -> Result<WireFrame, CodecError> {
    let payload = frame_payload(frame)?;
    postcard::from_bytes(payload).map_err(CodecError::Decode)
}

fn frame_payload(frame: &[u8]) -> Result<&[u8], CodecError> {
    if frame.len() < LENGTH_PREFIX_BYTES {
        return Err(CodecError::FrameTooShort {
            actual: frame.len(),
            prefix: LENGTH_PREFIX_BYTES,
        });
    }

    let declared = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    if declared > MAX_FRAME_PAYLOAD_BYTES {
        return Err(CodecError::FrameTooLarge {
            actual: declared,
            max: MAX_FRAME_PAYLOAD_BYTES,
        });
    }

    let payload = &frame[LENGTH_PREFIX_BYTES..];
    if payload.len() != declared {
        return Err(CodecError::FrameLengthMismatch {
            declared,
            actual: payload.len(),
        });
    }
    Ok(payload)
}

fn frame_length(source: &[u8]) -> Result<Option<usize>, CodecError> {
    if source.len() < LENGTH_PREFIX_BYTES {
        return Ok(None);
    }

    let length = u32::from_be_bytes([source[0], source[1], source[2], source[3]]) as usize;
    if length > MAX_FRAME_PAYLOAD_BYTES {
        return Err(CodecError::FrameTooLarge {
            actual: length,
            max: MAX_FRAME_PAYLOAD_BYTES,
        });
    }
    Ok(Some(length))
}

/// A tokio codec for length-prefixed protocol frames.
pub struct ProtocolCodec<B> {
    marker: PhantomData<B>,
}

impl<B> ProtocolCodec<B> {
    pub const fn new() -> Self {
        Self {
            marker: PhantomData,
        }
    }
}

impl<B> Default for ProtocolCodec<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: MessageBody> Encoder<Envelope<B>> for ProtocolCodec<B> {
    type Error = CodecError;

    fn encode(&mut self, item: Envelope<B>, destination: &mut BytesMut) -> Result<(), Self::Error> {
        let frame = item.encode()?;
        destination.extend_from_slice(&frame);
        Ok(())
    }
}

impl<B: MessageBody> Decoder for ProtocolCodec<B> {
    type Item = Envelope<B>;
    type Error = CodecError;

    fn decode(&mut self, source: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(length) = frame_length(source)? else {
            return Ok(None);
        };

        let total = LENGTH_PREFIX_BYTES + length;
        if source.len() < total {
            source.reserve(total - source.len());
            return Ok(None);
        }

        let frame = source.split_to(total);
        Envelope::<B>::decode(&frame[..]).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{
        ApplicationMetrics, ComponentMetrics, CustomMetric, DeploymentAccepted, HelloResponse,
        InstanceMetrics, InstanceStatus, MetricsReport, MetricsRequest, ResourceMetrics,
        StatusReport, TrafficMetrics,
    };
    use crate::signature::{Artifact, Deployment, SignedDeployment};

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
            signature: crate::signature::DeploymentSignature {
                algorithm: crate::signature::SignatureAlgorithm::ED25519,
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
            Command::Hello(crate::message::HelloRequest {
                capabilities: vec![crate::message::Capability::SIGNED_DEPLOYMENTS],
            }),
        );
        let frame = request.encode()?;
        assert_eq!(Request::decode(&frame)?, request);

        let response: Response = Envelope::current(
            RequestId::new(1),
            Outcome::Hello(HelloResponse {
                application: "example".to_string(),
                capabilities: vec![crate::message::Capability::STATUS_REPORTING],
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
            Outcome::DeploymentRejected(crate::message::DeploymentRejected {
                code: crate::message::RejectionCode::INVALID_SIGNATURE,
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
}
