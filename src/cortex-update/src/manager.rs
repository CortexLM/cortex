//! Update manager - main API for update operations.

use std::path::PathBuf;

use crate::CURRENT_VERSION;
use crate::api::{CortexSoftwareClient, ReleaseAsset, ReleaseInfo};
use crate::config::{ReleaseChannel, UpdateConfig};
use crate::download::{DownloadProgress, Downloader};
use crate::error::{UpdateError, UpdateResult};
use crate::install::{DownloadedUpdate, Installer};
use crate::method::InstallMethod;
use crate::verify::verify_sha256;
use crate::version::{VersionCache, VersionComparison, compare_versions};

/// Information about an available update.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    /// Current installed version
    pub current_version: String,
    /// Latest available version
    pub latest_version: String,
    /// Release channel
    pub channel: ReleaseChannel,
    /// URL to changelog
    pub changelog_url: Option<String>,
    /// Brief release notes
    pub release_notes: Option<String>,
    /// Asset for current platform
    pub asset: ReleaseAsset,
    /// Detected installation method
    pub install_method: InstallMethod,
}

/// Outcome of an update operation.
#[derive(Debug)]
pub enum UpdateOutcome {
    /// Successfully updated
    Updated { from: String, to: String },
    /// Already on latest version
    AlreadyLatest,
    /// Update skipped by user
    Skipped,
    /// Requires restart to complete
    RequiresRestart,
}

/// Manager for update operations.
pub struct UpdateManager {
    client: CortexSoftwareClient,
    config: UpdateConfig,
    install_method: InstallMethod,
}

impl UpdateManager {
    /// Create a new update manager with default config.
    pub fn new() -> UpdateResult<Self> {
        let config = UpdateConfig::load();
        Self::with_config(config)
    }

    /// Create with a specific config.
    pub fn with_config(config: UpdateConfig) -> UpdateResult<Self> {
        let client = if let Some(url) = &config.custom_url {
            CortexSoftwareClient::with_url(url.clone())
        } else {
            CortexSoftwareClient::new()
        };

        let install_method = InstallMethod::detect();

        Ok(Self {
            client,
            config,
            install_method,
        })
    }

    /// Get the current configuration.
    pub fn config(&self) -> &UpdateConfig {
        &self.config
    }

    /// Get the detected installation method.
    pub fn install_method(&self) -> InstallMethod {
        self.install_method
    }

    /// Check if an update is available (uses cache if valid).
    pub async fn check_update(&self) -> UpdateResult<Option<UpdateInfo>> {
        // Try to use cache first
        if let Some(cache) = VersionCache::load() {
            if cache.is_valid(&self.config) {
                if cache.has_update() && !self.config.is_version_skipped(&cache.latest.version) {
                    return Ok(Some(self.build_update_info(&cache.latest)?));
                }
                return Ok(None);
            }
        }

        // Cache invalid or missing, check server
        self.check_update_forced().await
    }

    /// Force check for updates (bypass cache).
    pub async fn check_update_forced(&self) -> UpdateResult<Option<UpdateInfo>> {
        let latest = self.client.get_latest(self.config.channel).await?;

        // Update cache
        let cache = VersionCache::new(latest.clone(), self.install_method);
        if let Err(e) = cache.save() {
            tracing::warn!("Failed to save version cache: {}", e);
        }

        // Check if update is available
        match compare_versions(CURRENT_VERSION, &latest.version) {
            VersionComparison::Older => {
                if self.config.is_version_skipped(&latest.version) {
                    return Ok(None);
                }
                Ok(Some(self.build_update_info(&latest)?))
            }
            _ => Ok(None),
        }
    }

    /// Build UpdateInfo from ReleaseInfo.
    fn build_update_info(&self, release: &ReleaseInfo) -> UpdateResult<UpdateInfo> {
        let asset = release
            .asset_for_current_platform()
            .ok_or_else(|| UpdateError::NoPlatformAsset {
                platform: crate::api::platform_key(),
            })?
            .clone();

        Ok(UpdateInfo {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: release.version.clone(),
            channel: release.channel,
            changelog_url: release.changelog_url.clone(),
            release_notes: release.release_notes.clone(),
            asset,
            install_method: self.install_method,
        })
    }

    /// Download an update.
    pub async fn download_update<F>(
        &self,
        info: &UpdateInfo,
        on_progress: F,
    ) -> UpdateResult<DownloadedUpdate>
    where
        F: FnMut(DownloadProgress),
    {
        let downloader = Downloader::new(self.client.clone())?;
        let path = downloader
            .download(&info.asset, &info.latest_version, on_progress)
            .await?;

        Ok(DownloadedUpdate::new(path, info.latest_version.clone()))
    }

    /// Verify a downloaded update.
    pub async fn verify(
        &self,
        download: &mut DownloadedUpdate,
        expected_sha256: &str,
    ) -> UpdateResult<()> {
        verify_sha256(&download.archive_path, expected_sha256).await?;
        download.mark_verified();
        Ok(())
    }

    /// Install a verified update.
    pub async fn install(&self, download: &DownloadedUpdate) -> UpdateResult<UpdateOutcome> {
        let installer = Installer::new(self.install_method);
        installer.install(download).await?;

        Ok(UpdateOutcome::Updated {
            from: CURRENT_VERSION.to_string(),
            to: download.version.clone(),
        })
    }

    /// Full update flow: check -> download -> verify -> install.
    pub async fn update<F>(&self, on_progress: F) -> UpdateResult<UpdateOutcome>
    where
        F: FnMut(DownloadProgress),
    {
        // Check for update
        let info = match self.check_update().await? {
            Some(info) => info,
            None => return Ok(UpdateOutcome::AlreadyLatest),
        };

        // Download
        let mut download = self.download_update(&info, on_progress).await?;

        // Verify
        self.verify(&mut download, &info.asset.sha256).await?;

        // Install
        self.install(&download).await
    }

    /// Skip a version (don't prompt for it again).
    pub fn skip_version(&mut self, version: &str) -> UpdateResult<()> {
        self.config.skip_version = Some(version.to_string());
        self.config.save().map_err(|e| UpdateError::ConfigError {
            message: e.to_string(),
        })?;
        Ok(())
    }

    /// Clear the skipped version.
    pub fn clear_skip(&mut self) -> UpdateResult<()> {
        self.config.skip_version = None;
        self.config.save().map_err(|e| UpdateError::ConfigError {
            message: e.to_string(),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_manager_creation() {
        let manager = UpdateManager::new();
        assert!(manager.is_ok());
    }

    #[test]
    fn test_install_method_detection() {
        let manager = UpdateManager::new().unwrap();
        // Should detect something
        let _method = manager.install_method();
    }
}
