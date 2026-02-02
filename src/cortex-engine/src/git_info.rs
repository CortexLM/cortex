//! Git information utilities.
//!
//! Provides utilities for extracting git repository information.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{CortexError, Result};

/// Git repository information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitInfo {
    /// Repository root directory.
    pub root: PathBuf,
    /// Current branch name.
    pub branch: Option<String>,
    /// Current commit hash.
    pub commit: Option<String>,
    /// Short commit hash.
    pub short_commit: Option<String>,
    /// Commit message.
    pub commit_message: Option<String>,
    /// Author name.
    pub author: Option<String>,
    /// Author email.
    pub author_email: Option<String>,
    /// Remote URL.
    pub remote_url: Option<String>,
    /// Is working tree dirty.
    pub is_dirty: bool,
    /// Number of uncommitted changes.
    pub changes: u32,
    /// Number of untracked files.
    pub untracked: u32,
    /// Tags pointing to current commit.
    pub tags: Vec<String>,
}

impl GitInfo {
    /// Get git info for a directory.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        // Find git root
        let root = find_git_root(path)?;

        // Get branch
        let branch = git_command(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();

        // Get commit
        let commit = git_command(&root, &["rev-parse", "HEAD"]).ok();
        let short_commit = git_command(&root, &["rev-parse", "--short", "HEAD"]).ok();

        // Get commit message
        let commit_message = git_command(&root, &["log", "-1", "--format=%s"]).ok();

        // Get author
        let author = git_command(&root, &["log", "-1", "--format=%an"]).ok();
        let author_email = git_command(&root, &["log", "-1", "--format=%ae"]).ok();

        // Get remote URL
        let remote_url = git_command(&root, &["remote", "get-url", "origin"]).ok();

        // Check if dirty
        let status = git_command(&root, &["status", "--porcelain"]).unwrap_or_default();
        let is_dirty = !status.is_empty();

        // Count changes
        let changes = status.lines().filter(|l| !l.starts_with("??")).count() as u32;
        let untracked = status.lines().filter(|l| l.starts_with("??")).count() as u32;

        // Get tags
        let tags = git_command(&root, &["tag", "--points-at", "HEAD"])
            .map(|t| t.lines().map(std::string::ToString::to_string).collect())
            .unwrap_or_default();

        Ok(Self {
            root,
            branch,
            commit,
            short_commit,
            commit_message,
            author,
            author_email,
            remote_url,
            is_dirty,
            changes,
            untracked,
            tags,
        })
    }

    /// Get current directory git info.
    pub fn current() -> Result<Self> {
        Self::from_path(std::env::current_dir()?)
    }

    /// Check if this is a git repository.
    pub fn is_repo(path: impl AsRef<Path>) -> bool {
        find_git_root(path.as_ref()).is_ok()
    }

    /// Get the repository name from remote URL.
    pub fn repo_name(&self) -> Option<String> {
        self.remote_url.as_ref().and_then(|url| {
            // Handle both SSH and HTTPS URLs
            let name = url.rsplit('/').next()?.trim_end_matches(".git").to_string();
            Some(name)
        })
    }

    /// Get owner/repo from remote URL.
    pub fn owner_repo(&self) -> Option<(String, String)> {
        self.remote_url.as_ref().and_then(|url| {
            // Handle HTTPS: https://github.com/owner/repo.git
            // Handle SSH: git@github.com:owner/repo.git
            let cleaned = url.trim_end_matches(".git");

            let parts: Vec<&str> = if cleaned.contains("://") {
                cleaned.rsplit('/').take(2).collect()
            } else {
                cleaned.rsplit('/').take(2).collect()
            };

            if parts.len() >= 2 {
                Some((parts[1].to_string(), parts[0].to_string()))
            } else {
                None
            }
        })
    }

    /// Get a summary string.
    pub fn summary(&self) -> String {
        let branch = self.branch.as_deref().unwrap_or("detached");
        let commit = self.short_commit.as_deref().unwrap_or("unknown");
        let dirty = if self.is_dirty { "*" } else { "" };
        format!("{branch} @ {commit}{dirty}")
    }
}

/// Find the git root directory.
pub fn find_git_root(path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .map_err(CortexError::Io)?;

    if output.status.success() {
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(PathBuf::from(root))
    } else {
        Err(CortexError::NotFound("Not a git repository".to_string()))
    }
}

/// Run a git command and return trimmed stdout.
fn git_command(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(CortexError::Io)?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(CortexError::Internal(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ))
    }
}

/// Git diff information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiff {
    /// Files changed.
    pub files: Vec<DiffFile>,
    /// Insertions count.
    pub insertions: u32,
    /// Deletions count.
    pub deletions: u32,
}

impl GitDiff {
    /// Get diff for staged changes.
    pub fn staged(path: impl AsRef<Path>) -> Result<Self> {
        Self::get_diff(path, &["diff", "--cached", "--numstat"])
    }

    /// Get diff for unstaged changes.
    pub fn unstaged(path: impl AsRef<Path>) -> Result<Self> {
        Self::get_diff(path, &["diff", "--numstat"])
    }

    /// Get diff between two refs.
    pub fn between(path: impl AsRef<Path>, from: &str, to: &str) -> Result<Self> {
        Self::get_diff(path, &["diff", "--numstat", &format!("{from}..{to}")])
    }

    fn get_diff(path: impl AsRef<Path>, args: &[&str]) -> Result<Self> {
        let root = find_git_root(path.as_ref())?;
        let output = git_command(&root, args)?;

        let mut files = Vec::new();
        let mut insertions = 0u32;
        let mut deletions = 0u32;

        for line in output.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                let ins = parts[0].parse().unwrap_or(0);
                let del = parts[1].parse().unwrap_or(0);
                let path = parts[2].to_string();

                insertions += ins;
                deletions += del;

                files.push(DiffFile {
                    path: PathBuf::from(path),
                    insertions: ins,
                    deletions: del,
                });
            }
        }

        Ok(Self {
            files,
            insertions,
            deletions,
        })
    }

    /// Get the full diff content.
    pub fn content(path: impl AsRef<Path>, staged: bool) -> Result<String> {
        let root = find_git_root(path.as_ref())?;
        if staged {
            git_command(&root, &["diff", "--cached"])
        } else {
            git_command(&root, &["diff"])
        }
    }
}

/// File in a diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffFile {
    /// File path.
    pub path: PathBuf,
    /// Lines inserted.
    pub insertions: u32,
    /// Lines deleted.
    pub deletions: u32,
}

impl DiffFile {
    /// Get total lines changed.
    pub fn total_changes(&self) -> u32 {
        self.insertions + self.deletions
    }
}

/// Git log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommit {
    /// Commit hash.
    pub hash: String,
    /// Short hash.
    pub short_hash: String,
    /// Author name.
    pub author: String,
    /// Author email.
    pub email: String,
    /// Commit date.
    pub date: String,
    /// Commit message.
    pub message: String,
}

impl GitCommit {
    /// Get recent commits.
    pub fn recent(path: impl AsRef<Path>, count: usize) -> Result<Vec<Self>> {
        let root = find_git_root(path.as_ref())?;
        let format = "%H%n%h%n%an%n%ae%n%aI%n%s";
        let output = git_command(
            &root,
            &["log", &format!("-{count}"), &format!("--format={format}")],
        )?;

        let mut commits = Vec::new();
        let lines: Vec<&str> = output.lines().collect();

        for chunk in lines.chunks(6) {
            if chunk.len() == 6 {
                commits.push(GitCommit {
                    hash: chunk[0].to_string(),
                    short_hash: chunk[1].to_string(),
                    author: chunk[2].to_string(),
                    email: chunk[3].to_string(),
                    date: chunk[4].to_string(),
                    message: chunk[5].to_string(),
                });
            }
        }

        Ok(commits)
    }
}

/// Git branch information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitBranch {
    /// Branch name.
    pub name: String,
    /// Is current branch.
    pub is_current: bool,
    /// Is remote branch.
    pub is_remote: bool,
    /// Tracking branch.
    pub tracking: Option<String>,
    /// Commits ahead of tracking.
    pub ahead: u32,
    /// Commits behind tracking.
    pub behind: u32,
}

impl GitBranch {
    /// List all branches.
    pub fn list(path: impl AsRef<Path>) -> Result<Vec<Self>> {
        let root = find_git_root(path.as_ref())?;
        let output = git_command(&root, &["branch", "-a", "--format=%(HEAD)%(refname:short)"])?;

        let mut branches = Vec::new();
        for line in output.lines() {
            let is_current = line.starts_with('*');
            let name = line.trim_start_matches('*').trim().to_string();
            let is_remote = name.starts_with("remotes/");

            branches.push(GitBranch {
                name,
                is_current,
                is_remote,
                tracking: None,
                ahead: 0,
                behind: 0,
            });
        }

        Ok(branches)
    }

    /// Get current branch.
    pub fn current(path: impl AsRef<Path>) -> Result<String> {
        let root = find_git_root(path.as_ref())?;
        git_command(&root, &["rev-parse", "--abbrev-ref", "HEAD"])
    }
}

/// Git stash entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStash {
    /// Stash index.
    pub index: u32,
    /// Stash message.
    pub message: String,
    /// Branch stashed from.
    pub branch: String,
}

impl GitStash {
    /// List stashes.
    pub fn list(path: impl AsRef<Path>) -> Result<Vec<Self>> {
        let root = find_git_root(path.as_ref())?;
        let output = git_command(&root, &["stash", "list"])?;

        let mut stashes = Vec::new();
        for (i, line) in output.lines().enumerate() {
            // Format: stash@{0}: On branch: message
            let parts: Vec<&str> = line.splitn(3, ": ").collect();
            if parts.len() >= 2 {
                let branch = parts[1].trim_start_matches("On ").to_string();
                let message = parts.get(2).unwrap_or(&"").to_string();

                stashes.push(GitStash {
                    index: i as u32,
                    message,
                    branch,
                });
            }
        }

        Ok(stashes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_info_repo_name() {
        let info = GitInfo {
            root: PathBuf::from("/test"),
            remote_url: Some("https://github.com/owner/repo.git".to_string()),
            branch: None,
            commit: None,
            short_commit: None,
            commit_message: None,
            author: None,
            author_email: None,
            is_dirty: false,
            changes: 0,
            untracked: 0,
            tags: vec![],
        };

        assert_eq!(info.repo_name(), Some("repo".to_string()));
    }

    #[test]
    fn test_git_info_summary() {
        let info = GitInfo {
            root: PathBuf::from("/test"),
            branch: Some("main".to_string()),
            short_commit: Some("abc123".to_string()),
            is_dirty: true,
            remote_url: None,
            commit: None,
            commit_message: None,
            author: None,
            author_email: None,
            changes: 1,
            untracked: 0,
            tags: vec![],
        };

        assert_eq!(info.summary(), "main @ abc123*");
    }

    #[test]
    fn test_diff_file() {
        let file = DiffFile {
            path: PathBuf::from("test.rs"),
            insertions: 10,
            deletions: 5,
        };

        assert_eq!(file.total_changes(), 15);
    }
}
