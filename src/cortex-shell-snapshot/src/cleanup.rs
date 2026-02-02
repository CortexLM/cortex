//! Snapshot cleanup utilities.

use super::{DEFAULT_RETENTION, Result};
use std::path::Path;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

/// Default retention period (7 days).
pub const SNAPSHOT_RETENTION: Duration = DEFAULT_RETENTION;

/// Remove shell snapshots that are older than the retention period.
pub async fn cleanup_stale_snapshots(
    snapshot_dir: &Path,
    active_session_id: Option<Uuid>,
    retention: Duration,
) -> Result<CleanupStats> {
    let mut stats = CleanupStats::default();

    // Check if directory exists
    if !snapshot_dir.exists() {
        return Ok(stats);
    }

    // Read directory
    let mut entries = tokio::fs::read_dir(snapshot_dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();

        // Only process snapshot files (not metadata)
        if path.extension().map(|e| e == "meta").unwrap_or(false) {
            continue;
        }

        stats.total_found += 1;

        // Check if this is the active session's snapshot
        if let Some(active_id) = active_session_id {
            let filename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if filename.contains(&active_id.to_string()) {
                stats.skipped_active += 1;
                continue;
            }
        }

        // Check age
        match entry.metadata().await {
            Ok(metadata) => {
                if let Ok(modified) = metadata.modified() {
                    let age = SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or(Duration::ZERO);

                    if age >= retention {
                        // Remove snapshot file
                        if let Err(e) = remove_snapshot_file(&path).await {
                            tracing::warn!("Failed to remove stale snapshot {:?}: {}", path, e);
                            stats.errors += 1;
                        } else {
                            stats.removed += 1;
                            stats.bytes_freed += metadata.len();
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get metadata for {:?}: {}", path, e);
                stats.errors += 1;
            }
        }
    }

    Ok(stats)
}

/// Remove a snapshot file and its metadata.
async fn remove_snapshot_file(path: &Path) -> Result<()> {
    // Remove main file
    tokio::fs::remove_file(path).await?;

    // Remove metadata file if exists
    let meta_path = path.with_extension("meta");
    if meta_path.exists() {
        let _ = tokio::fs::remove_file(&meta_path).await;
    }

    Ok(())
}

/// Statistics from cleanup operation.
#[derive(Debug, Default)]
pub struct CleanupStats {
    /// Total snapshots found.
    pub total_found: usize,

    /// Snapshots removed.
    pub removed: usize,

    /// Snapshots skipped (active session).
    pub skipped_active: usize,

    /// Bytes freed.
    pub bytes_freed: u64,

    /// Errors encountered.
    pub errors: usize,
}

impl CleanupStats {
    /// Check if any cleanup was performed.
    pub fn any_cleaned(&self) -> bool {
        self.removed > 0
    }
}

/// List all snapshots in a directory.
pub async fn list_snapshots(snapshot_dir: &Path) -> Result<Vec<SnapshotInfo>> {
    let mut snapshots = Vec::new();

    if !snapshot_dir.exists() {
        return Ok(snapshots);
    }

    let mut entries = tokio::fs::read_dir(snapshot_dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();

        // Skip metadata files
        if path.extension().map(|e| e == "meta").unwrap_or(false) {
            continue;
        }

        // Get file info
        if let Ok(metadata) = entry.metadata().await {
            let info = SnapshotInfo {
                path: path.clone(),
                size_bytes: metadata.len(),
                modified: metadata.modified().ok(),
            };
            snapshots.push(info);
        }
    }

    // Sort by modification time (newest first)
    snapshots.sort_by(|a, b| b.modified.cmp(&a.modified));

    Ok(snapshots)
}

/// Information about a snapshot file.
#[derive(Debug)]
pub struct SnapshotInfo {
    /// Path to the snapshot.
    pub path: std::path::PathBuf,

    /// Size in bytes.
    pub size_bytes: u64,

    /// Last modification time.
    pub modified: Option<SystemTime>,
}

impl SnapshotInfo {
    /// Get the age of the snapshot.
    pub fn age(&self) -> Option<Duration> {
        self.modified
            .and_then(|m| SystemTime::now().duration_since(m).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cleanup_stats_default() {
        let stats = CleanupStats::default();
        assert_eq!(stats.total_found, 0);
        assert_eq!(stats.removed, 0);
        assert!(!stats.any_cleaned());
    }
}
