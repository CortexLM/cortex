//! Git-based snapshot for fast undo/redo tracking.
//!
//! Uses git's plumbing commands (write-tree, read-tree) for instant snapshots.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{CortexError, Result};

/// Git-based snapshot manager.
pub struct GitSnapshot {
    /// Path to the separate git directory for snapshots.
    git_dir: PathBuf,
    /// Working tree (project directory).
    work_tree: PathBuf,
}

impl GitSnapshot {
    /// Create a new git snapshot manager.
    pub fn new(cortex_home: &Path, project_id: &str, work_tree: PathBuf) -> Result<Self> {
        let git_dir = cortex_home.join("snapshots").join("git").join(project_id);

        Ok(Self { git_dir, work_tree })
    }

    /// Initialize the snapshot git repository if needed.
    pub fn init(&self) -> Result<()> {
        if !self.git_dir.exists() {
            std::fs::create_dir_all(&self.git_dir)?;

            // Initialize bare-ish git repo
            let output = Command::new("git")
                .arg("init")
                .env("GIT_DIR", &self.git_dir)
                .env("GIT_WORK_TREE", &self.work_tree)
                .output()?;

            if !output.status.success() {
                tracing::warn!(
                    "Failed to init snapshot git: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }

            // Disable autocrlf for consistent snapshots on Windows
            let _ = Command::new("git")
                .args(["config", "core.autocrlf", "false"])
                .env("GIT_DIR", &self.git_dir)
                .output();

            tracing::info!("Initialized git snapshot at {:?}", self.git_dir);
        }
        Ok(())
    }

    /// Track current state and return a snapshot hash.
    /// This is very fast because it uses git's index.
    pub fn track(&self) -> Result<String> {
        self.init()?;

        // Stage all files (respects .gitignore automatically)
        let add_output = Command::new("git")
            .args(["add", "."])
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .current_dir(&self.work_tree)
            .output()?;

        if !add_output.status.success() {
            tracing::warn!(
                "git add failed: {}",
                String::from_utf8_lossy(&add_output.stderr)
            );
        }

        // Create tree object - this is instant!
        let output = Command::new("git")
            .arg("write-tree")
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .current_dir(&self.work_tree)
            .output()?;

        if !output.status.success() {
            return Err(CortexError::Snapshot(format!(
                "git write-tree failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
        tracing::debug!("Snapshot tracked: {}", hash);
        Ok(hash)
    }

    /// Get list of files changed since a snapshot.
    pub fn changed_files(&self, hash: &str) -> Result<Vec<PathBuf>> {
        // Stage current state
        let _ = Command::new("git")
            .args(["add", "."])
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .current_dir(&self.work_tree)
            .output();

        // Get diff
        let output = Command::new("git")
            .args(["diff", "--name-only", hash, "--", "."])
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .current_dir(&self.work_tree)
            .output()?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let files: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|s| !s.is_empty())
            .map(|s| self.work_tree.join(s))
            .collect();

        Ok(files)
    }

    /// Get unified diff since a snapshot.
    pub fn diff(&self, hash: &str) -> Result<String> {
        // Stage current state
        let _ = Command::new("git")
            .args(["add", "."])
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .current_dir(&self.work_tree)
            .output();

        let output = Command::new("git")
            .args(["diff", "--no-ext-diff", hash, "--", "."])
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .current_dir(&self.work_tree)
            .output()?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Restore files to a snapshot state.
    pub fn restore(&self, hash: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["read-tree", hash])
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .current_dir(&self.work_tree)
            .output()?;

        if !output.status.success() {
            return Err(CortexError::Snapshot(format!(
                "git read-tree failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let output = Command::new("git")
            .args(["checkout-index", "-a", "-f"])
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .current_dir(&self.work_tree)
            .output()?;

        if !output.status.success() {
            return Err(CortexError::Snapshot(format!(
                "git checkout-index failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::info!("Restored to snapshot: {}", hash);
        Ok(())
    }

    /// Restore a specific file to a snapshot state.
    pub fn restore_file(&self, hash: &str, file: &Path) -> Result<()> {
        let relative = file.strip_prefix(&self.work_tree).unwrap_or(file);

        let output = Command::new("git")
            .args(["checkout", hash, "--", relative.to_string_lossy().as_ref()])
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .current_dir(&self.work_tree)
            .output()?;

        if !output.status.success() {
            // File might not exist in snapshot - check and delete if so
            let check = Command::new("git")
                .args(["ls-tree", hash, "--", relative.to_string_lossy().as_ref()])
                .env("GIT_DIR", &self.git_dir)
                .env("GIT_WORK_TREE", &self.work_tree)
                .output()?;

            if check.stdout.is_empty() {
                // File didn't exist in snapshot, delete it
                let _ = std::fs::remove_file(file);
                tracing::debug!("Deleted file that didn't exist in snapshot: {:?}", file);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_git_snapshot_basic() {
        let temp = TempDir::new().unwrap();
        let work_tree = temp.path().to_path_buf();
        let cortex_home = temp.path().join(".cortex");

        // Create a test file
        std::fs::write(work_tree.join("test.txt"), "hello").unwrap();

        let snapshot = GitSnapshot::new(&cortex_home, "test-project", work_tree.clone()).unwrap();

        // Track initial state
        let hash1 = snapshot.track().unwrap();
        assert!(!hash1.is_empty());

        // Modify file
        std::fs::write(work_tree.join("test.txt"), "world").unwrap();

        // Track new state
        let hash2 = snapshot.track().unwrap();
        assert_ne!(hash1, hash2);

        // Check changed files
        let changed = snapshot.changed_files(&hash1).unwrap();
        assert!(changed.iter().any(|p| p.ends_with("test.txt")));
    }
}
