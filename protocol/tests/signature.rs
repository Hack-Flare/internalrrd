use std::error::Error;

use protocol::{
    Artifact, Deployment, SignatureAlgorithm, SignatureError, SignedDeployment, SigningKey,
};

fn deployment() -> Deployment {
    Deployment {
        application: "example".to_string(),
        version: "1.2.3".to_string(),
        artifact: Artifact {
            url: "https://artifacts.example/example-1.2.3".to_string(),
            sha256: [0xab; 32],
            size_bytes: 4096,
        },
        issued_at_unix: 1_700_000_000,
    }
}

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

#[test]
fn signs_and_verifies_a_deployment() -> Result<(), Box<dyn Error>> {
    let key = signing_key();
    let signed = SignedDeployment::sign(deployment(), "release-2026", &key)?;

    assert_eq!(signed.key_id(), "release-2026");
    signed.verify(&key.verifying_key())?;
    Ok(())
}

#[test]
fn rejects_a_tampered_deployment() -> Result<(), Box<dyn Error>> {
    let key = signing_key();
    let mut signed = SignedDeployment::sign(deployment(), "release-2026", &key)?;
    signed.deployment.version = "9.9.9".to_string();

    assert!(matches!(
        signed.verify(&key.verifying_key()),
        Err(SignatureError::Invalid)
    ));
    Ok(())
}

#[test]
fn rejects_a_signature_from_another_key() -> Result<(), Box<dyn Error>> {
    let key = signing_key();
    let other_key = SigningKey::from_bytes(&[9; 32]);
    let signed = SignedDeployment::sign(deployment(), "release-2026", &key)?;

    assert!(matches!(
        signed.verify(&other_key.verifying_key()),
        Err(SignatureError::Invalid)
    ));
    Ok(())
}

#[test]
fn rejects_unsupported_algorithms() -> Result<(), Box<dyn Error>> {
    let key = signing_key();
    let mut signed = SignedDeployment::sign(deployment(), "release-2026", &key)?;
    signed.signature.algorithm = SignatureAlgorithm::new(0x42);

    assert!(matches!(
        signed.verify(&key.verifying_key()),
        Err(SignatureError::UnsupportedAlgorithm(_))
    ));
    Ok(())
}

#[test]
fn rejects_invalid_signature_lengths() -> Result<(), Box<dyn Error>> {
    let key = signing_key();
    let mut signed = SignedDeployment::sign(deployment(), "release-2026", &key)?;
    signed.signature.bytes.truncate(32);

    assert!(matches!(
        signed.verify(&key.verifying_key()),
        Err(SignatureError::InvalidLength(32))
    ));
    Ok(())
}

#[test]
fn rejects_empty_key_identifiers() {
    let key = signing_key();

    assert!(matches!(
        SignedDeployment::sign(deployment(), "  ", &key),
        Err(SignatureError::EmptyKeyId)
    ));
}

#[test]
fn signing_bytes_change_with_the_deployment() -> Result<(), Box<dyn Error>> {
    let mut other = deployment();
    other.version = "1.2.4".to_string();

    assert_ne!(deployment().signing_bytes()?, other.signing_bytes()?);
    Ok(())
}
