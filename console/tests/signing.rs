use std::error::Error;

use console::config::SigningConfig;
use console::signing::{Signer, SigningError};
use protocol::{Artifact, Deployment, SigningKey};

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

fn signing_config(key: &SigningKey) -> SigningConfig {
    SigningConfig {
        key_id: "release-2026".to_string(),
        private_key: hex::encode(key.to_bytes()),
    }
}

#[test]
fn signs_a_verifiable_deployment() -> Result<(), Box<dyn Error>> {
    let key = SigningKey::from_bytes(&[7; 32]);
    let signer = Signer::from_config(&signing_config(&key))?;

    let signed = signer.sign(deployment())?;

    assert_eq!(signed.key_id(), "release-2026");
    signed.verify(&key.verifying_key())?;
    Ok(())
}

#[test]
fn rejects_signatures_from_another_key() -> Result<(), Box<dyn Error>> {
    let key = SigningKey::from_bytes(&[7; 32]);
    let other_key = SigningKey::from_bytes(&[9; 32]);
    let signer = Signer::from_config(&signing_config(&key))?;

    let signed = signer.sign(deployment())?;

    assert!(signed.verify(&other_key.verifying_key()).is_err());
    Ok(())
}

#[test]
fn rejects_empty_key_identifiers() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let mut config = signing_config(&key);
    config.key_id = "  ".to_string();

    assert!(matches!(
        Signer::from_config(&config),
        Err(SigningError::EmptyKeyId)
    ));
}

#[test]
fn rejects_invalid_key_hex() {
    let config = SigningConfig {
        key_id: "release-2026".to_string(),
        private_key: "not-hex".to_string(),
    };

    assert!(matches!(
        Signer::from_config(&config),
        Err(SigningError::InvalidHex(_))
    ));
}

#[test]
fn rejects_keys_with_the_wrong_length() {
    let config = SigningConfig {
        key_id: "release-2026".to_string(),
        private_key: "00".repeat(31),
    };

    assert!(matches!(
        Signer::from_config(&config),
        Err(SigningError::InvalidLength(31))
    ));
}
