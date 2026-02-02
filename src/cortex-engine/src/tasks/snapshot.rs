//! Snapshot management for undo/recovery.
//!
//! Provides file system snapshots for reverting changes.

use std::collections::HashMap;
use std::fs;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

/// Snapshot of file system state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Snapshot ID.
    pub id: String,
    /// Description.
    pub description: String,
    /// Timestamp.
    pub timestamp: u64,
    /// Turn ID that created this snapshot.
    pub turn_id: Option<String>,
    /// File states.
    pub files: HashMap<PathBuf, FileState>,
    /// Directories created.
    pub directories: Vec<PathBuf>,
}

impl Snapshot {
    /// Create a new empty snapshot.
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            timestamp: timestamp_now(),
            turn_id: None,
            files: HashMap::new(),
            directories: Vec::new(),
        }
    }

    /// Set turn ID.
    pub fn with_turn(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    /// Add a file to the snapshot.
    pub fn add_file(&mut self, path: impl AsRef<Path>, state: FileState) {
        self.files.insert(path.as_ref().to_path_buf(), state);
    }

    /// Add a directory.
    pub fn add_directory(&mut self, path: impl AsRef<Path>) {
        self.directories.push(path.as_ref().to_path_buf());
    }

    /// Capture file state from disk.
    pub fn capture_file(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();

        let state = if path.exists() {
            let content = fs::read(path)?;
            let metadata = fs::metadata(path)?;

            FileState::Exists {
                content,
                permissions: get_permissions(&metadata),
                modified: metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
            }
        } else {
            FileState::NotExists
        };

        self.files.insert(path.to_path_buf(), state);
        Ok(())
    }

    /// Capture multiple files.
    pub fn capture_files<P: AsRef<Path>>(
        &mut self,
        paths: impl IntoIterator<Item = P>,
    ) -> Result<()> {
        for path in paths {
            self.capture_file(path)?;
        }
        Ok(())
    }

    /// Get files that would be modified.
    pub fn files_to_modify(&self) -> Vec<&PathBuf> {
        self.files.keys().collect()
    }

    /// Get number of files.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Check if snapshot is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.directories.is_empty()
    }
}

/// File state in a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileState {
    /// File exists with content.
    Exists {
        /// File content.
        content: Vec<u8>,
        /// File permissions (Unix mode).
        permissions: Option<u32>,
        /// Last modified timestamp.
        modified: Option<u64>,
    },
    /// File does not exist.
    NotExists,
}

impl FileState {
    /// Check if file exists in this state.
    pub fn exists(&self) -> bool {
        matches!(self, Self::Exists { .. })
    }

    /// Get content if exists.
    pub fn content(&self) -> Option<&[u8]> {
        match self {
            Self::Exists { content, .. } => Some(content),
            Self::NotExists => None,
        }
    }

    /// Get content as string if valid UTF-8.
    pub fn content_str(&self) -> Option<&str> {
        self.content().and_then(|c| std::str::from_utf8(c).ok())
    }
}

/// Get file permissions.
#[cfg(unix)]
fn get_permissions(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn get_permissions(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

/// Snapshot manager for maintaining snapshot history.
pub struct SnapshotManager {
    /// Snapshots indexed by ID.
    snapshots: RwLock<HashMap<String, Snapshot>>,
    /// Snapshot order (newest first).
    order: RwLock<Vec<String>>,
    /// Maximum snapshots to keep.
    max_snapshots: usize,
    /// Storage directory.
    storage_dir: Option<PathBuf>,
}

impl SnapshotManager {
    /// Create a new snapshot manager.
    pub fn new(max_snapshots: usize) -> Self {
        Self {
            snapshots: RwLock::new(HashMap::new()),
            order: RwLock::new(Vec::new()),
            max_snapshots,
            storage_dir: None,
        }
    }

    /// Set storage directory for persistence.
    pub fn with_storage(mut self, dir: impl AsRef<Path>) -> Self {
        self.storage_dir = Some(dir.as_ref().to_path_buf());
        self
    }

    /// Create a new snapshot.
    pub async fn create(&self, description: impl Into<String>) -> Snapshot {
        let id = generate_snapshot_id();
        Snapshot::new(id, description)
    }

    /// Save a snapshot.
    pub async fn save(&self, snapshot: Snapshot) -> Result<()> {
        let id = snapshot.id.clone();

        // Add to storage
        let mut snapshots = self.snapshots.write().await;
        let mut order = self.order.write().await;

        snapshots.insert(id.clone(), snapshot);
        order.insert(0, id);

        // Trim old snapshots
        while order.len() > self.max_snapshots {
            if let Some(old_id) = order.pop() {
                snapshots.remove(&old_id);
            }
        }

        // Persist if storage is configured
        if let Some(ref dir) = self.storage_dir {
            self.persist_to_disk(dir, &snapshots).await?;
        }

        Ok(())
    }

    /// Get a snapshot by ID.
    pub async fn get(&self, id: &str) -> Option<Snapshot> {
        self.snapshots.read().await.get(id).cloned()
    }

    /// Get the latest snapshot.
    pub async fn latest(&self) -> Option<Snapshot> {
        let order = self.order.read().await;
        if let Some(id) = order.first() {
            self.snapshots.read().await.get(id).cloned()
        } else {
            None
        }
    }

    /// Get snapshot for a turn.
    pub async fn for_turn(&self, turn_id: &str) -> Option<Snapshot> {
        let snapshots = self.snapshots.read().await;
        snapshots
            .values()
            .find(|s| s.turn_id.as_deref() == Some(turn_id))
            .cloned()
    }

    /// List all snapshots.
    pub async fn list(&self) -> Vec<SnapshotInfo> {
        let snapshots = self.snapshots.read().await;
        let order = self.order.read().await;

        order
            .iter()
            .filter_map(|id| snapshots.get(id))
            .map(|s| SnapshotInfo {
                id: s.id.clone(),
                description: s.description.clone(),
                timestamp: s.timestamp,
                file_count: s.files.len(),
            })
            .collect()
    }

    /// Delete a snapshot.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut snapshots = self.snapshots.write().await;
        let mut order = self.order.write().await;

        snapshots.remove(id);
        order.retain(|i| i != id);

        Ok(())
    }

    /// Clear all snapshots.
    pub async fn clear(&self) {
        self.snapshots.write().await.clear();
        self.order.write().await.clear();
    }

    /// Restore a snapshot to disk.
    pub async fn restore(&self, id: &str) -> Result<RestoreResult> {
        let snapshot = self
            .get(id)
            .await
            .ok_or_else(|| CortexError::NotFound(format!("Snapshot not found: {id}")))?;

        let mut result = RestoreResult::new();

        // Restore files
        for (path, state) in &snapshot.files {
            match state {
                FileState::Exists {
                    content,
                    permissions,
                    ..
                } => {
                    // Create parent directories
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }

                    // Write content
                    fs::write(path, content)?;

                    // Set permissions
                    #[cfg(unix)]
                    if let Some(mode) = permissions {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(path, std::fs::Permissions::from_mode(*mode))?;
                    }

                    result.restored.push(path.clone());
                }
                FileState::NotExists => {
                    // Delete file if it exists
                    if path.exists() {
                        fs::remove_file(path)?;
                        result.deleted.push(path.clone());
                    }
                }
            }
        }

        // Remove directories that were created after snapshot
        for dir in snapshot.directories.iter().rev() {
            if dir.exists() && dir.is_dir() {
                // Only remove if empty
                if fs::read_dir(dir)?.next().is_none() {
                    fs::remove_dir(dir)?;
                    result.directories_removed.push(dir.clone());
                }
            }
        }

        Ok(result)
    }

    /// Persist snapshots to disk.
    async fn persist_to_disk(
        &self,
        dir: &Path,
        snapshots: &HashMap<String, Snapshot>,
    ) -> Result<()> {
        fs::create_dir_all(dir)?;

        for (id, snapshot) in snapshots {
            let path = dir.join(format!("{id}.json"));
            let json = serde_json::to_string_pretty(snapshot)
                .map_err(|e| CortexError::Serialization(e.to_string()))?;
            fs::write(path, json)?;
        }

        Ok(())
    }

    /// Load snapshots from disk.
    pub async fn load_from_disk(&self) -> Result<()> {
        let dir = match &self.storage_dir {
            Some(d) => d,
            None => return Ok(()),
        };

        if !dir.exists() {
            return Ok(());
        }

        let mut snapshots = self.snapshots.write().await;
        let mut order = self.order.write().await;

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e == "json").unwrap_or(false) {
                let content = fs::read_to_string(&path)?;
                let snapshot: Snapshot = serde_json::from_str(&content)
                    .map_err(|e| CortexError::Serialization(e.to_string()))?;

                order.push(snapshot.id.clone());
                snapshots.insert(snapshot.id.clone(), snapshot);
            }
        }

        // Sort by timestamp (newest first)
        order.sort_by(|a, b| {
            let ts_a = snapshots.get(a).map(|s| s.timestamp).unwrap_or(0);
            let ts_b = snapshots.get(b).map(|s| s.timestamp).unwrap_or(0);
            ts_b.cmp(&ts_a)
        });

        Ok(())
    }
}

/// Snapshot info for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Snapshot ID.
    pub id: String,
    /// Description.
    pub description: String,
    /// Timestamp.
    pub timestamp: u64,
    /// Number of files.
    pub file_count: usize,
}

/// Result of a restore operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RestoreResult {
    /// Files restored.
    pub restored: Vec<PathBuf>,
    /// Files deleted.
    pub deleted: Vec<PathBuf>,
    /// Directories removed.
    pub directories_removed: Vec<PathBuf>,
}

impl RestoreResult {
    /// Create a new restore result.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get total changes.
    pub fn total_changes(&self) -> usize {
        self.restored.len() + self.deleted.len() + self.directories_removed.len()
    }

    /// Check if anything was changed.
    pub fn has_changes(&self) -> bool {
        self.total_changes() > 0
    }
}

/// Generate a unique snapshot ID.
fn generate_snapshot_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("snap_{ts:x}")
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_snapshot_creation() {
        let snapshot = Snapshot::new("test-1", "Test snapshot");
        assert_eq!(snapshot.id, "test-1");
        assert!(snapshot.is_empty());
    }

    #[test]
    fn test_file_state() {
        let state = FileState::Exists {
            content: b"hello".to_vec(),
            permissions: Some(0o644),
            modified: Some(12345),
        };

        assert!(state.exists());
        assert_eq!(state.content(), Some(b"hello".as_slice()));
        assert_eq!(state.content_str(), Some("hello"));
    }

    #[tokio::test]
    async fn test_snapshot_manager() {
        let manager = SnapshotManager::new(10);

        let mut snapshot = manager.create("Test").await;
        snapshot.add_file(
            "/test.txt",
            FileState::Exists {
                content: b"test".to_vec(),
                permissions: None,
                modified: None,
            },
        );

        manager.save(snapshot).await.unwrap();

        let latest = manager.latest().await.unwrap();
        assert_eq!(latest.file_count(), 1);
    }

    #[test]
    fn test_capture_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello").unwrap();

        let mut snapshot = Snapshot::new("test", "Test");
        snapshot.capture_file(&file_path).unwrap();

        assert_eq!(snapshot.file_count(), 1);
        let state = snapshot.files.get(&file_path).unwrap();
        assert_eq!(state.content_str(), Some("hello"));
    }
}
