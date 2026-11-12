use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::CodecError;
use crate::signature::SignedDeployment;

/// Identifies one request and its response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RequestId(u64);

impl RequestId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for RequestId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The stable type tag carried by every wire frame.
///
/// Request and response tags occupy separate ranges, so a message sent in the wrong direction is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageType(u16);

impl MessageType {
    pub const HELLO_REQUEST: Self = Self(0x0001);
    pub const DEPLOY_REQUEST: Self = Self(0x0002);
    pub const STATUS_REQUEST: Self = Self(0x0003);
    pub const METRICS_REQUEST: Self = Self(0x0004);

    pub const HELLO_RESPONSE: Self = Self(0x8001);
    pub const DEPLOYMENT_ACCEPTED: Self = Self(0x8002);
    pub const DEPLOYMENT_REJECTED: Self = Self(0x8003);
    pub const STATUS_RESPONSE: Self = Self(0x8004);
    pub const ERROR_RESPONSE: Self = Self(0x8005);
    pub const METRICS_RESPONSE: Self = Self(0x8006);

    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }
}

impl fmt::Display for MessageType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "0x{:04x}", self.0)
    }
}

/// A capability advertised during the handshake.
///
/// Values are stable on the wire and unknown values are preserved for forward compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Capability(u16);

impl Capability {
    pub const SIGNED_DEPLOYMENTS: Self = Self(1);
    pub const STATUS_REPORTING: Self = Self(2);
    pub const METRICS_REPORTING: Self = Self(3);

    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }
}

/// Why a server rejected a deployment notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RejectionCode(u16);

impl RejectionCode {
    pub const INVALID_SIGNATURE: Self = Self(1);
    pub const UNSUPPORTED_VERSION: Self = Self(2);
    pub const OUTDATED_RELEASE: Self = Self(3);
    pub const ALREADY_DEPLOYED: Self = Self(4);
    pub const ARTIFACT_UNAVAILABLE: Self = Self(5);
    pub const HEALTH_CHECK_FAILED: Self = Self(6);
    pub const INTERNAL: Self = Self(7);

    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }
}

/// A transport-level or request-level protocol failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProtocolErrorCode(u16);

impl ProtocolErrorCode {
    pub const UNSUPPORTED_VERSION: Self = Self(1);
    pub const INVALID_MESSAGE: Self = Self(2);
    pub const UNKNOWN_MESSAGE_TYPE: Self = Self(3);
    pub const UNAUTHORIZED: Self = Self(4);
    pub const INTERNAL: Self = Self(5);

    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloRequest {
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResponse {
    pub application: String,
    pub capabilities: Vec<Capability>,
    pub deployed_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentAccepted {
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentRejected {
    pub code: RejectionCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ProtocolErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub id: u64,
    pub pid: u32,
    pub healthy: bool,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    pub application: String,
    pub deployed_version: Option<String>,
    pub instances: Vec<InstanceStatus>,
}

/// Requests a metrics snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsRequest {
    /// When set, restrict instance metrics to this application instance.
    pub instance: Option<u64>,
}

/// A snapshot of service and application metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsReport {
    /// Unix timestamp in seconds when the snapshot was captured.
    pub captured_at_unix: u64,
    pub internalrrd: ComponentMetrics,
    pub application: Option<ApplicationMetrics>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplicationMetrics {
    pub name: String,
    pub deployed_version: Option<String>,
    pub aggregate: ComponentMetrics,
    pub instances: Vec<InstanceMetrics>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceMetrics {
    pub id: u64,
    pub pid: u32,
    pub healthy: bool,
    pub uptime_secs: u64,
    pub metrics: ComponentMetrics,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentMetrics {
    pub resources: ResourceMetrics,
    pub traffic: TrafficMetrics,
    pub custom: Vec<CustomMetric>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceMetrics {
    /// CPU usage in basis points, where 10_000 is one fully used core.
    pub cpu_basis_points: u32,
    pub memory_used_bytes: u64,
    pub memory_limit_bytes: Option<u64>,
    pub storage_used_bytes: u64,
    pub storage_limit_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficMetrics {
    pub total_requests: u64,
    pub requests_last_24h: u64,
    /// Average requests per second over the reporting window, scaled by 1000.
    pub average_requests_per_second_milli: u64,
    pub average_window_seconds: u32,
}

/// An application specific metric that has no typed field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomMetric {
    pub name: String,
    pub value: f64,
    pub unit: String,
}

/// Messages sent by the control plane to a server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    Hello(HelloRequest),
    Deploy(SignedDeployment),
    Status,
    Metrics(MetricsRequest),
}

/// Messages sent by a server to the control plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Hello(HelloResponse),
    DeploymentAccepted(DeploymentAccepted),
    DeploymentRejected(DeploymentRejected),
    Status(StatusReport),
    Metrics(Box<MetricsReport>),
    Error(ProtocolError),
}

/// A typed message body that can be carried in an envelope.
pub trait MessageBody: Sized {
    /// The stable type tag for this body.
    fn message_type(&self) -> MessageType;

    /// Encode the body without the envelope.
    fn encode_payload(&self) -> Result<Vec<u8>, CodecError>;

    /// Decode a body after checking its type tag.
    fn decode_payload(message_type: MessageType, payload: &[u8]) -> Result<Self, CodecError>;
}

impl MessageBody for Command {
    fn message_type(&self) -> MessageType {
        match self {
            Self::Hello(_) => MessageType::HELLO_REQUEST,
            Self::Deploy(_) => MessageType::DEPLOY_REQUEST,
            Self::Status => MessageType::STATUS_REQUEST,
            Self::Metrics(_) => MessageType::METRICS_REQUEST,
        }
    }

    fn encode_payload(&self) -> Result<Vec<u8>, CodecError> {
        postcard::to_allocvec(self).map_err(CodecError::Encode)
    }

    fn decode_payload(message_type: MessageType, payload: &[u8]) -> Result<Self, CodecError> {
        match message_type {
            MessageType::HELLO_REQUEST
            | MessageType::DEPLOY_REQUEST
            | MessageType::STATUS_REQUEST
            | MessageType::METRICS_REQUEST => {}
            other => return Err(CodecError::UnknownMessageType(other.raw())),
        }

        let command: Command = postcard::from_bytes(payload).map_err(CodecError::Decode)?;
        let actual = command.message_type();
        if actual != message_type {
            return Err(CodecError::MessageTypeMismatch {
                declared: message_type.raw(),
                actual: actual.raw(),
            });
        }
        Ok(command)
    }
}

impl MessageBody for Outcome {
    fn message_type(&self) -> MessageType {
        match self {
            Self::Hello(_) => MessageType::HELLO_RESPONSE,
            Self::DeploymentAccepted(_) => MessageType::DEPLOYMENT_ACCEPTED,
            Self::DeploymentRejected(_) => MessageType::DEPLOYMENT_REJECTED,
            Self::Status(_) => MessageType::STATUS_RESPONSE,
            Self::Metrics(_) => MessageType::METRICS_RESPONSE,
            Self::Error(_) => MessageType::ERROR_RESPONSE,
        }
    }

    fn encode_payload(&self) -> Result<Vec<u8>, CodecError> {
        postcard::to_allocvec(self).map_err(CodecError::Encode)
    }

    fn decode_payload(message_type: MessageType, payload: &[u8]) -> Result<Self, CodecError> {
        match message_type {
            MessageType::HELLO_RESPONSE
            | MessageType::DEPLOYMENT_ACCEPTED
            | MessageType::DEPLOYMENT_REJECTED
            | MessageType::STATUS_RESPONSE
            | MessageType::METRICS_RESPONSE
            | MessageType::ERROR_RESPONSE => {}
            other => return Err(CodecError::UnknownMessageType(other.raw())),
        }

        let outcome: Outcome = postcard::from_bytes(payload).map_err(CodecError::Decode)?;
        let actual = outcome.message_type();
        if actual != message_type {
            return Err(CodecError::MessageTypeMismatch {
                declared: message_type.raw(),
                actual: actual.raw(),
            });
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_request_types() {
        let result = Command::decode_payload(MessageType::new(0x7fff), &[]);

        assert!(matches!(
            result,
            Err(CodecError::UnknownMessageType(0x7fff))
        ));
    }

    #[test]
    fn rejects_unknown_response_types() {
        let result = Outcome::decode_payload(MessageType::new(0xfffe), &[]);

        assert!(matches!(
            result,
            Err(CodecError::UnknownMessageType(0xfffe))
        ));
    }

    #[test]
    fn rejects_mismatched_message_types() -> Result<(), CodecError> {
        let payload = Command::Status.encode_payload()?;
        let result = Command::decode_payload(MessageType::HELLO_REQUEST, &payload);

        assert!(matches!(
            result,
            Err(CodecError::MessageTypeMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn preserves_unknown_capabilities_on_the_wire() -> Result<(), CodecError> {
        let request = HelloRequest {
            capabilities: vec![Capability::SIGNED_DEPLOYMENTS, Capability::new(0x1234)],
        };

        let payload = Command::Hello(request.clone()).encode_payload()?;
        let decoded = Command::decode_payload(MessageType::HELLO_REQUEST, &payload)?;

        assert_eq!(decoded, Command::Hello(request));
        Ok(())
    }
}
