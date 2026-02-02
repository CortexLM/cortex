//! Snapshot configuration.

use super::{DEFAULT_RETENTION, SNAPSHOT_TIMEOUT};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Configuration for shell snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotConfig {
    /// Whether snapshotting is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Path to store snapshots.
    #[serde(default)]
    pub snapshot_dir: Option<PathBuf>,

    /// Timeout for capture operations.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Retention period in seconds.
    #[serde(default = "default_retention_secs")]
    pub retention_secs: u64,

    /// Whether to validate snapshots before use.
    #[serde(default = "default_validate")]
    pub validate: bool,

    /// Additional variables to exclude from export.
    #[serde(default)]
    pub exclude_vars: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

fn default_timeout_secs() -> u64 {
    SNAPSHOT_TIMEOUT.as_secs()
}

fn default_retention_secs() -> u64 {
    DEFAULT_RETENTION.as_secs()
}

fn default_validate() -> bool {
    true
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            snapshot_dir: None,
            timeout_secs: SNAPSHOT_TIMEOUT.as_secs(),
            retention_secs: DEFAULT_RETENTION.as_secs(),
            validate: true,
            exclude_vars: Vec::new(),
        }
    }
}

impl SnapshotConfig {
    /// Create a new config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the timeout duration.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    /// Get the retention duration.
    pub fn retention(&self) -> Duration {
        Duration::from_secs(self.retention_secs)
    }

    /// Get the snapshot directory, with fallback.
    pub fn snapshot_dir(&self, cortex_home: &std::path::Path) -> PathBuf {
        self.snapshot_dir
            .clone()
            .unwrap_or_else(|| cortex_home.join(super::SNAPSHOT_DIR))
    }

    /// Builder: set enabled.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Builder: set snapshot directory.
    pub fn snapshot_dir_path(mut self, dir: impl Into<PathBuf>) -> Self {
        self.snapshot_dir = Some(dir.into());
        self
    }

    /// Builder: set timeout.
    pub fn timeout_duration(mut self, timeout: Duration) -> Self {
        self.timeout_secs = timeout.as_secs();
        self
    }

    /// Builder: set retention.
    pub fn retention_duration(mut self, retention: Duration) -> Self {
        self.retention_secs = retention.as_secs();
        self
    }

    /// Builder: add excluded variable.
    pub fn exclude_var(mut self, var: impl Into<String>) -> Self {
        self.exclude_vars.push(var.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SnapshotConfig::default();
        assert!(config.enabled);
        assert!(config.validate);
        assert_eq!(config.timeout_secs, 10);
    }

    #[test]
    fn test_builder() {
        let config = SnapshotConfig::new()
            .enabled(false)
            .timeout_duration(Duration::from_secs(30));

        assert!(!config.enabled);
        assert_eq!(config.timeout(), Duration::from_secs(30));
    }
}
