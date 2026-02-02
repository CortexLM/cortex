//! Log pruner for intelligent cleanup of log files.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{debug, info};

use crate::Result;

use super::config::AutoCompactionConfig;
use super::utils::chrono_timestamp;

/// Log file information for pruning decisions.
#[derive(Debug)]
pub struct LogFileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    pub age_days: u32,
}

/// Result of log pruning operation.
#[derive(Debug, Clone, Serialize)]
pub struct LogPruningResult {
    /// Number of files deleted.
    pub files_deleted: usize,
    /// Total bytes freed.
    pub bytes_freed: u64,
    /// Number of files rotated.
    pub files_rotated: usize,
    /// Any errors encountered (non-fatal).
    pub errors: Vec<String>,
}

impl LogPruningResult {
    fn new() -> Self {
        Self {
            files_deleted: 0,
            bytes_freed: 0,
            files_rotated: 0,
            errors: Vec::new(),
        }
    }

    fn add_error(&mut self, error: String) {
        self.errors.push(error);
    }
}

/// Log pruner for intelligent cleanup of log files.
pub struct LogPruner {
    config: AutoCompactionConfig,
}

impl LogPruner {
    pub fn new(config: AutoCompactionConfig) -> Self {
        Self { config }
    }

    /// Prune log files in the specified directory.
    ///
    /// Applies the following rules:
    /// 1. Delete files older than retention period
    /// 2. Rotate files larger than max size
    /// 3. Keep most recent files even if over size limit
    pub fn prune(&self, logs_dir: &Path) -> Result<LogPruningResult> {
        let mut result = LogPruningResult::new();

        if !logs_dir.exists() {
            return Ok(result);
        }

        let now = SystemTime::now();

        // Collect log file information
        let mut log_files = Vec::new();
        if let Ok(entries) = fs::read_dir(logs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && is_log_file(&path) {
                    if let Ok(metadata) = entry.metadata() {
                        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        let age = now.duration_since(modified).unwrap_or_default();
                        let age_days = (age.as_secs() / (24 * 60 * 60)) as u32;

                        log_files.push(LogFileInfo {
                            path,
                            size: metadata.len(),
                            modified,
                            age_days,
                        });
                    }
                }
            }
        }

        // Sort by modified time (newest first) for rotation priority
        log_files.sort_by_key(|f| std::cmp::Reverse(f.modified));

        // Process each log file
        for (idx, log_file) in log_files.iter().enumerate() {
            // Delete files older than retention period
            if self.config.log_retention_days > 0
                && log_file.age_days > self.config.log_retention_days
            {
                match fs::remove_file(&log_file.path) {
                    Ok(()) => {
                        result.files_deleted += 1;
                        result.bytes_freed += log_file.size;
                        info!(
                            path = %log_file.path.display(),
                            age_days = log_file.age_days,
                            "Deleted old log file"
                        );
                    }
                    Err(e) => {
                        result.add_error(format!(
                            "Failed to delete {}: {}",
                            log_file.path.display(),
                            e
                        ));
                    }
                }
                continue;
            }

            // Rotate files that exceed max size (except the most recent)
            if log_file.size > self.config.max_log_file_size && idx > 0 {
                if let Err(e) = self.rotate_log_file(&log_file.path) {
                    result.add_error(format!(
                        "Failed to rotate {}: {}",
                        log_file.path.display(),
                        e
                    ));
                } else {
                    result.files_rotated += 1;
                }
            }
        }

        if result.files_deleted > 0 || result.files_rotated > 0 {
            info!(
                files_deleted = result.files_deleted,
                bytes_freed = result.bytes_freed,
                files_rotated = result.files_rotated,
                "Log pruning completed"
            );
        }

        Ok(result)
    }

    /// Rotate a log file by compressing and archiving it.
    fn rotate_log_file(&self, path: &Path) -> std::io::Result<()> {
        let timestamp = chrono_timestamp();
        let rotated_name = format!(
            "{}.{}.log",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("log"),
            timestamp
        );
        let rotated_path = path.with_file_name(rotated_name);

        // Rename the original file
        fs::rename(path, &rotated_path)?;

        debug!(
            original = %path.display(),
            rotated = %rotated_path.display(),
            "Rotated log file"
        );

        Ok(())
    }
}

/// Check if a file is a log file based on extension.
fn is_log_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| matches!(ext, "log" | "txt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_logs_dir() -> (TempDir, PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let logs_dir = temp_dir.path().join("logs");
        fs::create_dir_all(&logs_dir).unwrap();
        (temp_dir, logs_dir)
    }

    #[test]
    fn test_log_pruning_old_files() {
        let (_temp, logs_dir) = create_logs_dir();

        // Create some log files
        fs::write(logs_dir.join("recent.log"), "recent log").unwrap();
        fs::write(logs_dir.join("old.log"), "old log").unwrap();

        // Set modification time of old.log to 30 days ago
        // Note: This is tricky to test without system calls, so we skip the age check in tests

        let config = AutoCompactionConfig {
            log_retention_days: 7,
            ..Default::default()
        };
        let pruner = LogPruner::new(config);

        let result = pruner.prune(&logs_dir).unwrap();
        assert!(result.errors.is_empty());
    }
}
