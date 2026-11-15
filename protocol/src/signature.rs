use std::fmt;

use ed25519_dalek::{Signature as Ed25519Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Domain separator mixed into every deployment signature.
pub const DEPLOYMENT_SIGNING_DOMAIN: &[u8] = b"internalrrd/deployment/v1";

/// Identifies the algorithm used for a detached signature.
///
/// Values are explicit and stable on the wire.
/// Unknown values are preserved so that peers can negotiate newer algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SignatureAlgorithm(u8);

impl SignatureAlgorithm {
    pub const ED25519: Self = Self(1);

    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u8 {
        self.0
    }
}

impl fmt::Display for SignatureAlgorithm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::ED25519 {
            formatter.write_str("ed25519")
        } else {
            write!(formatter, "0x{:02x}", self.0)
        }
    }
}

/// A release artifact that a server should retrieve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub url: String,
    pub sha256: [u8; 32],
    pub size_bytes: u64,
}

/// The content covered by a deployment signature.
///
/// Field order is part of the signed encoding.
/// Do not reorder fields without changing the signing domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    pub application: String,
    pub version: String,
    pub artifact: Artifact,
    pub issued_at_unix: u64,
}

impl Deployment {
    /// The canonical byte string that is signed and verified.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, SignatureError> {
        let payload = postcard::to_allocvec(self).map_err(SignatureError::Encode)?;
        let mut bytes = Vec::with_capacity(DEPLOYMENT_SIGNING_DOMAIN.len() + payload.len());
        bytes.extend_from_slice(DEPLOYMENT_SIGNING_DOMAIN);
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentSignature {
    pub algorithm: SignatureAlgorithm,
    pub key_id: String,
    pub bytes: Vec<u8>,
}

/// A deployment notification together with its detached signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDeployment {
    pub deployment: Deployment,
    pub signature: DeploymentSignature,
}

impl SignedDeployment {
    /// Sign a deployment for distribution to servers.
    pub fn sign(
        deployment: Deployment,
        key_id: impl Into<String>,
        signing_key: &SigningKey,
    ) -> Result<Self, SignatureError> {
        let key_id = key_id.into();
        if key_id.trim().is_empty() {
            return Err(SignatureError::EmptyKeyId);
        }

        let signing_bytes = deployment.signing_bytes()?;
        let signature = signing_key.sign(&signing_bytes);

        Ok(Self {
            deployment,
            signature: DeploymentSignature {
                algorithm: SignatureAlgorithm::ED25519,
                key_id,
                bytes: signature.to_bytes().to_vec(),
            },
        })
    }

    /// The identifier of the key that produced this signature.
    pub fn key_id(&self) -> &str {
        &self.signature.key_id
    }

    /// Verify the signature against a trusted public key.
    pub fn verify(&self, verifying_key: &VerifyingKey) -> Result<(), SignatureError> {
        if self.signature.algorithm != SignatureAlgorithm::ED25519 {
            return Err(SignatureError::UnsupportedAlgorithm(
                self.signature.algorithm,
            ));
        }

        let length = self.signature.bytes.len();
        let signature_bytes: [u8; 64] = self
            .signature
            .bytes
            .as_slice()
            .try_into()
            .map_err(|_| SignatureError::InvalidLength(length))?;
        let signature = Ed25519Signature::from_bytes(&signature_bytes);

        let signing_bytes = self.deployment.signing_bytes()?;
        verifying_key
            .verify(&signing_bytes, &signature)
            .map_err(|_| SignatureError::Invalid)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    #[error("deployment could not be encoded for signing: {0}")]
    Encode(#[source] postcard::Error),
    #[error("signature key identifier must not be empty")]
    EmptyKeyId,
    #[error("unsupported signature algorithm {0}")]
    UnsupportedAlgorithm(SignatureAlgorithm),
    #[error("invalid Ed25519 signature length {0}, expected 64 bytes")]
    InvalidLength(usize),
    #[error("deployment signature is invalid")]
    Invalid,
}
