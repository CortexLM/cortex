//! Snapshot creation and management.

use crate::{Result, SnapshotError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Default timeout for git operations in seconds
const DEFAULT_GIT_TIMEOUT_SECS: u64 = 30;

/// Get the configured git timeout duration
fn get_git_timeout() -> Duration {
    std::env::var("CORTEX_GIT_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_GIT_TIMEOUT_SECS))
}

/// Execute a git command with timeout and environment variables
async fn git_command_with_timeout_env(
    args: &[&str],
    cwd: &PathBuf,
    git_dir: &PathBuf,
    work_tree: &PathBuf,
) -> Result<std::process::Output> {
    let timeout = get_git_timeout();

    let future = Command::new("git")
        .env("GIT_DIR", git_dir)
        .env("GIT_WORK_TREE", work_tree)
        .args(args)
        .current_dir(cwd)
        .output();

    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result.map_err(SnapshotError::Io),
        Err(_) => Err(SnapshotError::GitTimeout {
            command: format!("git {}", args.join(" ")),
            timeout_secs: timeout.as_secs(),
        }),
    }
}

/// Execute a git command with timeout in git_dir only
async fn git_command_in_dir_with_timeout(
    args: &[&str],
    cwd: &PathBuf,
) -> Result<std::process::Output> {
    let timeout = get_git_timeout();

    let future = Command::new("git").args(args).current_dir(cwd).output();

    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result.map_err(SnapshotError::Io),
        Err(_) => Err(SnapshotError::GitTimeout {
            command: format!("git {}", args.join(" ")),
            timeout_secs: timeout.as_secs(),
        }),
    }
}

/// A snapshot of the workspace state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Unique snapshot ID.
    pub id: String,
    /// Git tree hash.
    pub tree_hash: String,
    /// Timestamp.
    pub created_at: DateTime<Utc>,
    /// Description.
    pub description: Option<String>,
    /// Session ID (if associated with a session).
    pub session_id: Option<String>,
    /// Message ID (if associated with a message).
    pub message_id: Option<String>,
    /// Files tracked in this snapshot.
    pub files: Vec<String>,
}

impl Snapshot {
    pub fn new(tree_hash: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            tree_hash,
            created_at: Utc::now(),
            description: None,
            session_id: None,
            message_id: None,
            files: Vec::new(),
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_message(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }
}

/// Manager for creating and managing snapshots.
pub struct SnapshotManager {
    /// Workspace root directory.
    root: PathBuf,
    /// Git directory for snapshots.
    git_dir: PathBuf,
    /// Whether git is initialized.
    initialized: bool,
}

impl SnapshotManager {
    pub fn new(root: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let data_dir = data_dir.into();

        // Create a unique git dir based on workspace path
        let hash = Self::hash_path(&root);
        let git_dir = data_dir.join("snapshots").join(&hash);

        Self {
            root,
            git_dir,
            initialized: false,
        }
    }

    /// Initialize the snapshot system.
    pub async fn init(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        // Create git dir if needed
        tokio::fs::create_dir_all(&self.git_dir).await?;

        // Check if already initialized
        let git_objects = self.git_dir.join("objects");
        if !git_objects.exists() {
            // Initialize bare git repo
            let output =
                git_command_in_dir_with_timeout(&["init", "--bare"], &self.git_dir).await?;

            if !output.status.success() {
                return Err(SnapshotError::Git(
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ));
            }

            // Configure git
            self.git_config("core.autocrlf", "false").await?;
            self.git_config("core.fileMode", "false").await?;

            info!("Initialized snapshot repository: {:?}", self.git_dir);
        }

        self.initialized = true;
        Ok(())
    }

    /// Create a snapshot of the current state.
    pub async fn create(&mut self) -> Result<Snapshot> {
        self.init().await?;

        // Add all files to index
        self.git_add_all().await?;

        // Write tree
        let tree_hash = self.git_write_tree().await?;

        let snapshot = Snapshot::new(tree_hash);

        info!(
            "Created snapshot: {} (tree: {})",
            snapshot.id, snapshot.tree_hash
        );
        Ok(snapshot)
    }

    /// Create a snapshot with metadata.
    pub async fn create_with_metadata(
        &mut self,
        description: Option<&str>,
        session_id: Option<&str>,
        message_id: Option<&str>,
    ) -> Result<Snapshot> {
        let mut snapshot = self.create().await?;

        if let Some(desc) = description {
            snapshot.description = Some(desc.to_string());
        }
        if let Some(sid) = session_id {
            snapshot.session_id = Some(sid.to_string());
        }
        if let Some(mid) = message_id {
            snapshot.message_id = Some(mid.to_string());
        }

        Ok(snapshot)
    }

    /// Get the diff between a snapshot and current state.
    pub async fn diff(&mut self, snapshot: &Snapshot) -> Result<String> {
        self.init().await?;

        // Add current state
        self.git_add_all().await?;

        // Get diff
        let output = git_command_with_timeout_env(
            &["diff", "--no-ext-diff", &snapshot.tree_hash, "--", "."],
            &self.root,
            &self.git_dir,
            &self.root,
        )
        .await?;

        if !output.status.success() {
            warn!(
                "Git diff failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(String::new());
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Get list of changed files since snapshot.
    pub async fn changed_files(&mut self, snapshot: &Snapshot) -> Result<Vec<PathBuf>> {
        self.init().await?;

        // Add current state
        self.git_add_all().await?;

        // Get changed files
        let output = git_command_with_timeout_env(
            &[
                "diff",
                "--no-ext-diff",
                "--name-only",
                &snapshot.tree_hash,
                "--",
                ".",
            ],
            &self.root,
            &self.git_dir,
            &self.root,
        )
        .await?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let files: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| self.root.join(l.trim()))
            .collect();

        Ok(files)
    }

    /// Get the diff between two snapshots.
    pub async fn diff_between(&mut self, before: &Snapshot, after: &Snapshot) -> Result<String> {
        self.diff_between_filtered(before, after, None).await
    }

    /// Get the diff between two snapshots, optionally filtered by files.
    pub async fn diff_between_filtered(
        &mut self,
        before: &Snapshot,
        after: &Snapshot,
        files: Option<&[PathBuf]>,
    ) -> Result<String> {
        self.init().await?;

        let mut args = vec![
            "diff".to_string(),
            "--no-ext-diff".to_string(),
            before.tree_hash.clone(),
            after.tree_hash.clone(),
            "--".to_string(),
        ];

        if let Some(files) = files {
            for file in files {
                let relative = file
                    .strip_prefix(&self.root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| file.to_string_lossy().to_string());
                args.push(relative);
            }
        } else {
            args.push(".".to_string());
        }

        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let output =
            git_command_with_timeout_env(&args_refs, &self.root, &self.git_dir, &self.root).await?;

        if !output.status.success() {
            warn!(
                "Git diff between snapshots failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(String::new());
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Restore workspace to snapshot state.
    pub async fn restore(&mut self, snapshot: &Snapshot) -> Result<()> {
        self.init().await?;

        // Read tree
        let output = git_command_with_timeout_env(
            &["read-tree", &snapshot.tree_hash],
            &self.root,
            &self.git_dir,
            &self.root,
        )
        .await?;

        if !output.status.success() {
            return Err(SnapshotError::RestoreFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        // Checkout
        let output = git_command_with_timeout_env(
            &["checkout-index", "-a", "-f"],
            &self.root,
            &self.git_dir,
            &self.root,
        )
        .await?;

        if !output.status.success() {
            return Err(SnapshotError::RestoreFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!(
            "Restored snapshot: {} (tree: {})",
            snapshot.id, snapshot.tree_hash
        );
        Ok(())
    }

    /// Restore specific files from snapshot.
    pub async fn restore_files(&mut self, snapshot: &Snapshot, files: &[PathBuf]) -> Result<()> {
        self.init().await?;

        for file in files {
            let relative = file
                .strip_prefix(&self.root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| file.to_string_lossy().to_string());

            let output = git_command_with_timeout_env(
                &["checkout", &snapshot.tree_hash, "--", &relative],
                &self.root,
                &self.git_dir,
                &self.root,
            )
            .await?;

            if !output.status.success() {
                // File might not exist in snapshot - try to delete it
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("did not match") || stderr.contains("pathspec") {
                    // File doesn't exist in snapshot, delete it
                    if file.exists() {
                        tokio::fs::remove_file(file).await?;
                        debug!("Deleted file not in snapshot: {:?}", file);
                    }
                } else {
                    warn!("Failed to restore file {:?}: {}", file, stderr);
                }
            } else {
                debug!("Restored file: {:?}", file);
            }
        }

        Ok(())
    }

    async fn git_add_all(&self) -> Result<()> {
        let output =
            git_command_with_timeout_env(&["add", "."], &self.root, &self.git_dir, &self.root)
                .await?;

        if !output.status.success() {
            // Ignore errors from git add (might have no files)
            debug!(
                "git add warning: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    async fn git_write_tree(&self) -> Result<String> {
        let output =
            git_command_with_timeout_env(&["write-tree"], &self.root, &self.git_dir, &self.root)
                .await?;

        if !output.status.success() {
            return Err(SnapshotError::CreateFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn git_config(&self, key: &str, value: &str) -> Result<()> {
        git_command_in_dir_with_timeout(&["config", key, value], &self.git_dir).await?;
        Ok(())
    }

    fn hash_path(path: &Path) -> String {
        let mut hasher = Sha256::new();
        hasher.update(path.to_string_lossy().as_bytes());
        hex::encode(hasher.finalize())[..16].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_snapshot_creation() {
        let temp_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        tokio::fs::write(&test_file, "Hello, World!").await.unwrap();

        let mut manager = SnapshotManager::new(temp_dir.path(), data_dir.path());
        let snapshot = manager.create().await.unwrap();

        assert!(!snapshot.tree_hash.is_empty());
        assert!(!snapshot.id.is_empty());
    }

    #[tokio::test]
    async fn test_diff_between() {
        let temp_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let mut manager = SnapshotManager::new(temp_dir.path(), data_dir.path());

        // Snapshot 1: initial
        let file1 = temp_dir.path().join("f1.txt");
        tokio::fs::write(&file1, "v1").await.unwrap();
        let s1 = manager.create().await.unwrap();

        // Snapshot 2: changed
        tokio::fs::write(&file1, "v2").await.unwrap();
        let s2 = manager.create().await.unwrap();

        let diff = manager.diff_between(&s1, &s2).await.unwrap();
        assert!(diff.contains("-v1"));
        assert!(diff.contains("+v2"));
    }
}
