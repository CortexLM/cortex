//! Snapshot service for tracking file system changes.

use crate::diff::UnifiedDiff;
use crate::error::Result;
use cortex_snapshot::{Snapshot, SnapshotManager};
use std::path::PathBuf;
use tracing::{debug, info};

/// Service for managing snapshots during agent turns.
pub struct SnapshotService {
    manager: SnapshotManager,
    before_snapshot: Option<Snapshot>,
    after_snapshot: Option<Snapshot>,
    target_files: Vec<PathBuf>,
}

impl SnapshotService {
    /// Create a new snapshot service.
    pub fn new(workspace_root: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            manager: SnapshotManager::new(workspace_root, data_dir),
            before_snapshot: None,
            after_snapshot: None,
            target_files: Vec::new(),
        }
    }

    /// Set target files for the next snapshots.
    pub fn set_target_files(&mut self, files: Vec<PathBuf>) {
        self.target_files = files;
    }

    /// Clear target files.
    pub fn clear_target_files(&mut self) {
        self.target_files.clear();
    }

    /// Take a 'before' snapshot.
    pub async fn take_before(&mut self) -> Result<()> {
        info!("Taking 'before' snapshot");
        let snapshot = self
            .manager
            .create()
            .await
            .map_err(|e| crate::error::CortexError::Internal(e.to_string()))?;
        self.before_snapshot = Some(snapshot);
        Ok(())
    }

    /// Take an 'after' snapshot.
    pub async fn take_after(&mut self) -> Result<()> {
        info!("Taking 'after' snapshot");
        let snapshot = self
            .manager
            .create()
            .await
            .map_err(|e| crate::error::CortexError::Internal(e.to_string()))?;
        self.after_snapshot = Some(snapshot);
        Ok(())
    }

    /// Detect changes between 'before' and 'after' snapshots.
    pub async fn detect_changes(&mut self) -> Result<UnifiedDiff> {
        let before = self.before_snapshot.as_ref().ok_or_else(|| {
            crate::error::CortexError::Internal("No 'before' snapshot taken".to_string())
        })?;

        let after = self.after_snapshot.as_ref().ok_or_else(|| {
            crate::error::CortexError::Internal("No 'after' snapshot taken".to_string())
        })?;

        if before.tree_hash == after.tree_hash {
            debug!("No changes detected between snapshots");
            return Ok(UnifiedDiff::empty());
        }

        let files = if self.target_files.is_empty() {
            None
        } else {
            Some(&self.target_files[..])
        };

        let diff_text = self
            .manager
            .diff_between_filtered(before, after, files)
            .await
            .map_err(|e| crate::error::CortexError::Internal(e.to_string()))?;

        if diff_text.is_empty() {
            return Ok(UnifiedDiff::empty());
        }

        UnifiedDiff::parse(&diff_text)
    }

    /// Get the 'before' snapshot.
    pub fn before_snapshot(&self) -> Option<&Snapshot> {
        self.before_snapshot.as_ref()
    }

    /// Get the 'after' snapshot.
    pub fn after_snapshot(&self) -> Option<&Snapshot> {
        self.after_snapshot.as_ref()
    }

    /// Reset the service state.
    pub fn reset(&mut self) {
        self.before_snapshot = None;
        self.after_snapshot = None;
        self.target_files.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_snapshot_service() {
        let temp_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        // Initialize git in temp_dir because SnapshotManager uses it
        let _ = tokio::process::Command::new("git")
            .arg("init")
            .current_dir(temp_dir.path())
            .output()
            .await
            .unwrap();

        let mut service = SnapshotService::new(temp_dir.path(), data_dir.path());

        // Create a file
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "original content")
            .await
            .unwrap();

        // Take before
        service.take_before().await.unwrap();

        // Modify file
        tokio::fs::write(&file_path, "modified content")
            .await
            .unwrap();

        // Take after
        service.take_after().await.unwrap();

        // Detect changes
        let diff = service.detect_changes().await.unwrap();
        assert!(!diff.is_empty());
        assert_eq!(diff.file_count(), 1);

        let file_diff = &diff.files[0];
        assert!(file_diff.to_string().contains("-original content"));
        assert!(file_diff.to_string().contains("+modified content"));
    }
}
