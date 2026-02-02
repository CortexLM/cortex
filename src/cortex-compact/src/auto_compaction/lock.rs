//! File-based lock for preventing concurrent compaction operations.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::{debug, warn};

use crate::{CompactionError, Result};

use super::config::COMPACTION_LOCK_FILE;
use super::utils::timestamp_now;

/// File-based lock for preventing concurrent compaction operations.
///
/// Uses advisory locking via lock files to coordinate between processes.
pub struct CompactionLock {
    lock_path: PathBuf,
    acquired: bool,
}

impl CompactionLock {
    /// Attempt to acquire the compaction lock.
    ///
    /// Returns `Ok(Some(lock))` if lock was acquired, `Ok(None)` if lock is held by another process,
    /// or `Err` if there was a filesystem error.
    pub fn try_acquire(data_dir: &Path) -> Result<Option<Self>> {
        let lock_path = data_dir.join(COMPACTION_LOCK_FILE);

        // Check if lock file exists and is recent
        if lock_path.exists() {
            if let Ok(metadata) = fs::metadata(&lock_path) {
                if let Ok(modified) = metadata.modified() {
                    // If lock is older than 1 hour, consider it stale
                    let age = SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or_default();

                    if age < Duration::from_secs(3600) {
                        debug!(lock_path = %lock_path.display(), "Compaction lock held by another process");
                        return Ok(None);
                    } else {
                        warn!(
                            lock_path = %lock_path.display(),
                            age_secs = age.as_secs(),
                            "Removing stale compaction lock"
                        );
                        let _ = fs::remove_file(&lock_path);
                    }
                }
            }
        }

        // Create lock file with PID
        let pid = std::process::id();
        let lock_content = format!("{}\n{}", pid, timestamp_now());

        match fs::write(&lock_path, lock_content) {
            Ok(()) => {
                debug!(lock_path = %lock_path.display(), pid = pid, "Acquired compaction lock");
                Ok(Some(Self {
                    lock_path,
                    acquired: true,
                }))
            }
            Err(e) => {
                warn!(error = %e, "Failed to create compaction lock file");
                Err(CompactionError::Io(e))
            }
        }
    }

    /// Release the lock.
    pub fn release(&mut self) {
        if self.acquired {
            if let Err(e) = fs::remove_file(&self.lock_path) {
                warn!(
                    error = %e,
                    lock_path = %self.lock_path.display(),
                    "Failed to remove compaction lock file"
                );
            } else {
                debug!(lock_path = %self.lock_path.display(), "Released compaction lock");
            }
            self.acquired = false;
        }
    }
}

impl Drop for CompactionLock {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_compaction_lock_acquire_release() {
        let temp_dir = TempDir::new().unwrap();
        let data_dir = temp_dir.path().to_path_buf();

        // First lock should succeed
        let lock1 = CompactionLock::try_acquire(&data_dir).unwrap();
        assert!(lock1.is_some());

        // Second lock should fail (lock held)
        let lock2 = CompactionLock::try_acquire(&data_dir).unwrap();
        assert!(lock2.is_none());

        // After releasing, should be able to acquire again
        drop(lock1);
        let lock3 = CompactionLock::try_acquire(&data_dir).unwrap();
        assert!(lock3.is_some());
    }
}
