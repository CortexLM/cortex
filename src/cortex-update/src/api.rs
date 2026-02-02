//! Cortex Foundation Software API client.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use cortex_engine::create_client_builder;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::SOFTWARE_URL;
use crate::config::ReleaseChannel;
use crate::error::{UpdateError, UpdateResult};

/// Asset information for a specific platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    /// Download URL for this asset
    pub url: String,
    /// SHA256 checksum of the file
    pub sha256: String,
    /// File size in bytes
    pub size: u64,
}

/// Release information from the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    /// Version string (semver)
    pub version: String,
    /// Release channel
    pub channel: ReleaseChannel,
    /// Release timestamp
    pub released_at: DateTime<Utc>,
    /// Minimum supported version for upgrade
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,
    /// URL to full changelog
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changelog_url: Option<String>,
    /// Brief release notes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
    /// Assets by platform key (e.g., "linux-x86_64", "darwin-aarch64", "windows-x86_64")
    pub assets: HashMap<String, ReleaseAsset>,
    /// Signature URLs by platform key
    #[serde(default)]
    pub signatures: HashMap<String, String>,
}

impl ReleaseInfo {
    /// Get the asset for the current platform.
    pub fn asset_for_current_platform(&self) -> Option<&ReleaseAsset> {
        let key = platform_key();
        self.assets.get(&key)
    }

    /// Get the signature URL for the current platform.
    pub fn signature_for_current_platform(&self) -> Option<&String> {
        let key = platform_key();
        self.signatures.get(&key)
    }
}

/// Changelog entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogEntry {
    pub version: String,
    pub released_at: DateTime<Utc>,
    pub title: String,
    pub changes: Vec<String>,
}

/// Client for the Cortex Foundation Software Distribution API.
#[derive(Clone)]
pub struct CortexSoftwareClient {
    client: Client,
    base_url: String,
}

impl CortexSoftwareClient {
    /// Get the base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl CortexSoftwareClient {
    /// Create a new client with the default URL.
    pub fn new() -> Self {
        Self::with_url(SOFTWARE_URL.to_string())
    }

    /// Create a new client with a custom URL.
    pub fn with_url(base_url: String) -> Self {
        let client = create_client_builder()
            .build()
            .unwrap_or_else(|_| Client::new());

        Self { client, base_url }
    }

    /// Get the latest release for a channel.
    pub async fn get_latest(&self, channel: ReleaseChannel) -> UpdateResult<ReleaseInfo> {
        let url = format!(
            "{}/v1/releases/latest?channel={}",
            self.base_url,
            channel.as_str()
        );

        let response =
            self.client
                .get(&url)
                .send()
                .await
                .map_err(|e| UpdateError::ConnectionFailed {
                    message: e.to_string(),
                })?;

        let status = response.status();
        if status.as_u16() == 404 {
            return Err(UpdateError::ServerError {
                status: 404,
                message: "No releases available yet. The release server has not published any releases. \
                         Please check https://github.com/CortexLM/cortex/releases for manual download or try again later."
                    .to_string(),
            });
        }

        if !status.is_success() {
            let status_code = status.as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(UpdateError::ServerError {
                status: status_code,
                message,
            });
        }

        let info: ReleaseInfo = response.json().await?;
        Ok(info)
    }

    /// Get a specific release by version.
    pub async fn get_release(&self, version: &str) -> UpdateResult<ReleaseInfo> {
        let url = format!("{}/v1/releases/{}", self.base_url, version);

        let response =
            self.client
                .get(&url)
                .send()
                .await
                .map_err(|e| UpdateError::ConnectionFailed {
                    message: e.to_string(),
                })?;

        if response.status().as_u16() == 404 {
            return Err(UpdateError::VersionNotFound {
                version: version.to_string(),
            });
        }

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(UpdateError::ServerError { status, message });
        }

        let info: ReleaseInfo = response.json().await?;
        Ok(info)
    }

    /// Get changelog entries since a version.
    pub async fn get_changelog(&self, since: &str) -> UpdateResult<Vec<ChangelogEntry>> {
        let url = format!("{}/v1/changelog?since={}", self.base_url, since);

        let response =
            self.client
                .get(&url)
                .send()
                .await
                .map_err(|e| UpdateError::ConnectionFailed {
                    message: e.to_string(),
                })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(UpdateError::ServerError { status, message });
        }

        let entries: Vec<ChangelogEntry> = response.json().await?;
        Ok(entries)
    }

    /// Download an asset to a destination path with progress reporting.
    pub async fn download<F>(
        &self,
        asset: &ReleaseAsset,
        dest: &Path,
        mut on_progress: F,
    ) -> UpdateResult<()>
    where
        F: FnMut(u64, u64), // (downloaded, total)
    {
        let response =
            self.client
                .get(&asset.url)
                .send()
                .await
                .map_err(|e| UpdateError::DownloadFailed {
                    message: e.to_string(),
                })?;

        if !response.status().is_success() {
            return Err(UpdateError::DownloadFailed {
                message: format!("HTTP {}", response.status()),
            });
        }

        let total_size = asset.size;
        let mut downloaded: u64 = 0;

        let mut file = tokio::fs::File::create(dest).await?;
        let mut stream = response.bytes_stream();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| UpdateError::DownloadFailed {
                message: e.to_string(),
            })?;

            file.write_all(&chunk).await?;

            downloaded += chunk.len() as u64;
            on_progress(downloaded, total_size);
        }

        file.flush().await?;

        Ok(())
    }

    /// Download a signature file.
    pub async fn download_signature(&self, url: &str) -> UpdateResult<Vec<u8>> {
        let response =
            self.client
                .get(url)
                .send()
                .await
                .map_err(|e| UpdateError::DownloadFailed {
                    message: e.to_string(),
                })?;

        if !response.status().is_success() {
            return Err(UpdateError::DownloadFailed {
                message: format!("Failed to download signature: HTTP {}", response.status()),
            });
        }

        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }
}

impl Default for CortexSoftwareClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the platform key for the current system.
pub fn platform_key() -> String {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        "unknown"
    };

    format!("{}-{}", os, arch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_key() {
        let key = platform_key();
        assert!(!key.is_empty());
        assert!(key.contains('-'));
    }
}
