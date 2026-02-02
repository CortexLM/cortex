//! Snapshot storage and persistence.

use crate::{Result, RevertPoint, Snapshot, SnapshotError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, info};

/// Persistent storage for snapshots.
pub struct SnapshotStorage {
    storage_path: PathBuf,
}

impl SnapshotStorage {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let storage_path = data_dir.into().join("snapshot_meta");
        Self { storage_path }
    }

    /// Initialize storage directory.
    pub async fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.storage_path).await?;
        Ok(())
    }

    /// Save a snapshot.
    pub async fn save_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        self.init().await?;
        let path = self.snapshot_path(&snapshot.id);
        let json = serde_json::to_string_pretty(snapshot)
            .map_err(|e| SnapshotError::CreateFailed(e.to_string()))?;
        fs::write(&path, json).await?;
        debug!("Saved snapshot: {}", snapshot.id);
        Ok(())
    }

    /// Load a snapshot by ID.
    pub async fn load_snapshot(&self, id: &str) -> Result<Snapshot> {
        let path = self.snapshot_path(id);
        let json = fs::read_to_string(&path)
            .await
            .map_err(|_| SnapshotError::NotFound(id.to_string()))?;
        let snapshot: Snapshot =
            serde_json::from_str(&json).map_err(|e| SnapshotError::NotFound(e.to_string()))?;
        Ok(snapshot)
    }

    /// Delete a snapshot.
    pub async fn delete_snapshot(&self, id: &str) -> Result<()> {
        let path = self.snapshot_path(id);
        if path.exists() {
            fs::remove_file(&path).await?;
            debug!("Deleted snapshot: {}", id);
        }
        Ok(())
    }

    /// List all snapshots.
    pub async fn list_snapshots(&self) -> Result<Vec<Snapshot>> {
        self.init().await?;
        let mut snapshots = Vec::new();

        let mut entries = fs::read_dir(&self.storage_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(json) = fs::read_to_string(&path).await {
                    if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&json) {
                        snapshots.push(snapshot);
                    }
                }
            }
        }

        // Sort by creation time
        snapshots.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(snapshots)
    }

    /// List snapshots for a session.
    pub async fn list_session_snapshots(&self, session_id: &str) -> Result<Vec<Snapshot>> {
        let all = self.list_snapshots().await?;
        Ok(all
            .into_iter()
            .filter(|s| s.session_id.as_deref() == Some(session_id))
            .collect())
    }

    /// Save session revert history.
    pub async fn save_revert_history(
        &self,
        session_id: &str,
        history: &[RevertPoint],
    ) -> Result<()> {
        self.init().await?;
        let path = self.history_path(session_id);
        let json = serde_json::to_string_pretty(history)
            .map_err(|e| SnapshotError::CreateFailed(e.to_string()))?;
        fs::write(&path, json).await?;
        debug!("Saved revert history for session: {}", session_id);
        Ok(())
    }

    /// Load session revert history.
    pub async fn load_revert_history(&self, session_id: &str) -> Result<Vec<RevertPoint>> {
        let path = self.history_path(session_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let json = fs::read_to_string(&path).await?;
        let history: Vec<RevertPoint> =
            serde_json::from_str(&json).map_err(|e| SnapshotError::NotFound(e.to_string()))?;
        Ok(history)
    }

    /// Clean up old snapshots (keep only the most recent N).
    pub async fn cleanup(&self, keep_count: usize) -> Result<usize> {
        let mut snapshots = self.list_snapshots().await?;
        let mut removed = 0;

        if snapshots.len() > keep_count {
            // Keep the most recent ones
            let to_remove: Vec<_> = snapshots.drain(keep_count..).collect();
            for snapshot in to_remove {
                self.delete_snapshot(&snapshot.id).await?;
                removed += 1;
            }
            info!("Cleaned up {} old snapshots", removed);
        }

        Ok(removed)
    }

    fn snapshot_path(&self, id: &str) -> PathBuf {
        self.storage_path.join(format!("{}.json", id))
    }

    fn history_path(&self, session_id: &str) -> PathBuf {
        self.storage_path
            .join(format!("history_{}.json", session_id))
    }
}

/// Index for fast snapshot lookup.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SnapshotIndex {
    /// Map from session ID to snapshot IDs.
    pub by_session: HashMap<String, Vec<String>>,
    /// Map from message ID to snapshot ID.
    pub by_message: HashMap<String, String>,
    /// All snapshot IDs in chronological order.
    pub chronological: Vec<String>,
}

impl SnapshotIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, snapshot: &Snapshot) {
        if let Some(ref session_id) = snapshot.session_id {
            self.by_session
                .entry(session_id.clone())
                .or_default()
                .push(snapshot.id.clone());
        }

        if let Some(ref message_id) = snapshot.message_id {
            self.by_message
                .insert(message_id.clone(), snapshot.id.clone());
        }

        self.chronological.push(snapshot.id.clone());
    }

    pub fn remove(&mut self, snapshot_id: &str) {
        self.chronological.retain(|id| id != snapshot_id);

        for ids in self.by_session.values_mut() {
            ids.retain(|id| id != snapshot_id);
        }

        self.by_message.retain(|_, id| id != snapshot_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_snapshot_storage() {
        let temp_dir = TempDir::new().unwrap();
        let storage = SnapshotStorage::new(temp_dir.path());

        let snapshot = Snapshot::new("test_hash".to_string()).with_description("Test snapshot");

        storage.save_snapshot(&snapshot).await.unwrap();

        let loaded = storage.load_snapshot(&snapshot.id).await.unwrap();
        assert_eq!(loaded.tree_hash, snapshot.tree_hash);
        assert_eq!(loaded.description, snapshot.description);
    }
}
