//! Revert functionality for undoing changes.

use crate::{Result, Snapshot, SnapshotManager};

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use tracing::info;

/// A point in history that can be reverted to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevertPoint {
    /// Associated snapshot.
    pub snapshot: Snapshot,
    /// Files that were modified after this point.
    pub modified_files: Vec<PathBuf>,
    /// The diff at this point.
    pub diff: Option<String>,
    /// Whether this point is active (not reverted past).
    pub active: bool,
}

impl RevertPoint {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot,
            modified_files: Vec::new(),
            diff: None,
            active: true,
        }
    }

    pub fn with_files(mut self, files: Vec<PathBuf>) -> Self {
        self.modified_files = files;
        self
    }

    pub fn with_diff(mut self, diff: String) -> Self {
        self.diff = Some(diff);
        self
    }
}

/// Manager for revert operations.
pub struct RevertManager {
    /// Snapshot manager.
    snapshot_manager: SnapshotManager,
    /// Stack of revert points (most recent last).
    history: VecDeque<RevertPoint>,
    /// Maximum history size.
    max_history: usize,
    /// Current position in history (for redo).
    current_position: usize,
    /// Redo stack.
    redo_stack: Vec<RevertPoint>,
}

impl RevertManager {
    pub fn new(snapshot_manager: SnapshotManager) -> Self {
        Self {
            snapshot_manager,
            history: VecDeque::new(),
            max_history: 100,
            current_position: 0,
            redo_stack: Vec::new(),
        }
    }

    pub fn with_max_history(mut self, max: usize) -> Self {
        self.max_history = max;
        self
    }

    /// Create a revert point at the current state.
    pub async fn checkpoint(&mut self, description: Option<&str>) -> Result<RevertPoint> {
        // Clear redo stack when making new changes
        self.redo_stack.clear();

        // Create snapshot
        let mut snapshot = self.snapshot_manager.create().await?;
        if let Some(desc) = description {
            snapshot.description = Some(desc.to_string());
        }

        let point = RevertPoint::new(snapshot);

        // Add to history
        self.history.push_back(point.clone());

        // Trim history if needed
        while self.history.len() > self.max_history {
            self.history.pop_front();
        }

        self.current_position = self.history.len();

        info!("Created checkpoint: {}", point.snapshot.id);
        Ok(point)
    }

    /// Create a checkpoint associated with a session/message.
    pub async fn checkpoint_for_message(
        &mut self,
        session_id: &str,
        message_id: &str,
        description: Option<&str>,
    ) -> Result<RevertPoint> {
        // Clear redo stack when making new changes
        self.redo_stack.clear();

        let snapshot = self
            .snapshot_manager
            .create_with_metadata(description, Some(session_id), Some(message_id))
            .await?;

        let point = RevertPoint::new(snapshot);

        self.history.push_back(point.clone());

        while self.history.len() > self.max_history {
            self.history.pop_front();
        }

        self.current_position = self.history.len();

        info!(
            "Created checkpoint for message {}: {}",
            message_id, point.snapshot.id
        );
        Ok(point)
    }

    /// Undo to the previous checkpoint.
    pub async fn undo(&mut self) -> Result<Option<RevertPoint>> {
        if self.history.is_empty() || self.current_position == 0 {
            return Ok(None);
        }

        // Save current state to redo stack
        let current_snapshot = self.snapshot_manager.create().await?;
        self.redo_stack.push(RevertPoint::new(current_snapshot));

        // Get the point to revert to
        self.current_position = self.current_position.saturating_sub(1);
        let point = self.history.get(self.current_position).cloned();

        if let Some(ref p) = point {
            // Get changed files before restore
            let changed = self.snapshot_manager.changed_files(&p.snapshot).await?;

            // Restore to that snapshot
            self.snapshot_manager.restore(&p.snapshot).await?;

            info!(
                "Undid to checkpoint: {} ({} files)",
                p.snapshot.id,
                changed.len()
            );
        }

        Ok(point)
    }

    /// Redo a previously undone action.
    pub async fn redo(&mut self) -> Result<Option<RevertPoint>> {
        if let Some(point) = self.redo_stack.pop() {
            self.snapshot_manager.restore(&point.snapshot).await?;
            self.current_position = self
                .current_position
                .saturating_add(1)
                .min(self.history.len());

            info!("Redid to checkpoint: {}", point.snapshot.id);
            Ok(Some(point))
        } else {
            Ok(None)
        }
    }

    /// Revert to a specific checkpoint by ID.
    pub async fn revert_to(&mut self, snapshot_id: &str) -> Result<Option<RevertPoint>> {
        let position = self
            .history
            .iter()
            .position(|p| p.snapshot.id == snapshot_id);

        if let Some(pos) = position {
            let point = self.history.get(pos).cloned();

            if let Some(ref p) = point {
                // Save everything after this point to redo stack
                for i in (pos + 1)..self.history.len() {
                    if let Some(rp) = self.history.get(i).cloned() {
                        self.redo_stack.push(rp);
                    }
                }

                // Get changed files
                let changed = self.snapshot_manager.changed_files(&p.snapshot).await?;

                // Restore
                self.snapshot_manager.restore(&p.snapshot).await?;
                self.current_position = pos + 1;

                info!(
                    "Reverted to checkpoint: {} ({} files changed)",
                    p.snapshot.id,
                    changed.len()
                );
            }

            Ok(point)
        } else {
            Ok(None)
        }
    }

    /// Revert changes for a specific message.
    pub async fn revert_message(&mut self, message_id: &str) -> Result<Vec<PathBuf>> {
        // Find the checkpoint before this message
        let position = self
            .history
            .iter()
            .position(|p| p.snapshot.message_id.as_deref() == Some(message_id));

        if let Some(pos) = position {
            // Find the previous checkpoint
            if pos > 0 {
                if let Some(prev) = self.history.get(pos - 1) {
                    let _current = self.history.get(pos).unwrap();

                    // Get files changed between these checkpoints
                    let changed = self.snapshot_manager.changed_files(&prev.snapshot).await?;

                    // Restore those specific files
                    self.snapshot_manager
                        .restore_files(&prev.snapshot, &changed)
                        .await?;

                    info!(
                        "Reverted message {}: {} files restored",
                        message_id,
                        changed.len()
                    );
                    return Ok(changed);
                }
            }
        }

        Ok(Vec::new())
    }

    /// Get the current history.
    pub fn history(&self) -> &VecDeque<RevertPoint> {
        &self.history
    }

    /// Get history for a specific session.
    pub fn session_history(&self, session_id: &str) -> Vec<&RevertPoint> {
        self.history
            .iter()
            .filter(|p| p.snapshot.session_id.as_deref() == Some(session_id))
            .collect()
    }

    /// Check if undo is available.
    pub fn can_undo(&self) -> bool {
        self.current_position > 0 && !self.history.is_empty()
    }

    /// Check if redo is available.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Get the diff between current state and a checkpoint.
    pub async fn diff_from(&mut self, snapshot_id: &str) -> Result<Option<String>> {
        let point = self.history.iter().find(|p| p.snapshot.id == snapshot_id);

        if let Some(p) = point {
            let diff = self.snapshot_manager.diff(&p.snapshot).await?;
            Ok(Some(diff))
        } else {
            Ok(None)
        }
    }

    /// Clear all history.
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.redo_stack.clear();
        self.current_position = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_revert_manager() {
        let temp_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let snapshot_manager = SnapshotManager::new(temp_dir.path(), data_dir.path());
        let mut revert_manager = RevertManager::new(snapshot_manager);

        // Create initial file
        let test_file = temp_dir.path().join("test.txt");
        tokio::fs::write(&test_file, "Version 1").await.unwrap();

        // Checkpoint
        let _cp1 = revert_manager.checkpoint(Some("Initial")).await.unwrap();

        // Modify file
        tokio::fs::write(&test_file, "Version 2").await.unwrap();

        // Another checkpoint
        let _cp2 = revert_manager.checkpoint(Some("Modified")).await.unwrap();

        // Verify can undo
        assert!(revert_manager.can_undo());
    }
}
