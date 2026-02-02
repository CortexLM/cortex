//! Ghost commit creation and management.

use crate::{GhostConfig, GhostError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
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

/// Execute a git command with timeout
async fn git_command_with_timeout(args: &[&str], cwd: &PathBuf) -> Result<std::process::Output> {
    let timeout = get_git_timeout();

    let future = Command::new("git").args(args).current_dir(cwd).output();

    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result.map_err(GhostError::Io),
        Err(_) => Err(GhostError::GitTimeout {
            command: format!("git {}", args.join(" ")),
            timeout_secs: timeout.as_secs(),
        }),
    }
}

/// Execute a git command with timeout and return status
async fn git_status_with_timeout(args: &[&str], cwd: &PathBuf) -> Result<std::process::ExitStatus> {
    let timeout = get_git_timeout();

    let future = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result.map_err(GhostError::Io),
        Err(_) => Err(GhostError::GitTimeout {
            command: format!("git {}", args.join(" ")),
            timeout_secs: timeout.as_secs(),
        }),
    }
}

/// A ghost commit that can be used for undo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhostCommit {
    /// Commit SHA.
    pub sha: String,
    /// Turn ID this commit is associated with.
    pub turn_id: String,
    /// Session ID.
    pub session_id: String,
    /// Message ID (if any).
    pub message_id: Option<String>,
    /// Timestamp.
    pub created_at: DateTime<Utc>,
    /// Description.
    pub description: Option<String>,
    /// Files included in snapshot.
    pub files: Vec<String>,
}

/// Report of ghost snapshot creation.
#[derive(Debug, Default, Clone)]
pub struct GhostSnapshotReport {
    /// Large untracked directories skipped.
    pub skipped_large_dirs: Vec<(PathBuf, i64)>,
    /// Large files skipped.
    pub skipped_large_files: Vec<(PathBuf, i64)>,
    /// Total files included.
    pub files_included: usize,
}

/// Manager for ghost commits.
pub struct GhostCommitManager {
    repo_path: PathBuf,
    config: GhostConfig,
    commits: Vec<GhostCommit>,
}

impl GhostCommitManager {
    pub fn new(repo_path: impl Into<PathBuf>, config: GhostConfig) -> Self {
        Self {
            repo_path: repo_path.into(),
            config,
            commits: Vec::new(),
        }
    }

    /// Check if the path is a git repository.
    pub async fn is_git_repo(&self) -> bool {
        git_status_with_timeout(&["rev-parse", "--git-dir"], &self.repo_path)
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Create a ghost commit for the current state.
    pub async fn create_ghost_commit(
        &mut self,
        session_id: &str,
        turn_id: &str,
        message_id: Option<&str>,
        description: Option<&str>,
    ) -> Result<(GhostCommit, GhostSnapshotReport)> {
        if !self.config.enabled {
            return Err(GhostError::GitFailed("Ghost commits disabled".into()));
        }

        if !self.is_git_repo().await {
            return Err(GhostError::NotGitRepo(self.repo_path.display().to_string()));
        }

        let mut report = GhostSnapshotReport::default();

        // Get list of files to include
        let files = self.collect_files_for_snapshot(&mut report).await?;
        report.files_included = files.len();

        // Stage all files for the ghost commit
        self.stage_files(&files).await?;

        // Create the ghost commit
        let commit_msg = format!(
            "ghost: {} - {}",
            turn_id,
            description.unwrap_or("automatic snapshot")
        );

        let sha = self.create_commit(&commit_msg).await?;

        let ghost = GhostCommit {
            sha: sha.clone(),
            turn_id: turn_id.to_string(),
            session_id: session_id.to_string(),
            message_id: message_id.map(String::from),
            created_at: Utc::now(),
            description: description.map(String::from),
            files: files
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
        };

        self.commits.push(ghost.clone());

        info!("Created ghost commit {} for turn {}", sha, turn_id);
        Ok((ghost, report))
    }

    /// Get the most recent ghost commit.
    pub fn get_latest(&self) -> Option<&GhostCommit> {
        self.commits.last()
    }

    /// Get ghost commit by turn ID.
    pub fn get_by_turn(&self, turn_id: &str) -> Option<&GhostCommit> {
        self.commits.iter().rev().find(|c| c.turn_id == turn_id)
    }

    /// Get all ghost commits for a session.
    pub fn get_session_commits(&self, session_id: &str) -> Vec<&GhostCommit> {
        self.commits
            .iter()
            .filter(|c| c.session_id == session_id)
            .collect()
    }

    /// Collect files to include in the snapshot.
    async fn collect_files_for_snapshot(
        &self,
        report: &mut GhostSnapshotReport,
    ) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let ignored: HashSet<_> = self.config.ignored_dirs.iter().collect();

        // Get tracked files
        let tracked = self.get_tracked_files().await?;
        files.extend(tracked);

        // Get untracked files (respecting limits)
        let untracked = self.get_untracked_files().await?;

        for file in untracked {
            let file_path = self.repo_path.join(&file);

            // Check if in ignored directory
            if file.components().any(|c| {
                if let std::path::Component::Normal(name) = c {
                    ignored.contains(&name.to_string_lossy().to_string())
                } else {
                    false
                }
            }) {
                continue;
            }

            // Check file size
            if let Ok(metadata) = tokio::fs::metadata(&file_path).await {
                if metadata.is_file() {
                    let size = metadata.len() as i64;
                    if size > self.config.max_untracked_file_size {
                        report.skipped_large_files.push((file.clone(), size));
                        continue;
                    }
                }
            }

            files.push(file);
        }

        Ok(files)
    }

    /// Get list of tracked files.
    async fn get_tracked_files(&self) -> Result<Vec<PathBuf>> {
        let output = git_command_with_timeout(&["ls-files"], &self.repo_path).await?;

        if !output.status.success() {
            return Err(GhostError::GitFailed("Failed to list tracked files".into()));
        }

        let files = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(PathBuf::from)
            .collect();

        Ok(files)
    }

    /// Get list of untracked files.
    async fn get_untracked_files(&self) -> Result<Vec<PathBuf>> {
        let output = git_command_with_timeout(
            &["ls-files", "--others", "--exclude-standard"],
            &self.repo_path,
        )
        .await?;

        if !output.status.success() {
            return Err(GhostError::GitFailed(
                "Failed to list untracked files".into(),
            ));
        }

        let files = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(PathBuf::from)
            .collect();

        Ok(files)
    }

    /// Stage files for commit.
    async fn stage_files(&self, files: &[PathBuf]) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }

        // Use git add with --intent-to-add for new files, then add all
        let output = git_command_with_timeout(&["add", "-A"], &self.repo_path).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Git add warning: {}", stderr);
        }

        Ok(())
    }

    /// Create a commit and return its SHA.
    async fn create_commit(&self, message: &str) -> Result<String> {
        // Check if there are changes to commit
        let status = git_command_with_timeout(&["status", "--porcelain"], &self.repo_path).await?;

        if status.stdout.is_empty() {
            // No changes, create empty commit
            let output = git_command_with_timeout(
                &["commit", "--allow-empty", "-m", message],
                &self.repo_path,
            )
            .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(GhostError::GitFailed(stderr.to_string()));
            }
        } else {
            // Commit changes
            let output =
                git_command_with_timeout(&["commit", "-m", message], &self.repo_path).await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(GhostError::GitFailed(stderr.to_string()));
            }
        }

        // Get the commit SHA
        let sha_output = git_command_with_timeout(&["rev-parse", "HEAD"], &self.repo_path).await?;

        let sha = String::from_utf8_lossy(&sha_output.stdout)
            .trim()
            .to_string();
        Ok(sha)
    }

    /// Load existing ghost commits from git log.
    pub async fn load_history(&mut self) -> Result<()> {
        let output = git_command_with_timeout(
            &["log", "--oneline", "--all", "--grep=ghost:"],
            &self.repo_path,
        )
        .await?;

        if output.status.success() {
            let log = String::from_utf8_lossy(&output.stdout);
            for line in log.lines() {
                if let Some((sha, rest)) = line.split_once(' ') {
                    if rest.starts_with("ghost:") {
                        // Parse turn_id from message
                        if let Some(turn_id) = rest
                            .strip_prefix("ghost: ")
                            .and_then(|s| s.split(" - ").next())
                        {
                            self.commits.push(GhostCommit {
                                sha: sha.to_string(),
                                turn_id: turn_id.to_string(),
                                session_id: String::new(),
                                message_id: None,
                                created_at: Utc::now(),
                                description: None,
                                files: Vec::new(),
                            });
                        }
                    }
                }
            }
        }

        debug!("Loaded {} ghost commits from history", self.commits.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_ghost_commit_manager() {
        let dir = tempdir().unwrap();

        // Initialize git repo
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let config = GhostConfig::default();
        let mut manager = GhostCommitManager::new(dir.path(), config);

        assert!(manager.is_git_repo().await);

        // Create a file
        tokio::fs::write(dir.path().join("test.txt"), "hello")
            .await
            .unwrap();

        // Create ghost commit
        let (ghost, report) = manager
            .create_ghost_commit("session-1", "turn-1", None, Some("test commit"))
            .await
            .unwrap();

        assert!(!ghost.sha.is_empty());
        assert_eq!(ghost.turn_id, "turn-1");
        assert!(report.files_included > 0);
    }
}
