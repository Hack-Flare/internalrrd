use protocol::{Deployment, SignedDeployment, SigningKey};

use crate::config::SigningConfig;

/// Signs deployments with the configured Ed25519 key.
pub struct Signer {
    key_id: String,
    key: SigningKey,
}

impl Signer {
    /// Build a signer from the signing section of a console configuration.
    pub fn from_config(config: &SigningConfig) -> Result<Self, SigningError> {
        if config.key_id.trim().is_empty() {
            return Err(SigningError::EmptyKeyId);
        }

        let bytes = hex::decode(config.private_key.trim()).map_err(SigningError::InvalidHex)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|bytes: Vec<u8>| SigningError::InvalidLength(bytes.len()))?;

        Ok(Self {
            key_id: config.key_id.clone(),
            key: SigningKey::from_bytes(&bytes),
        })
    }

    /// Sign one deployment with the configured key.
    pub fn sign(&self, deployment: Deployment) -> Result<SignedDeployment, SigningError> {
        SignedDeployment::sign(deployment, &self.key_id, &self.key).map_err(SigningError::Sign)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    #[error("signing key identifier must not be empty")]
    EmptyKeyId,
    #[error("signing key is not valid hexadecimal: {0}")]
    InvalidHex(#[source] hex::FromHexError),
    #[error("signing key must be 32 bytes, got {0}")]
    InvalidLength(usize),
    #[error("failed to sign the deployment: {0}")]
    Sign(#[source] protocol::SignatureError),
}
