//! Version caching and comparison.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::CURRENT_VERSION;
use crate::api::ReleaseInfo;
use crate::config::UpdateConfig;
use crate::error::{UpdateError, UpdateResult};
use crate::method::InstallMethod;

/// Cached version information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionCache {
    /// Latest release info from last check
    pub latest: ReleaseInfo,
    /// When the check was performed
    pub checked_at: DateTime<Utc>,
    /// Detected installation method
    pub install_method: InstallMethod,
    /// Current version at time of check
    pub current_version: String,
}

impl VersionCache {
    /// Create a new cache entry.
    pub fn new(latest: ReleaseInfo, install_method: InstallMethod) -> Self {
        Self {
            latest,
            checked_at: Utc::now(),
            install_method,
            current_version: CURRENT_VERSION.to_string(),
        }
    }

    /// Get the path to the cache file.
    pub fn cache_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".cortex").join("update_cache.json"))
    }

    /// Load the cache from disk.
    pub fn load() -> Option<Self> {
        let path = Self::cache_path()?;
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Save the cache to disk.
    pub fn save(&self) -> UpdateResult<()> {
        let path = Self::cache_path().ok_or(UpdateError::CacheError {
            message: "No home directory".to_string(),
        })?;

        // Create directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;

        Ok(())
    }

    /// Check if the cache is still valid based on config.
    pub fn is_valid(&self, config: &UpdateConfig) -> bool {
        let max_age = Duration::minutes(config.check_interval_minutes as i64);
        let age = Utc::now() - self.checked_at;
        age < max_age
    }

    /// Check if an update is available.
    pub fn has_update(&self) -> bool {
        compare_versions(&self.current_version, &self.latest.version) == VersionComparison::Older
    }
}

/// Result of comparing two versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionComparison {
    /// Current is older than target
    Older,
    /// Current equals target
    Equal,
    /// Current is newer than target
    Newer,
}

/// Compare two semver version strings.
pub fn compare_versions(current: &str, target: &str) -> VersionComparison {
    let current = parse_version(current);
    let target = parse_version(target);

    match current.cmp(&target) {
        std::cmp::Ordering::Less => VersionComparison::Older,
        std::cmp::Ordering::Equal => VersionComparison::Equal,
        std::cmp::Ordering::Greater => VersionComparison::Newer,
    }
}

/// Parse a version string into comparable parts.
fn parse_version(version: &str) -> (u32, u32, u32, String) {
    // Remove 'v' prefix if present
    let version = version.strip_prefix('v').unwrap_or(version);

    // Split by '-' to separate prerelease
    let (version_part, prerelease) = version
        .split_once('-')
        .map(|(v, p)| (v, p.to_string()))
        .unwrap_or((version, String::new()));

    // Split by '.' for major.minor.patch
    let parts: Vec<u32> = version_part
        .split('.')
        .take(3)
        .filter_map(|s| s.parse().ok())
        .collect();

    let major = parts.first().copied().unwrap_or(0);
    let minor = parts.get(1).copied().unwrap_or(0);
    let patch = parts.get(2).copied().unwrap_or(0);

    (major, minor, patch, prerelease)
}

/// Check if a version meets the minimum requirement.
pub fn meets_minimum(current: &str, minimum: &str) -> bool {
    compare_versions(current, minimum) != VersionComparison::Older
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions("0.1.0", "0.2.0"), VersionComparison::Older);
        assert_eq!(compare_versions("0.2.0", "0.2.0"), VersionComparison::Equal);
        assert_eq!(compare_versions("0.3.0", "0.2.0"), VersionComparison::Newer);
        assert_eq!(compare_versions("1.0.0", "0.9.9"), VersionComparison::Newer);
    }

    #[test]
    fn test_compare_versions_with_prefix() {
        assert_eq!(
            compare_versions("v0.1.0", "0.2.0"),
            VersionComparison::Older
        );
        assert_eq!(
            compare_versions("0.1.0", "v0.2.0"),
            VersionComparison::Older
        );
    }

    #[test]
    fn test_meets_minimum() {
        assert!(meets_minimum("0.2.0", "0.1.0"));
        assert!(meets_minimum("0.1.0", "0.1.0"));
        assert!(!meets_minimum("0.0.9", "0.1.0"));
    }
}
