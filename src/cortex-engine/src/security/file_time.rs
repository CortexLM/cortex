//! File time tracking for read-before-write protection.
//!
//! Ensures files are read before being modified and detects
//! external changes since last read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;
use tokio::sync::RwLock;

/// Error types for file time tracking.
#[derive(Debug, Error)]
pub enum FileTimeError {
    #[error("File '{0}' must be read before editing")]
    NotRead(String),

    #[error("File '{0}' was modified externally since last read")]
    ModifiedExternally(String),

    #[error("Cannot get modification time for '{0}': {1}")]
    IoError(String, String),
}

/// Record of when a file was last read.
#[derive(Debug, Clone)]
struct FileReadRecord {
    /// Modification time when we read it.
    mtime_at_read: SystemTime,
    /// When we read it (for potential future LRU eviction).
    #[allow(dead_code)]
    read_at: std::time::Instant,
}

/// Tracks file read times per session.
#[derive(Debug, Default)]
pub struct FileTimeTracker {
    /// Records keyed by session ID, then by file path.
    records: Arc<RwLock<HashMap<String, HashMap<PathBuf, FileReadRecord>>>>,
    /// File locks for concurrent write protection.
    locks: Arc<RwLock<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>,
}

impl FileTimeTracker {
    /// Create a new tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a file was read.
    pub async fn record_read(&self, session_id: &str, path: &Path) -> Result<(), FileTimeError> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        let mtime = tokio::fs::metadata(&path)
            .await
            .and_then(|m| m.modified())
            .map_err(|e| FileTimeError::IoError(path.display().to_string(), e.to_string()))?;

        let record = FileReadRecord {
            mtime_at_read: mtime,
            read_at: std::time::Instant::now(),
        };

        let mut records = self.records.write().await;
        records
            .entry(session_id.to_string())
            .or_default()
            .insert(path, record);

        Ok(())
    }

    /// Assert that a file can be written (was read and not modified externally).
    pub async fn assert_writable(
        &self,
        session_id: &str,
        path: &Path,
    ) -> Result<(), FileTimeError> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        // Check if file was read
        let records = self.records.read().await;
        let session_records = records.get(session_id);

        let record = session_records
            .and_then(|r| r.get(&path))
            .ok_or_else(|| FileTimeError::NotRead(path.display().to_string()))?;

        // Check if file was modified externally
        if path.exists() {
            let current_mtime = tokio::fs::metadata(&path)
                .await
                .and_then(|m| m.modified())
                .map_err(|e| FileTimeError::IoError(path.display().to_string(), e.to_string()))?;

            if current_mtime != record.mtime_at_read {
                return Err(FileTimeError::ModifiedExternally(
                    path.display().to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Execute a write operation with file locking.
    pub async fn with_lock<F, T>(&self, path: &Path, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        // Get or create lock for this file
        let lock = {
            let mut locks = self.locks.write().await;
            locks
                .entry(path.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };

        // Acquire lock and execute
        let _guard = lock.lock().await;
        f.await
    }

    /// Clear records for a session.
    pub async fn clear_session(&self, session_id: &str) {
        let mut records = self.records.write().await;
        records.remove(session_id);
    }

    /// Check if a file was read in this session.
    pub async fn was_read(&self, session_id: &str, path: &Path) -> bool {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let records = self.records.read().await;
        records
            .get(session_id)
            .map(|r| r.contains_key(&path))
            .unwrap_or(false)
    }

    /// Get the time a file was last read.
    pub async fn last_read_time(
        &self,
        session_id: &str,
        path: &Path,
    ) -> Option<std::time::Instant> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let records = self.records.read().await;
        records
            .get(session_id)
            .and_then(|r| r.get(&path))
            .map(|r| r.read_at)
    }

    /// Update the record after a successful write.
    pub async fn record_write(&self, session_id: &str, path: &Path) -> Result<(), FileTimeError> {
        // Re-record with new mtime
        self.record_read(session_id, path).await
    }
}

/// Global file time tracker instance.
static GLOBAL_TRACKER: std::sync::OnceLock<FileTimeTracker> = std::sync::OnceLock::new();

/// Get the global file time tracker.
pub fn global_tracker() -> &'static FileTimeTracker {
    GLOBAL_TRACKER.get_or_init(FileTimeTracker::new)
}

impl Clone for FileTimeTracker {
    fn clone(&self) -> Self {
        Self {
            records: Arc::clone(&self.records),
            locks: Arc::clone(&self.locks),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_read_before_write() {
        let tracker = FileTimeTracker::new();
        let session = "test-session";

        // Create temp file
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "test content").unwrap();
        let path = file.path();

        // Should fail - not read yet
        let result = tracker.assert_writable(session, path).await;
        assert!(matches!(result, Err(FileTimeError::NotRead(_))));

        // Record read
        tracker.record_read(session, path).await.unwrap();

        // Should succeed now
        let result = tracker.assert_writable(session, path).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_external_modification() {
        let tracker = FileTimeTracker::new();
        let session = "test-session";

        // Create temp file
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();

        // Record read
        tracker.record_read(session, &path).await.unwrap();

        // Modify externally
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tokio::fs::write(&path, "modified content").await.unwrap();

        // Should fail - modified externally
        let result = tracker.assert_writable(session, &path).await;
        assert!(matches!(result, Err(FileTimeError::ModifiedExternally(_))));
    }

    #[tokio::test]
    async fn test_file_lock() {
        let tracker = FileTimeTracker::new();
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();

        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter2 = counter.clone();
        let tracker2 = tracker.clone();
        let path2 = path.clone();

        // Two concurrent writes should be serialized
        let h1 = tokio::spawn(async move {
            tracker
                .with_lock(&path, async {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                })
                .await;
        });

        let h2 = tokio::spawn(async move {
            tracker2
                .with_lock(&path2, async {
                    counter2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                })
                .await;
        });

        h1.await.unwrap();
        h2.await.unwrap();
    }
}
