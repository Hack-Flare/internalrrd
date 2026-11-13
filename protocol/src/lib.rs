pub mod codec;
pub mod error;
pub mod message;
pub mod signature;
pub mod version;

/// The default TCP port used for the management protocol.
pub const DEFAULT_MANAGEMENT_PORT: u16 = 54;

pub use codec::{
    Envelope, LENGTH_PREFIX_BYTES, MAX_FRAME_PAYLOAD_BYTES, ProtocolCodec, Request, RequestCodec,
    Response, ResponseCodec,
};
pub use ed25519_dalek::{SigningKey, VerifyingKey};
pub use error::CodecError;
pub use message::{
    ApplicationMetrics, Capability, Command, ComponentMetrics, CustomMetric, DeploymentAccepted,
    DeploymentRejected, HelloRequest, HelloResponse, InstanceMetrics, InstanceStatus, MessageBody,
    MessageType, MetricsReport, MetricsRequest, Outcome, ProtocolError, ProtocolErrorCode,
    RejectionCode, RequestId, ResourceMetrics, StatusReport, TrafficMetrics,
};
pub use signature::{
    Artifact, DEPLOYMENT_SIGNING_DOMAIN, Deployment, DeploymentSignature, SignatureAlgorithm,
    SignatureError, SignedDeployment,
};
pub use version::ProtocolVersion;
