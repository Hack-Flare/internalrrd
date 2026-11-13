use std::io;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use protocol::{Artifact, Deployment};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Downloads, verifies, and extracts release artifacts.
#[derive(Debug)]
pub struct ReleaseManager {
    root: PathBuf,
    max_artifact_bytes: u64,
    client: reqwest::Client,
}

/// A release that has been verified and extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRelease {
    pub version: String,
    pub directory: PathBuf,
}

/// The deployment information stored alongside an extracted release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseMetadata {
    pub version: String,
    pub issued_at_unix: u64,
}

const METADATA_FILE: &str = ".internalrrd-release";

impl ReleaseManager {
    pub fn new(root: impl Into<PathBuf>, max_artifact_bytes: u64) -> Result<Self, ReleaseError> {
        if max_artifact_bytes == 0 {
            return Err(ReleaseError::ZeroLimit);
        }

        let root = root.into();
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map_err(ReleaseError::Io)?
                .join(root)
        };

        let client = reqwest::Client::builder()
            .build()
            .map_err(ReleaseError::Client)?;

        Ok(Self {
            root,
            max_artifact_bytes,
            client,
        })
    }

    /// Download, verify, and extract the artifact for a deployment.
    pub async fn install(&self, deployment: &Deployment) -> Result<InstalledRelease, ReleaseError> {
        validate_version(&deployment.version)?;
        let artifact = &deployment.artifact;
        if artifact.url.trim().is_empty() {
            return Err(ReleaseError::InvalidUrl(artifact.url.clone()));
        }
        if artifact.size_bytes > self.max_artifact_bytes {
            return Err(ReleaseError::TooLarge {
                size: artifact.size_bytes,
                limit: self.max_artifact_bytes,
            });
        }

        let downloads = self.root.join("downloads");
        fs::create_dir_all(&downloads)
            .await
            .map_err(ReleaseError::Io)?;
        let digest = hex::encode(artifact.sha256);
        let archive = downloads.join(format!("{digest}.tar.gz"));

        if !archive_is_valid(&archive, artifact).await {
            self.download(artifact, &archive).await?;
        }
        verify_file(&archive, artifact).await?;

        let releases = self.root.join("releases");
        fs::create_dir_all(&releases)
            .await
            .map_err(ReleaseError::Io)?;
        let name = format!(
            "{}-{}",
            sanitize_version(&deployment.version),
            &digest[..12]
        );
        let directory = releases.join(&name);
        if is_directory(&directory).await {
            write_metadata(&directory, deployment).await?;
            return Ok(InstalledRelease {
                version: deployment.version.clone(),
                directory,
            });
        }

        let temporary = self.root.join("tmp").join(&name);
        if temporary.exists() {
            fs::remove_dir_all(&temporary)
                .await
                .map_err(ReleaseError::Io)?;
        }
        fs::create_dir_all(&temporary)
            .await
            .map_err(ReleaseError::Io)?;

        let archive_for_extraction = archive.clone();
        let destination = temporary.clone();
        let extracted =
            tokio::task::spawn_blocking(move || extract(&archive_for_extraction, &destination))
                .await
                .map_err(ReleaseError::Task)?;

        if let Err(error) = extracted {
            let _ = fs::remove_dir_all(&temporary).await;
            return Err(error);
        }

        match fs::rename(&temporary, &directory).await {
            Ok(()) => {}
            Err(error) if is_already_exists(&error) => {
                let _ = fs::remove_dir_all(&temporary).await;
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&temporary).await;
                return Err(ReleaseError::Io(error));
            }
        }

        write_metadata(&directory, deployment).await?;

        Ok(InstalledRelease {
            version: deployment.version.clone(),
            directory,
        })
    }

    /// Read the deployment metadata stored with an extracted release.
    pub async fn metadata_of(&self, directory: &Path) -> Option<ReleaseMetadata> {
        let contents = fs::read_to_string(directory.join(METADATA_FILE))
            .await
            .ok()?;
        let mut lines = contents.lines();
        let version = lines.next()?.trim();
        let issued_at_unix = lines.next()?.trim().parse().ok()?;

        if version.is_empty() {
            return None;
        }

        Some(ReleaseMetadata {
            version: version.to_string(),
            issued_at_unix,
        })
    }

    /// Point the current release link at a release directory.
    #[cfg(unix)]
    pub async fn set_current(&self, release: &Path) -> Result<(), ReleaseError> {
        let link = self.root.join("current");
        let temporary = self.root.join("current.tmp");

        let _ = fs::remove_file(&temporary).await;
        fs::symlink(release, &temporary)
            .await
            .map_err(ReleaseError::Current)?;
        fs::rename(&temporary, &link)
            .await
            .map_err(ReleaseError::Current)?;
        Ok(())
    }

    #[cfg(not(unix))]
    pub async fn set_current(&self, release: &Path) -> Result<(), ReleaseError> {
        let link = self.root.join("current");
        fs::write(&link, release.to_string_lossy().as_bytes())
            .await
            .map_err(ReleaseError::Current)
    }

    /// Resolve the current release directory, if one exists.
    #[cfg(unix)]
    pub async fn current(&self) -> Option<PathBuf> {
        let link = self.root.join("current");
        let target = fs::read_link(&link).await.ok()?;
        if is_directory(&target).await {
            Some(target)
        } else {
            None
        }
    }

    #[cfg(not(unix))]
    pub async fn current(&self) -> Option<PathBuf> {
        let link = self.root.join("current");
        let contents = fs::read_to_string(&link).await.ok()?;
        let target = PathBuf::from(contents.trim());
        if is_directory(&target).await {
            Some(target)
        } else {
            None
        }
    }

    async fn download(&self, artifact: &Artifact, destination: &Path) -> Result<(), ReleaseError> {
        let source = ArtifactSource::parse(&artifact.url)?;
        let partial = destination.with_extension("part");
        let _ = fs::remove_file(&partial).await;

        let result = match source {
            ArtifactSource::File(path) => self.copy_file(&path, &partial).await,
            ArtifactSource::Http(url) => self.fetch_http(&url, &partial).await,
        };

        if let Err(error) = result {
            let _ = fs::remove_file(&partial).await;
            return Err(error);
        }

        fs::rename(&partial, destination)
            .await
            .map_err(ReleaseError::Io)
    }

    async fn copy_file(&self, source: &Path, destination: &Path) -> Result<(), ReleaseError> {
        let mut input =
            fs::File::open(source)
                .await
                .map_err(|source_error| ReleaseError::Open {
                    path: source.to_path_buf(),
                    source: source_error,
                })?;
        let mut output = fs::File::create(destination)
            .await
            .map_err(ReleaseError::Io)?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut total = 0u64;

        loop {
            let read = input.read(&mut buffer).await.map_err(ReleaseError::Io)?;
            if read == 0 {
                break;
            }
            total += read as u64;
            if total > self.max_artifact_bytes {
                return Err(ReleaseError::TooLarge {
                    size: total,
                    limit: self.max_artifact_bytes,
                });
            }
            output
                .write_all(&buffer[..read])
                .await
                .map_err(ReleaseError::Io)?;
        }

        output.flush().await.map_err(ReleaseError::Io)
    }

    async fn fetch_http(&self, url: &str, destination: &Path) -> Result<(), ReleaseError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(ReleaseError::Http)?;

        if !response.status().is_success() {
            return Err(ReleaseError::HttpStatus {
                url: url.to_string(),
                status: response.status().as_u16(),
            });
        }

        let mut output = fs::File::create(destination)
            .await
            .map_err(ReleaseError::Io)?;
        let mut stream = response.bytes_stream();
        let mut total = 0u64;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(ReleaseError::Http)?;
            total += chunk.len() as u64;
            if total > self.max_artifact_bytes {
                return Err(ReleaseError::TooLarge {
                    size: total,
                    limit: self.max_artifact_bytes,
                });
            }
            output.write_all(&chunk).await.map_err(ReleaseError::Io)?;
        }

        output.flush().await.map_err(ReleaseError::Io)
    }
}

enum ArtifactSource {
    File(PathBuf),
    Http(String),
}

impl ArtifactSource {
    fn parse(url: &str) -> Result<Self, ReleaseError> {
        if let Some(path) = url.strip_prefix("file://") {
            if path.is_empty() {
                return Err(ReleaseError::InvalidUrl(url.to_string()));
            }
            return Ok(Self::File(PathBuf::from(path)));
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            return Ok(Self::Http(url.to_string()));
        }
        Err(ReleaseError::InvalidUrl(url.to_string()))
    }
}

async fn write_metadata(directory: &Path, deployment: &Deployment) -> Result<(), ReleaseError> {
    let contents = format!("{}\n{}\n", deployment.version, deployment.issued_at_unix);
    fs::write(directory.join(METADATA_FILE), contents)
        .await
        .map_err(ReleaseError::Io)
}

fn validate_version(version: &str) -> Result<(), ReleaseError> {
    if version.trim().is_empty()
        || version.contains('/')
        || version.contains('\\')
        || version == "."
        || version == ".."
    {
        return Err(ReleaseError::InvalidVersion);
    }
    Ok(())
}

fn sanitize_version(version: &str) -> String {
    version
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

async fn is_directory(path: &Path) -> bool {
    fs::metadata(path)
        .await
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn is_already_exists(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::AlreadyExists
}

async fn archive_is_valid(path: &Path, artifact: &Artifact) -> bool {
    verify_file(path, artifact).await.is_ok()
}

async fn verify_file(path: &Path, artifact: &Artifact) -> Result<(), ReleaseError> {
    let path = path.to_path_buf();
    let expected = artifact.sha256;
    let expected_size = artifact.size_bytes;

    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path).map_err(|source| ReleaseError::Open {
            path: path.clone(),
            source,
        })?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut size = 0u64;

        loop {
            let read = file.read(&mut buffer).map_err(ReleaseError::Io)?;
            if read == 0 {
                break;
            }
            size += read as u64;
            hasher.update(&buffer[..read]);
        }

        if size != expected_size {
            return Err(ReleaseError::SizeMismatch {
                expected: expected_size,
                actual: size,
            });
        }

        let digest = hasher.finalize();
        if digest.as_slice() != expected {
            return Err(ReleaseError::ChecksumMismatch);
        }

        Ok(())
    })
    .await
    .map_err(ReleaseError::Task)?
}

fn extract(archive: &Path, destination: &Path) -> Result<(), ReleaseError> {
    let file = std::fs::File::open(archive).map_err(|source| ReleaseError::Open {
        path: archive.to_path_buf(),
        source,
    })?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().map_err(ReleaseError::Archive)? {
        let mut entry = entry.map_err(ReleaseError::Archive)?;
        let path = entry.path().map_err(ReleaseError::Archive)?.into_owned();

        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(ReleaseError::UnsafePath(path));
        }

        let unpacked = entry
            .unpack_in(destination)
            .map_err(ReleaseError::Archive)?;
        if !unpacked {
            return Err(ReleaseError::UnsafePath(path));
        }
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("maximum artifact size must be greater than zero")]
    ZeroLimit,
    #[error("release version must not be empty or contain path separators")]
    InvalidVersion,
    #[error("artifact URL '{0}' is not supported")]
    InvalidUrl(String),
    #[error("artifact exceeds the configured limit of {limit} bytes (reported size {size})")]
    TooLarge { size: u64, limit: u64 },
    #[error("failed to create the artifact HTTP client: {0}")]
    Client(#[source] reqwest::Error),
    #[error("artifact request failed: {0}")]
    Http(#[source] reqwest::Error),
    #[error("artifact request to {url} returned HTTP status {status}")]
    HttpStatus { url: String, status: u16 },
    #[error("failed to open {path}: {source}")]
    Open { path: PathBuf, source: io::Error },
    #[error("failed to read or write release files: {0}")]
    Io(#[from] io::Error),
    #[error("artifact size mismatch: expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("artifact checksum does not match the deployment")]
    ChecksumMismatch,
    #[error("artifact archive is invalid: {0}")]
    Archive(#[source] io::Error),
    #[error("artifact contains an unsafe path: {0}")]
    UnsafePath(PathBuf),
    #[error("release extraction task failed: {0}")]
    Task(#[source] tokio::task::JoinError),
    #[error("failed to update the current release link: {0}")]
    Current(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use protocol::Artifact;

    use super::*;

    fn create_artifact(directory: &Path) -> Result<(PathBuf, [u8; 32], u64), Box<dyn Error>> {
        let archive_path = directory.join("example.tar.gz");
        let file = std::fs::File::create(&archive_path)?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);

        let contents = b"#!/bin/sh\nexit 0\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, "bin/example", &contents[..])?;
        builder.finish()?;

        let encoder = builder.into_inner()?;
        encoder.finish()?;

        let bytes = std::fs::read(&archive_path)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        Ok((archive_path, digest, bytes.len() as u64))
    }

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
        let (archive, digest, size) = create_artifact(temporary.path())?;
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
        let (archive, digest, size) = create_artifact(temporary.path())?;
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
        let (archive, digest, size) = create_artifact(temporary.path())?;
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
        let (archive, digest, size) = create_artifact(temporary.path())?;
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
}
