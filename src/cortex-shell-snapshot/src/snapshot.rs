//! Shell snapshot structure and operations.

use super::{Result, ShellType, SnapshotError, scripts};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Metadata for a shell snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    /// Snapshot version.
    pub version: u32,

    /// Session ID that owns this snapshot.
    pub session_id: Uuid,

    /// Shell type.
    pub shell_type: ShellType,

    /// Creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Whether the snapshot has been validated.
    pub validated: bool,

    /// Size of the snapshot in bytes.
    pub size_bytes: u64,
}

impl SnapshotMetadata {
    /// Current metadata version.
    pub const VERSION: u32 = 1;

    /// Create new metadata.
    pub fn new(session_id: Uuid, shell_type: ShellType) -> Self {
        Self {
            version: Self::VERSION,
            session_id,
            shell_type,
            created_at: Utc::now(),
            validated: false,
            size_bytes: 0,
        }
    }
}

/// A shell snapshot.
#[derive(Debug, Clone)]
pub struct ShellSnapshot {
    /// Path to the snapshot file.
    pub path: PathBuf,

    /// Snapshot metadata.
    pub metadata: SnapshotMetadata,
}

impl ShellSnapshot {
    /// Create a new snapshot from a path and metadata.
    pub fn new(path: PathBuf, metadata: SnapshotMetadata) -> Self {
        Self { path, metadata }
    }

    /// Load a snapshot from a path.
    pub async fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if !path.exists() {
            return Err(SnapshotError::NotFound(path));
        }

        // Read metadata from companion .meta file
        let meta_path = path.with_extension("meta");
        let metadata = if meta_path.exists() {
            let meta_content = tokio::fs::read_to_string(&meta_path).await?;
            serde_json::from_str(&meta_content)
                .map_err(|e| SnapshotError::InvalidFormat(e.to_string()))?
        } else {
            // Try to infer metadata from filename
            Self::infer_metadata(&path)?
        };

        Ok(Self { path, metadata })
    }

    /// Infer metadata from filename.
    fn infer_metadata(path: &Path) -> Result<SnapshotMetadata> {
        let filename = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| SnapshotError::InvalidFormat("Invalid filename".to_string()))?;

        // Try to parse session ID from filename (format: snapshot_<session_id>.<shell>)
        let session_id = if let Some(id_part) = filename.strip_prefix("snapshot_") {
            Uuid::parse_str(id_part).unwrap_or_else(|_| Uuid::new_v4())
        } else {
            Uuid::new_v4()
        };

        // Infer shell type from extension
        let shell_type = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|e| e.parse().ok())
            .unwrap_or(ShellType::Bash);

        Ok(SnapshotMetadata::new(session_id, shell_type))
    }

    /// Get the snapshot content.
    pub async fn content(&self) -> Result<String> {
        tokio::fs::read_to_string(&self.path)
            .await
            .map_err(SnapshotError::from)
    }

    /// Generate a restore script that sources this snapshot.
    ///
    /// The path is properly escaped to prevent shell injection attacks.
    /// Paths containing single quotes are escaped using shell-safe quoting.
    pub fn restore_script(&self) -> String {
        let header = scripts::restore_header(self.metadata.shell_type);
        let escaped_path = shell_escape_path(&self.path);
        format!("{header}\n# Source snapshot\nsource {escaped_path}\n")
    }

    /// Save the snapshot to disk.
    pub async fn save(&self, content: &str) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Write content
        tokio::fs::write(&self.path, content).await?;

        // Write metadata
        let meta_path = self.path.with_extension("meta");
        let meta_content = serde_json::to_string_pretty(&self.metadata)
            .map_err(|e| SnapshotError::Internal(e.to_string()))?;
        tokio::fs::write(&meta_path, meta_content).await?;

        Ok(())
    }

    /// Validate the snapshot by attempting to parse it.
    pub async fn validate(&self) -> Result<bool> {
        let content = self.content().await?;

        // Basic validation: check for header comment
        if !content.contains("Cortex Shell Snapshot") {
            return Err(SnapshotError::ValidationFailed(
                "Missing snapshot header".to_string(),
            ));
        }

        // Check for reasonable content length
        if content.len() < 50 {
            return Err(SnapshotError::ValidationFailed(
                "Snapshot too short".to_string(),
            ));
        }

        Ok(true)
    }

    /// Get the age of the snapshot.
    pub fn age(&self) -> chrono::Duration {
        Utc::now() - self.metadata.created_at
    }

    /// Check if the snapshot is stale.
    pub fn is_stale(&self, retention: std::time::Duration) -> bool {
        let age_secs = self.age().num_seconds();
        age_secs > retention.as_secs() as i64
    }

    /// Generate the snapshot filename.
    pub fn filename(session_id: Uuid, shell_type: ShellType) -> String {
        format!(
            "snapshot_{}.{}",
            session_id,
            shell_type.snapshot_extension()
        )
    }

    /// Generate the full path for a snapshot.
    pub fn path_for(dir: &Path, session_id: Uuid, shell_type: ShellType) -> PathBuf {
        dir.join(Self::filename(session_id, shell_type))
    }
}

impl Drop for ShellSnapshot {
    fn drop(&mut self) {
        // Optionally clean up the snapshot file
        // Only do this if explicitly marked for cleanup
        // For now, we keep snapshots for reuse
    }
}

/// Escape a path for safe use in shell commands.
///
/// This function handles paths containing single quotes by using the
/// shell-safe escaping technique: 'path'"'"'with'"'"'quotes'
///
/// For paths without single quotes, simple single-quoting is used.
fn shell_escape_path(path: &Path) -> String {
    let path_str = path.display().to_string();

    if !path_str.contains('\'') {
        // Simple case: no single quotes, just wrap in single quotes
        format!("'{}'", path_str)
    } else {
        // Complex case: escape single quotes using '"'"' technique
        // This closes the single-quoted string, adds a double-quoted single quote,
        // and reopens the single-quoted string
        let escaped = path_str.replace('\'', "'\"'\"'");
        format!("'{}'", escaped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_new() {
        let session_id = Uuid::new_v4();
        let metadata = SnapshotMetadata::new(session_id, ShellType::Zsh);

        assert_eq!(metadata.version, SnapshotMetadata::VERSION);
        assert_eq!(metadata.session_id, session_id);
        assert_eq!(metadata.shell_type, ShellType::Zsh);
        assert!(!metadata.validated);
    }

    #[test]
    fn test_filename() {
        let session_id = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
        let filename = ShellSnapshot::filename(session_id, ShellType::Zsh);
        assert_eq!(
            filename,
            "snapshot_12345678-1234-1234-1234-123456789012.zsh"
        );
    }

    #[test]
    fn test_shell_escape_path_simple() {
        let path = Path::new("/tmp/test/snapshot.sh");
        let escaped = shell_escape_path(path);
        assert_eq!(escaped, "'/tmp/test/snapshot.sh'");
    }

    #[test]
    fn test_shell_escape_path_with_single_quotes() {
        let path = Path::new("/tmp/test's/snap'shot.sh");
        let escaped = shell_escape_path(path);
        // Single quotes should be escaped using '"'"' technique
        assert_eq!(escaped, "'/tmp/test'\"'\"'s/snap'\"'\"'shot.sh'");
    }

    #[test]
    fn test_shell_escape_path_spaces() {
        let path = Path::new("/tmp/test path/snapshot.sh");
        let escaped = shell_escape_path(path);
        assert_eq!(escaped, "'/tmp/test path/snapshot.sh'");
    }
}
