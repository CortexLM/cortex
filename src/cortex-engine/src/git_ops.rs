//! Git operations.
//!
//! Provides comprehensive Git operations for repository
//! management, status checking, and diff generation.

use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{CortexError, Result};

/// Git status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    /// Is a git repository.
    pub is_repo: bool,
    /// Current branch.
    pub branch: Option<String>,
    /// Remote tracking branch.
    pub tracking: Option<String>,
    /// Commits ahead of remote.
    pub ahead: u32,
    /// Commits behind remote.
    pub behind: u32,
    /// Staged files.
    pub staged: Vec<FileStatus>,
    /// Modified files.
    pub modified: Vec<FileStatus>,
    /// Untracked files.
    pub untracked: Vec<PathBuf>,
    /// Has stash.
    pub stash_count: u32,
    /// Is clean.
    pub is_clean: bool,
}

impl GitStatus {
    /// Check if working tree is dirty.
    pub fn is_dirty(&self) -> bool {
        !self.is_clean
    }

    /// Get total changed files.
    pub fn changed_count(&self) -> usize {
        self.staged.len() + self.modified.len() + self.untracked.len()
    }

    /// Format as short status.
    pub fn short_status(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref branch) = self.branch {
            parts.push(branch.clone());
        }

        if self.ahead > 0 {
            parts.push(format!("↑{}", self.ahead));
        }

        if self.behind > 0 {
            parts.push(format!("↓{}", self.behind));
        }

        if !self.staged.is_empty() {
            parts.push(format!("+{}", self.staged.len()));
        }

        if !self.modified.is_empty() {
            parts.push(format!("~{}", self.modified.len()));
        }

        if !self.untracked.is_empty() {
            parts.push(format!("?{}", self.untracked.len()));
        }

        parts.join(" ")
    }
}

/// File status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStatus {
    /// File path.
    pub path: PathBuf,
    /// Status code.
    pub status: FileStatusCode,
    /// Old path (for renames).
    pub old_path: Option<PathBuf>,
}

/// File status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatusCode {
    /// Added.
    Added,
    /// Modified.
    Modified,
    /// Deleted.
    Deleted,
    /// Renamed.
    Renamed,
    /// Copied.
    Copied,
    /// Unmerged.
    Unmerged,
    /// Untracked.
    Untracked,
}

impl FileStatusCode {
    /// Get short code.
    pub fn code(&self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::Unmerged => 'U',
            Self::Untracked => '?',
        }
    }

    /// Parse from git status code.
    pub fn from_code(c: char) -> Option<Self> {
        match c {
            'A' => Some(Self::Added),
            'M' => Some(Self::Modified),
            'D' => Some(Self::Deleted),
            'R' => Some(Self::Renamed),
            'C' => Some(Self::Copied),
            'U' => Some(Self::Unmerged),
            '?' => Some(Self::Untracked),
            _ => None,
        }
    }
}

/// Git commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommit {
    /// Commit hash.
    pub hash: String,
    /// Short hash.
    pub short_hash: String,
    /// Author name.
    pub author: String,
    /// Author email.
    pub author_email: String,
    /// Commit date.
    pub date: u64,
    /// Commit message.
    pub message: String,
    /// Parent hashes.
    pub parents: Vec<String>,
}

impl GitCommit {
    /// Get first line of message.
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

/// Git diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiff {
    /// Files changed.
    pub files: Vec<DiffFile>,
    /// Total additions.
    pub additions: u32,
    /// Total deletions.
    pub deletions: u32,
    /// Raw diff.
    pub raw: String,
}

impl GitDiff {
    /// Get file count.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Get total lines changed.
    pub fn lines_changed(&self) -> u32 {
        self.additions + self.deletions
    }
}

/// Diff file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffFile {
    /// File path.
    pub path: PathBuf,
    /// Old path (for renames).
    pub old_path: Option<PathBuf>,
    /// Additions.
    pub additions: u32,
    /// Deletions.
    pub deletions: u32,
    /// Is binary.
    pub is_binary: bool,
    /// Status.
    pub status: FileStatusCode,
}

/// Git operations.
pub struct Git {
    /// Repository path.
    repo_path: PathBuf,
}

impl Git {
    /// Create a new Git instance.
    pub fn new(repo_path: impl Into<PathBuf>) -> Self {
        Self {
            repo_path: repo_path.into(),
        }
    }

    /// Create for current directory.
    pub fn current_dir() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Check if is a git repository.
    pub fn is_repo(&self) -> bool {
        self.repo_path.join(".git").exists()
    }

    /// Get repository root.
    pub fn root(&self) -> Result<PathBuf> {
        let output = self.run(&["rev-parse", "--show-toplevel"])?;
        Ok(PathBuf::from(output.trim()))
    }

    /// Get current branch.
    pub fn branch(&self) -> Result<String> {
        let output = self.run(&["rev-parse", "--abbrev-ref", "HEAD"])?;
        Ok(output.trim().to_string())
    }

    /// Get status.
    pub fn status(&self) -> Result<GitStatus> {
        if !self.is_repo() {
            return Ok(GitStatus {
                is_repo: false,
                branch: None,
                tracking: None,
                ahead: 0,
                behind: 0,
                staged: Vec::new(),
                modified: Vec::new(),
                untracked: Vec::new(),
                stash_count: 0,
                is_clean: true,
            });
        }

        let branch = self.branch().ok();
        let tracking = self.tracking_branch().ok();
        let (ahead, behind) = self.ahead_behind().unwrap_or((0, 0));
        let stash_count = self.stash_count().unwrap_or(0);

        let output = self.run(&["status", "--porcelain", "-z"])?;

        let mut staged = Vec::new();
        let mut modified = Vec::new();
        let mut untracked = Vec::new();

        for entry in output.split('\0') {
            if entry.len() < 3 {
                continue;
            }

            let index_status = entry.chars().nth(0);
            let worktree_status = entry.chars().nth(1);
            let path = PathBuf::from(entry[3..].to_string());

            // Index (staged) status
            if let Some(c) = index_status
                && c != ' '
                && c != '?'
                && let Some(status) = FileStatusCode::from_code(c)
            {
                staged.push(FileStatus {
                    path: path.clone(),
                    status,
                    old_path: None,
                });
            }

            // Worktree status
            if let Some(c) = worktree_status
                && c != ' '
            {
                if c == '?' {
                    untracked.push(path.clone());
                } else if let Some(status) = FileStatusCode::from_code(c) {
                    modified.push(FileStatus {
                        path: path.clone(),
                        status,
                        old_path: None,
                    });
                }
            }
        }

        let is_clean = staged.is_empty() && modified.is_empty() && untracked.is_empty();

        Ok(GitStatus {
            is_repo: true,
            branch,
            tracking,
            ahead,
            behind,
            staged,
            modified,
            untracked,
            stash_count,
            is_clean,
        })
    }

    /// Get tracking branch.
    fn tracking_branch(&self) -> Result<String> {
        let output = self.run(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])?;
        Ok(output.trim().to_string())
    }

    /// Get ahead/behind count.
    fn ahead_behind(&self) -> Result<(u32, u32)> {
        let output = self.run(&["rev-list", "--left-right", "--count", "@{u}...HEAD"])?;
        let parts: Vec<&str> = output.trim().split('\t').collect();

        if parts.len() == 2 {
            let behind = parts[0].parse().unwrap_or(0);
            let ahead = parts[1].parse().unwrap_or(0);
            Ok((ahead, behind))
        } else {
            Ok((0, 0))
        }
    }

    /// Get stash count.
    fn stash_count(&self) -> Result<u32> {
        let output = self.run(&["stash", "list"])?;
        Ok(output.lines().count() as u32)
    }

    /// Get diff.
    pub fn diff(&self, staged: bool) -> Result<GitDiff> {
        let args = if staged {
            vec!["diff", "--cached", "--stat", "-p"]
        } else {
            vec!["diff", "--stat", "-p"]
        };

        let raw = self.run(&args)?;

        // Parse stat
        let stat_args = if staged {
            vec!["diff", "--cached", "--numstat"]
        } else {
            vec!["diff", "--numstat"]
        };

        let stat_output = self.run(&stat_args)?;

        let mut files = Vec::new();
        let mut total_additions = 0u32;
        let mut total_deletions = 0u32;

        for line in stat_output.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                let additions = parts[0].parse().unwrap_or(0);
                let deletions = parts[1].parse().unwrap_or(0);
                let path = PathBuf::from(parts[2]);

                total_additions += additions;
                total_deletions += deletions;

                files.push(DiffFile {
                    path,
                    old_path: None,
                    additions,
                    deletions,
                    is_binary: parts[0] == "-",
                    status: FileStatusCode::Modified,
                });
            }
        }

        Ok(GitDiff {
            files,
            additions: total_additions,
            deletions: total_deletions,
            raw,
        })
    }

    /// Get commit log.
    pub fn log(&self, count: usize) -> Result<Vec<GitCommit>> {
        let format = "%H%n%h%n%an%n%ae%n%at%n%s%n%P%n---";
        let count_arg = format!("-{count}");
        let format_arg = format!("--format={format}");
        let args = vec!["log", &count_arg, &format_arg];

        let output = self.run(&args)?;
        let mut commits = Vec::new();

        for entry in output.split("---\n") {
            let lines: Vec<&str> = entry.lines().collect();
            if lines.len() >= 6 {
                commits.push(GitCommit {
                    hash: lines[0].to_string(),
                    short_hash: lines[1].to_string(),
                    author: lines[2].to_string(),
                    author_email: lines[3].to_string(),
                    date: lines[4].parse().unwrap_or(0),
                    message: lines[5].to_string(),
                    parents: if lines.len() > 6 {
                        lines[6]
                            .split_whitespace()
                            .map(std::string::ToString::to_string)
                            .collect()
                    } else {
                        Vec::new()
                    },
                });
            }
        }

        Ok(commits)
    }

    /// Stage files.
    pub fn add(&self, paths: &[&str]) -> Result<()> {
        let mut args = vec!["add"];
        args.extend(paths);
        self.run(&args)?;
        Ok(())
    }

    /// Stage all.
    pub fn add_all(&self) -> Result<()> {
        self.run(&["add", "-A"])?;
        Ok(())
    }

    /// Commit.
    pub fn commit(&self, message: &str) -> Result<String> {
        let output = self.run(&["commit", "-m", message])?;

        // Extract commit hash
        if let Some(hash) = output.lines().next()
            && hash.contains('[')
            && hash.contains(']')
            && let Some(short) = hash.split_whitespace().nth(1)
        {
            return Ok(short.trim_end_matches(']').to_string());
        }

        // Get HEAD hash
        self.run(&["rev-parse", "--short", "HEAD"])
            .map(|s| s.trim().to_string())
    }

    /// Push.
    pub fn push(&self, remote: &str, branch: &str) -> Result<()> {
        self.run(&["push", remote, branch])?;
        Ok(())
    }

    /// Pull.
    pub fn pull(&self, remote: &str, branch: &str) -> Result<()> {
        self.run(&["pull", remote, branch])?;
        Ok(())
    }

    /// Fetch.
    pub fn fetch(&self, remote: &str) -> Result<()> {
        self.run(&["fetch", remote])?;
        Ok(())
    }

    /// Checkout branch.
    pub fn checkout(&self, branch: &str) -> Result<()> {
        self.run(&["checkout", branch])?;
        Ok(())
    }

    /// Create and checkout new branch.
    pub fn checkout_new(&self, branch: &str) -> Result<()> {
        self.run(&["checkout", "-b", branch])?;
        Ok(())
    }

    /// Stash.
    pub fn stash(&self) -> Result<()> {
        self.run(&["stash"])?;
        Ok(())
    }

    /// Stash pop.
    pub fn stash_pop(&self) -> Result<()> {
        self.run(&["stash", "pop"])?;
        Ok(())
    }

    /// Reset.
    pub fn reset(&self, mode: ResetMode, target: &str) -> Result<()> {
        let mode_arg = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.run(&["reset", mode_arg, target])?;
        Ok(())
    }

    /// Get remotes.
    pub fn remotes(&self) -> Result<Vec<String>> {
        let output = self.run(&["remote"])?;
        Ok(output
            .lines()
            .map(std::string::ToString::to_string)
            .collect())
    }

    /// Get remote URL.
    pub fn remote_url(&self, remote: &str) -> Result<String> {
        let output = self.run(&["remote", "get-url", remote])?;
        Ok(output.trim().to_string())
    }

    /// Run a git command.
    fn run(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .args(args)
            .output()
            .map_err(|e| CortexError::Internal(format!("Failed to run git: {e}")))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(CortexError::Internal(format!("Git error: {stderr}")))
        }
    }
}

/// Reset mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResetMode {
    /// Soft reset.
    Soft,
    /// Mixed reset (default).
    #[default]
    Mixed,
    /// Hard reset.
    Hard,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_status_code() {
        assert_eq!(FileStatusCode::Added.code(), 'A');
        assert_eq!(
            FileStatusCode::from_code('M'),
            Some(FileStatusCode::Modified)
        );
        assert_eq!(FileStatusCode::from_code('X'), None);
    }

    #[test]
    fn test_git_commit_subject() {
        let commit = GitCommit {
            hash: "abc123".to_string(),
            short_hash: "abc".to_string(),
            author: "Test".to_string(),
            author_email: "test@example.com".to_string(),
            date: 0,
            message: "First line\n\nMore details".to_string(),
            parents: Vec::new(),
        };

        assert_eq!(commit.subject(), "First line");
    }

    #[test]
    fn test_git_status_short() {
        let status = GitStatus {
            is_repo: true,
            branch: Some("main".to_string()),
            tracking: Some("origin/main".to_string()),
            ahead: 2,
            behind: 1,
            staged: vec![FileStatus {
                path: PathBuf::from("file.rs"),
                status: FileStatusCode::Added,
                old_path: None,
            }],
            modified: Vec::new(),
            untracked: vec![PathBuf::from("new.txt")],
            stash_count: 0,
            is_clean: false,
        };

        let short = status.short_status();
        assert!(short.contains("main"));
        assert!(short.contains("↑2"));
        assert!(short.contains("↓1"));
    }

    #[test]
    fn test_git_diff_lines() {
        let diff = GitDiff {
            files: vec![DiffFile {
                path: PathBuf::from("file.rs"),
                old_path: None,
                additions: 10,
                deletions: 5,
                is_binary: false,
                status: FileStatusCode::Modified,
            }],
            additions: 10,
            deletions: 5,
            raw: String::new(),
        };

        assert_eq!(diff.lines_changed(), 15);
        assert_eq!(diff.file_count(), 1);
    }
}
