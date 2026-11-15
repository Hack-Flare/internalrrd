use std::error::Error;

use internalrrd::config::{TrustConfig, TrustedKeyConfig};
use internalrrd::trust::{TrustError, TrustStore};
use protocol::{Artifact, Deployment, SignedDeployment, SigningKey};

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
