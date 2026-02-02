//! Configuration for the auto-compaction system.

use serde::{Deserialize, Serialize};

use crate::CompactionConfig;

// ============================================================================
// Constants
// ============================================================================

/// Default compaction interval in seconds (1 hour).
pub const DEFAULT_COMPACTION_INTERVAL_SECS: u64 = 3600;

/// Default log retention period in days.
pub const DEFAULT_LOG_RETENTION_DAYS: u32 = 7;

/// Default session retention period in days (0 = keep forever).
pub const DEFAULT_SESSION_RETENTION_DAYS: u32 = 0;

/// Maximum size for a single log file in bytes (10 MB).
pub const MAX_LOG_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Lock file name for compaction operations.
pub const COMPACTION_LOCK_FILE: &str = ".compaction.lock";

/// Temp file suffix for atomic writes.
pub const TEMP_SUFFIX: &str = ".tmp";

/// Backup file suffix.
pub const BACKUP_SUFFIX: &str = ".bak";

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the auto-compaction system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoCompactionConfig {
    /// Whether auto-compaction is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Compaction check interval in seconds.
    #[serde(default = "default_interval")]
    pub interval_secs: u64,

    /// Log retention period in days (0 = keep forever).
    #[serde(default = "default_log_retention")]
    pub log_retention_days: u32,

    /// Session retention period in days (0 = keep forever).
    #[serde(default)]
    pub session_retention_days: u32,

    /// Maximum log file size in bytes before rotation.
    #[serde(default = "default_max_log_size")]
    pub max_log_file_size: u64,

    /// Whether to vacuum database on startup.
    #[serde(default = "default_enabled")]
    pub vacuum_on_startup: bool,

    /// Minimum free disk space in MB before triggering cleanup.
    #[serde(default = "default_min_disk_space")]
    pub min_free_disk_space_mb: u64,

    /// Conversation compaction config.
    #[serde(default)]
    pub conversation: CompactionConfig,
}

fn default_enabled() -> bool {
    true
}

fn default_interval() -> u64 {
    DEFAULT_COMPACTION_INTERVAL_SECS
}

fn default_log_retention() -> u32 {
    DEFAULT_LOG_RETENTION_DAYS
}

fn default_max_log_size() -> u64 {
    MAX_LOG_FILE_SIZE
}

fn default_min_disk_space() -> u64 {
    100 // 100 MB minimum
}

impl Default for AutoCompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: DEFAULT_COMPACTION_INTERVAL_SECS,
            log_retention_days: DEFAULT_LOG_RETENTION_DAYS,
            session_retention_days: DEFAULT_SESSION_RETENTION_DAYS,
            max_log_file_size: MAX_LOG_FILE_SIZE,
            vacuum_on_startup: true,
            min_free_disk_space_mb: 100,
            conversation: CompactionConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_compaction_config_defaults() {
        let config = AutoCompactionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.interval_secs, DEFAULT_COMPACTION_INTERVAL_SECS);
        assert_eq!(config.log_retention_days, DEFAULT_LOG_RETENTION_DAYS);
        assert_eq!(config.max_log_file_size, MAX_LOG_FILE_SIZE);
    }
}
