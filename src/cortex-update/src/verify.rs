//! SHA256 verification for downloaded files.

use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::AsyncReadExt;

use crate::error::{UpdateError, UpdateResult};

/// Verify SHA256 checksum of a file.
pub async fn verify_sha256(path: &Path, expected: &str) -> UpdateResult<()> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 8192];

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let result = hasher.finalize();
    let actual = hex::encode(result);

    // Normalize expected (remove any whitespace, lowercase)
    let expected = expected.trim().to_lowercase();

    if actual != expected {
        return Err(UpdateError::ChecksumMismatch { expected, actual });
    }

    Ok(())
}

/// Verify SHA256 checksum synchronously (for smaller files).
pub fn verify_sha256_sync(path: &Path, expected: &str) -> UpdateResult<()> {
    let content = std::fs::read(path)?;
    let result = Sha256::digest(&content);
    let actual = hex::encode(result);

    let expected = expected.trim().to_lowercase();

    if actual != expected {
        return Err(UpdateError::ChecksumMismatch { expected, actual });
    }

    Ok(())
}

/// Calculate SHA256 hash of a file.
pub async fn calculate_sha256(path: &Path) -> UpdateResult<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 8192];

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let result = hasher.finalize();
    Ok(hex::encode(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_verify_sha256() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "hello world").unwrap();

        // SHA256 of "hello world"
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

        verify_sha256(file.path(), expected).await.unwrap();
    }

    #[tokio::test]
    async fn test_verify_sha256_mismatch() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "hello world").unwrap();

        let wrong = "0000000000000000000000000000000000000000000000000000000000000000";

        let result = verify_sha256(file.path(), wrong).await;
        assert!(matches!(result, Err(UpdateError::ChecksumMismatch { .. })));
    }

    #[tokio::test]
    async fn test_calculate_sha256() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "hello world").unwrap();

        let hash = calculate_sha256(file.path()).await.unwrap();
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
