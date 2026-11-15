use std::error::Error;
use std::path::Path;

use internalrrd::release::{ReleaseError, ReleaseManager, ReleaseMetadata};
use protocol::{Artifact, Deployment};

mod common;

use common::create_artifact;

fn deployment(archive: &Path, digest: [u8; 32], size: u64) -> Deployment {
    Deployment {
        application: "example".to_string(),
        version: "1.2.3".to_string(),
        artifact: Artifact {
            url: format!("file://{}", archive.display()),
            sha256: digest,
            size_bytes: size,
        },
        issued_at_unix: 1_700_000_000,
    }
}

#[tokio::test]
async fn installs_and_extracts_a_release() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let (archive, digest, size) =
        create_artifact(temporary.path(), "bin/example", b"#!/bin/sh\nexit 0\n")?;
    let manager = ReleaseManager::new(temporary.path().join("releases"), 1024 * 1024)?;

    let release = manager.install(&deployment(&archive, digest, size)).await?;

    assert_eq!(release.version, "1.2.3");
    assert!(release.directory.join("bin/example").is_file());
    let name = release
        .directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("release directory has no name")?;
    assert!(name.starts_with("1.2.3-"));
    assert_eq!(name.len(), "1.2.3-".len() + 12);
    assert_eq!(
        manager.metadata_of(&release.directory).await,
        Some(ReleaseMetadata {
            version: "1.2.3".to_string(),
            issued_at_unix: 1_700_000_000,
        })
    );
    Ok(())
}

#[tokio::test]
async fn rejects_checksum_mismatches() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let (archive, digest, size) =
        create_artifact(temporary.path(), "bin/example", b"#!/bin/sh\nexit 0\n")?;
    let manager = ReleaseManager::new(temporary.path().join("releases"), 1024 * 1024)?;

    let mut deployment = deployment(&archive, digest, size);
    deployment.artifact.sha256[0] ^= 0xff;

    assert!(matches!(
        manager.install(&deployment).await,
        Err(ReleaseError::ChecksumMismatch)
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_artifacts_above_the_limit() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let (archive, digest, size) =
        create_artifact(temporary.path(), "bin/example", b"#!/bin/sh\nexit 0\n")?;
    let manager = ReleaseManager::new(temporary.path().join("releases"), 8)?;

    assert!(matches!(
        manager.install(&deployment(&archive, digest, size)).await,
        Err(ReleaseError::TooLarge { .. })
    ));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn tracks_the_current_release() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let (archive, digest, size) =
        create_artifact(temporary.path(), "bin/example", b"#!/bin/sh\nexit 0\n")?;
    let manager = ReleaseManager::new(temporary.path().join("releases"), 1024 * 1024)?;

    let release = manager.install(&deployment(&archive, digest, size)).await?;
    assert!(manager.current().await.is_none());

    manager.set_current(&release.directory).await?;
    assert_eq!(
        manager.current().await.as_deref(),
        Some(release.directory.as_path())
    );
    Ok(())
}
