//! Database vacuumer for optimizing session storage.

use serde::Serialize;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};

use crate::{CompactionError, Result};

use super::atomic_ops::atomic_write;
use super::config::AutoCompactionConfig;

/// Result of database vacuuming operation.
#[derive(Debug, Clone, Serialize)]
pub struct VacuumResult {
    /// Number of sessions processed.
    pub sessions_processed: usize,
    /// Number of orphaned history files cleaned.
    pub orphaned_cleaned: usize,
    /// Number of sessions compacted.
    pub sessions_compacted: usize,
    /// Number of old sessions deleted.
    pub sessions_deleted: usize,
    /// Total bytes freed.
    pub bytes_freed: u64,
    /// Any errors encountered (non-fatal).
    pub errors: Vec<String>,
}

impl VacuumResult {
    fn new() -> Self {
        Self {
            sessions_processed: 0,
            orphaned_cleaned: 0,
            sessions_compacted: 0,
            sessions_deleted: 0,
            bytes_freed: 0,
            errors: Vec::new(),
        }
    }

    fn add_error(&mut self, error: String) {
        self.errors.push(error);
    }
}

/// Database vacuumer for optimizing session storage.
pub struct DatabaseVacuumer {
    config: AutoCompactionConfig,
}

impl DatabaseVacuumer {
    pub fn new(config: AutoCompactionConfig) -> Self {
        Self { config }
    }

    /// Perform vacuuming on session storage.
    ///
    /// This includes:
    /// 1. Cleaning orphaned history files (no matching session)
    /// 2. Deleting old sessions past retention period
    /// 3. Compacting history files (removing redundant data)
    pub fn vacuum(&self, sessions_dir: &Path, history_dir: &Path) -> Result<VacuumResult> {
        let mut result = VacuumResult::new();

        // Collect session IDs from metadata files
        let session_ids: HashSet<String> = self.collect_session_ids(sessions_dir);

        // Clean orphaned history files
        result.orphaned_cleaned = self.clean_orphaned_history(history_dir, &session_ids)?;

        // Process each session
        if sessions_dir.exists() {
            let now = SystemTime::now();
            let retention_secs =
                Duration::from_secs(self.config.session_retention_days as u64 * 24 * 60 * 60);

            if let Ok(entries) = fs::read_dir(sessions_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && path.extension().is_some_and(|e| e == "json") {
                        result.sessions_processed += 1;

                        // Check session age for deletion
                        if self.config.session_retention_days > 0 {
                            if let Ok(metadata) = entry.metadata() {
                                if let Ok(modified) = metadata.modified() {
                                    let age = now.duration_since(modified).unwrap_or_default();
                                    if age > retention_secs {
                                        if let Err(e) = self.delete_session(&path, history_dir) {
                                            result.add_error(format!(
                                                "Failed to delete old session {}: {}",
                                                path.display(),
                                                e
                                            ));
                                        } else {
                                            result.sessions_deleted += 1;
                                            result.bytes_freed += metadata.len();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if result.orphaned_cleaned > 0
            || result.sessions_compacted > 0
            || result.sessions_deleted > 0
        {
            info!(
                sessions_processed = result.sessions_processed,
                orphaned_cleaned = result.orphaned_cleaned,
                sessions_deleted = result.sessions_deleted,
                bytes_freed = result.bytes_freed,
                "Database vacuum completed"
            );
        }

        Ok(result)
    }

    /// Collect all session IDs from metadata files.
    fn collect_session_ids(&self, sessions_dir: &Path) -> HashSet<String> {
        let mut ids = HashSet::new();

        if sessions_dir.exists() {
            if let Ok(entries) = fs::read_dir(sessions_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && path.extension().is_some_and(|e| e == "json") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            ids.insert(stem.to_string());
                        }
                    }
                }
            }
        }

        ids
    }

    /// Clean history files that have no matching session.
    fn clean_orphaned_history(
        &self,
        history_dir: &Path,
        session_ids: &HashSet<String>,
    ) -> Result<usize> {
        let mut cleaned = 0;

        if !history_dir.exists() {
            return Ok(0);
        }

        if let Ok(entries) = fs::read_dir(history_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if !session_ids.contains(stem) {
                            match fs::remove_file(&path) {
                                Ok(()) => {
                                    cleaned += 1;
                                    debug!(
                                        path = %path.display(),
                                        "Cleaned orphaned history file"
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        path = %path.display(),
                                        error = %e,
                                        "Failed to clean orphaned history file"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(cleaned)
    }

    /// Delete a session and its associated history.
    fn delete_session(&self, session_path: &Path, history_dir: &Path) -> io::Result<()> {
        // Delete session metadata
        fs::remove_file(session_path)?;

        // Delete associated history file if it exists
        if let Some(stem) = session_path.file_stem().and_then(|s| s.to_str()) {
            let history_path = history_dir.join(format!("{}.jsonl", stem));
            if history_path.exists() {
                fs::remove_file(&history_path)?;
            }
        }

        debug!(path = %session_path.display(), "Deleted old session");
        Ok(())
    }

    /// Compact a history file by removing old entries.
    ///
    /// Uses atomic write to ensure file safety.
    pub fn compact_history_file(&self, path: &Path, keep_recent: usize) -> Result<(usize, usize)> {
        if !path.exists() {
            return Ok((0, 0));
        }

        // Read all lines
        let file = File::open(path).map_err(CompactionError::Io)?;
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map_while(|l| l.ok()).collect();

        let original_count = lines.len();

        // Keep only recent entries
        if original_count <= keep_recent {
            return Ok((original_count, 0));
        }

        let keep_lines = &lines[lines.len() - keep_recent..];
        let compacted_data = keep_lines.join("\n") + "\n";

        // Atomic write
        atomic_write(path, compacted_data.as_bytes()).map_err(CompactionError::Io)?;

        let removed = original_count - keep_lines.len();
        debug!(
            path = %path.display(),
            original = original_count,
            kept = keep_lines.len(),
            removed = removed,
            "Compacted history file"
        );

        Ok((keep_lines.len(), removed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_dirs() -> (TempDir, PathBuf, PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let sessions_dir = temp_dir.path().join("sessions");
        let history_dir = temp_dir.path().join("history");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::create_dir_all(&history_dir).unwrap();
        (temp_dir, sessions_dir, history_dir)
    }

    use std::path::PathBuf;

    #[test]
    fn test_vacuum_orphaned_files() {
        let (_temp, sessions_dir, history_dir) = create_test_dirs();

        // Create a session
        fs::write(sessions_dir.join("session1.json"), r#"{"id": "session1"}"#).unwrap();

        // Create matching history
        fs::write(
            history_dir.join("session1.jsonl"),
            r#"{"message": "hello"}"#,
        )
        .unwrap();

        // Create orphaned history (no matching session)
        fs::write(history_dir.join("orphan.jsonl"), r#"{"message": "orphan"}"#).unwrap();

        let config = AutoCompactionConfig::default();
        let vacuumer = DatabaseVacuumer::new(config);

        let result = vacuumer.vacuum(&sessions_dir, &history_dir).unwrap();
        assert_eq!(result.orphaned_cleaned, 1);
        assert!(!history_dir.join("orphan.jsonl").exists());
        assert!(history_dir.join("session1.jsonl").exists());
    }

    #[test]
    fn test_compact_history_file() {
        let temp_dir = TempDir::new().unwrap();
        let history_path = temp_dir.path().join("test.jsonl");

        // Create history with 10 lines
        let lines: Vec<String> = (0..10).map(|i| format!(r#"{{"line": {}}}"#, i)).collect();
        fs::write(&history_path, lines.join("\n") + "\n").unwrap();

        let config = AutoCompactionConfig::default();
        let vacuumer = DatabaseVacuumer::new(config);

        let (kept, removed) = vacuumer.compact_history_file(&history_path, 3).unwrap();
        assert_eq!(kept, 3);
        assert_eq!(removed, 7);

        // Verify content
        let content = fs::read_to_string(&history_path).unwrap();
        let result_lines: Vec<_> = content.lines().collect();
        assert_eq!(result_lines.len(), 3);
    }
}
