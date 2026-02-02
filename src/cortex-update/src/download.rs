//! Download functionality with progress tracking.

use std::path::{Path, PathBuf};

use crate::api::{CortexSoftwareClient, ReleaseAsset};
use crate::error::{UpdateError, UpdateResult};

/// Progress information during download.
#[derive(Debug, Clone, Copy)]
pub struct DownloadProgress {
    /// Bytes downloaded so far
    pub downloaded: u64,
    /// Total bytes to download
    pub total: u64,
}

impl DownloadProgress {
    /// Get download progress as a percentage (0-100).
    pub fn percentage(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.downloaded as f32 / self.total as f32) * 100.0
    }

    /// Get human-readable downloaded size.
    pub fn downloaded_human(&self) -> String {
        format_bytes(self.downloaded)
    }

    /// Get human-readable total size.
    pub fn total_human(&self) -> String {
        format_bytes(self.total)
    }
}

/// Format bytes as human-readable string.
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Download manager for update assets.
pub struct Downloader {
    client: CortexSoftwareClient,
    temp_dir: PathBuf,
}

impl Downloader {
    /// Create a new downloader with a secure temporary directory.
    ///
    /// Uses a randomly-named subdirectory to prevent symlink attacks and
    /// predictable file name exploits.
    pub fn new(client: CortexSoftwareClient) -> UpdateResult<Self> {
        // Use a random suffix to prevent predictable temp directory names
        // This mitigates symlink attacks where an attacker pre-creates
        // files with expected names
        let random_suffix: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ std::process::id() as u64;

        let temp_dir = std::env::temp_dir().join(format!("cortex-update-{:x}", random_suffix));

        // Create with restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700) // Owner-only access
                .recursive(true)
                .create(&temp_dir)?;
        }

        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(&temp_dir)?;
        }

        Ok(Self { client, temp_dir })
    }

    /// Download an asset with progress callback.
    pub async fn download<F>(
        &self,
        asset: &ReleaseAsset,
        version: &str,
        mut on_progress: F,
    ) -> UpdateResult<PathBuf>
    where
        F: FnMut(DownloadProgress),
    {
        // Determine filename from URL
        let filename = asset.url.rsplit('/').next().unwrap_or("cortex-update.bin");

        let dest = self.temp_dir.join(format!("{}_{}", version, filename));

        // Remove existing file if present
        if dest.exists() {
            std::fs::remove_file(&dest)?;
        }

        // Download with progress
        self.client
            .download(asset, &dest, |downloaded, total| {
                on_progress(DownloadProgress { downloaded, total });
            })
            .await?;

        Ok(dest)
    }

    /// Download a signature file.
    pub async fn download_signature(&self, url: &str, version: &str) -> UpdateResult<PathBuf> {
        let filename = format!("{}.sig", version);
        let dest = self.temp_dir.join(filename);

        let bytes = self.client.download_signature(url).await?;
        std::fs::write(&dest, bytes)?;

        Ok(dest)
    }

    /// Get the temp directory path.
    pub fn temp_dir(&self) -> &Path {
        &self.temp_dir
    }

    /// Clean up temp files.
    pub fn cleanup(&self) -> UpdateResult<()> {
        if self.temp_dir.exists() {
            std::fs::remove_dir_all(&self.temp_dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1048576), "1.0 MB");
        assert_eq!(format_bytes(1073741824), "1.0 GB");
    }

    #[test]
    fn test_download_progress() {
        let progress = DownloadProgress {
            downloaded: 50_000_000,
            total: 100_000_000,
        };
        assert!((progress.percentage() - 50.0).abs() < 0.01);
        assert_eq!(progress.downloaded_human(), "47.7 MB");
        assert_eq!(progress.total_human(), "95.4 MB");
    }
}
