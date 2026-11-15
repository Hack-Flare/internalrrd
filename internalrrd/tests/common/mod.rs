use std::error::Error;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};

/// Build a gzipped tar archive and return its path, SHA-256 digest, and size.
pub fn create_artifact(
    directory: &Path,
    entry: &str,
    contents: &[u8],
) -> Result<(PathBuf, [u8; 32], u64), Box<dyn Error>> {
    let archive_path = directory.join("example.tar.gz");
    let file = std::fs::File::create(&archive_path)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);

    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append_data(&mut header, entry, contents)?;
    builder.finish()?;

    let encoder = builder.into_inner()?;
    encoder.finish()?;

    let bytes = std::fs::read(&archive_path)?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    Ok((archive_path, digest, bytes.len() as u64))
}
