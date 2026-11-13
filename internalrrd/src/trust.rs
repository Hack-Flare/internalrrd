use std::collections::BTreeMap;

use protocol::{SignatureAlgorithm, SignedDeployment, VerifyingKey};

use crate::config::{TrustConfig, TrustedKeyConfig};

/// A set of public keys that are trusted to sign deployments.
#[derive(Debug, Clone)]
pub struct TrustStore {
    keys: BTreeMap<String, VerifyingKey>,
}

impl TrustStore {
    /// Build a trust store from configuration.
    pub fn from_config(config: &TrustConfig) -> Result<Self, TrustError> {
        let mut keys = BTreeMap::new();

        for key in &config.keys {
            let verifying_key = parse_key(key)?;
            verify_duplicate(&keys, &key.id)?;
            keys.insert(key.id.clone(), verifying_key);
        }

        Ok(Self { keys })
    }

    /// The number of trusted keys.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Verify a signed deployment against the trusted key it references.
    pub fn verify(&self, signed: &SignedDeployment) -> Result<(), TrustError> {
        if signed.signature.algorithm != SignatureAlgorithm::ED25519 {
            return Err(TrustError::UnsupportedAlgorithm(signed.signature.algorithm));
        }

        let key_id = signed.signature.key_id.as_str();
        let key = self
            .keys
            .get(key_id)
            .ok_or_else(|| TrustError::UnknownKey(key_id.to_string()))?;

        signed.verify(key).map_err(TrustError::Signature)
    }
}

fn parse_key(config: &TrustedKeyConfig) -> Result<VerifyingKey, TrustError> {
    if config.algorithm != SignatureAlgorithm::ED25519.raw() {
        return Err(TrustError::UnsupportedAlgorithm(SignatureAlgorithm::new(
            config.algorithm,
        )));
    }

    let bytes = hex::decode(config.public_key.trim()).map_err(|error| TrustError::InvalidKey {
        key_id: config.id.clone(),
        message: error.to_string(),
    })?;
    let bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| TrustError::InvalidKey {
            key_id: config.id.clone(),
            message: format!("expected 32 bytes, got {}", bytes.len()),
        })?;

    VerifyingKey::from_bytes(&bytes).map_err(|error| TrustError::InvalidKey {
        key_id: config.id.clone(),
        message: error.to_string(),
    })
}

fn verify_duplicate(keys: &BTreeMap<String, VerifyingKey>, key_id: &str) -> Result<(), TrustError> {
    if keys.contains_key(key_id) {
        return Err(TrustError::DuplicateKey(key_id.to_string()));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    #[error("trusted key '{key_id}' is invalid: {message}")]
    InvalidKey { key_id: String, message: String },
    #[error("trusted key '{0}' is defined more than once")]
    DuplicateKey(String),
    #[error("deployment signature algorithm {0} is not supported")]
    UnsupportedAlgorithm(SignatureAlgorithm),
    #[error("deployment was signed with unknown key '{0}'")]
    UnknownKey(String),
    #[error("deployment signature could not be verified: {0}")]
    Signature(#[source] protocol::SignatureError),
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use protocol::{Artifact, Deployment, SigningKey};

    use super::*;

    fn deployment() -> Deployment {
        Deployment {
            application: "example".to_string(),
            version: "1.2.3".to_string(),
            artifact: Artifact {
                url: "file:///tmp/example.tar.gz".to_string(),
                sha256: [0xab; 32],
                size_bytes: 4096,
            },
            issued_at_unix: 1_700_000_000,
        }
    }

    fn config_for(signing_key: &SigningKey) -> TrustConfig {
        TrustConfig {
            keys: vec![TrustedKeyConfig {
                id: "release-2026".to_string(),
                algorithm: 1,
                public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            }],
        }
    }

    #[test]
    fn verifies_a_trusted_deployment() -> Result<(), Box<dyn Error>> {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let store = TrustStore::from_config(&config_for(&signing_key))?;
        let signed = SignedDeployment::sign(deployment(), "release-2026", &signing_key)?;

        store.verify(&signed)?;
        assert_eq!(store.len(), 1);
        Ok(())
    }

    #[test]
    fn rejects_unknown_keys() -> Result<(), Box<dyn Error>> {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let store = TrustStore::from_config(&config_for(&signing_key))?;
        let signed = SignedDeployment::sign(deployment(), "other-key", &signing_key)?;

        assert!(matches!(
            store.verify(&signed),
            Err(TrustError::UnknownKey(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_untrusted_signatures() -> Result<(), Box<dyn Error>> {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let other_key = SigningKey::from_bytes(&[9; 32]);
        let store = TrustStore::from_config(&config_for(&signing_key))?;
        let signed = SignedDeployment::sign(deployment(), "release-2026", &other_key)?;

        assert!(matches!(
            store.verify(&signed),
            Err(TrustError::Signature(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_invalid_public_keys() {
        let config = TrustConfig {
            keys: vec![TrustedKeyConfig {
                id: "release".to_string(),
                algorithm: 1,
                public_key: "00".repeat(31),
            }],
        };

        assert!(matches!(
            TrustStore::from_config(&config),
            Err(TrustError::InvalidKey { .. })
        ));
    }
}
