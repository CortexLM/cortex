//! Review manager and core functionality.

use crate::{prompts, Result, ReviewError, ReviewTarget};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

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
async fn git_command_with_timeout(
    args: &[&str],
    cwd: &PathBuf,
) -> std::result::Result<std::process::Output, ReviewError> {
    let timeout = get_git_timeout();

    let future = Command::new("git").args(args).current_dir(cwd).output();

    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result.map_err(ReviewError::Io),
        Err(_) => Err(ReviewError::GitTimeout {
            command: format!("git {}", args.join(" ")),
            timeout_secs: timeout.as_secs(),
        }),
    }
}

/// Execute a git command with timeout and return status
async fn git_status_with_timeout(
    args: &[&str],
    cwd: &PathBuf,
) -> std::result::Result<std::process::ExitStatus, ReviewError> {
    let timeout = get_git_timeout();

    let future = Command::new("git").args(args).current_dir(cwd).status();

    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result.map_err(ReviewError::Io),
        Err(_) => Err(ReviewError::GitTimeout {
            command: format!("git {}", args.join(" ")),
            timeout_secs: timeout.as_secs(),
        }),
    }
}

/// A review request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRequest {
    /// Target to review.
    pub target: ReviewTarget,
    /// Custom user hint for display.
    pub user_hint: Option<String>,
    /// Focus areas for the review.
    pub focus_areas: Vec<String>,
}

impl ReviewRequest {
    pub fn new(target: ReviewTarget) -> Self {
        Self {
            target,
            user_hint: None,
            focus_areas: Vec::new(),
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.user_hint = Some(hint.into());
        self
    }

    pub fn with_focus(mut self, focus: impl Into<String>) -> Self {
        self.focus_areas.push(focus.into());
        self
    }
}

/// Result of resolving a review request.
#[derive(Debug, Clone)]
pub struct ResolvedReview {
    /// The target being reviewed.
    pub target: ReviewTarget,
    /// The prompt to send to the AI.
    pub prompt: String,
    /// User-facing description.
    pub description: String,
    /// The diff content (if available).
    pub diff: Option<String>,
    /// Files affected.
    pub files: Vec<String>,
}

/// Result of a review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    /// Target that was reviewed.
    pub target: ReviewTarget,
    /// Findings from the review.
    pub findings: Vec<ReviewFinding>,
    /// Summary.
    pub summary: Option<String>,
}

/// A single finding from a review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
    /// Severity level.
    pub severity: Severity,
    /// File path.
    pub file: Option<String>,
    /// Line number.
    pub line: Option<u32>,
    /// Description of the issue.
    pub description: String,
    /// Suggested fix.
    pub suggestion: Option<String>,
}

/// Severity levels for findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Critical => write!(f, "CRITICAL"),
            Self::High => write!(f, "HIGH"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::Low => write!(f, "LOW"),
            Self::Info => write!(f, "INFO"),
        }
    }
}

/// Manager for code reviews.
pub struct ReviewManager {
    repo_path: PathBuf,
}

impl ReviewManager {
    pub fn new(repo_path: impl Into<PathBuf>) -> Self {
        Self {
            repo_path: repo_path.into(),
        }
    }

    /// Check if path is a git repository.
    pub async fn is_git_repo(&self) -> bool {
        git_command_with_timeout(&["rev-parse", "--git-dir"], &self.repo_path)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Resolve a review request to get the prompt and context.
    pub async fn resolve(&self, request: &ReviewRequest) -> Result<ResolvedReview> {
        if !self.is_git_repo().await {
            return Err(ReviewError::NotGitRepo);
        }

        let mut target = request.target.clone();
        let mut merge_base = None;

        // Resolve merge base for branch targets
        if let ReviewTarget::BaseBranch { branch, .. } = &target {
            merge_base = prompts::get_merge_base(&self.repo_path, branch).await?;
            target = ReviewTarget::BaseBranch {
                branch: branch.clone(),
                merge_base: merge_base.clone(),
            };
        }

        // Get commit title if needed
        if let ReviewTarget::Commit { sha, title: None } = &target {
            let title = prompts::get_commit_title(&self.repo_path, sha).await?;
            target = ReviewTarget::Commit {
                sha: sha.clone(),
                title,
            };
        }

        // Build prompt
        let prompt = prompts::build_review_prompt(&target, merge_base.as_deref());

        // Get diff
        let diff = self.get_diff(&target).await?;
        let files = self.get_affected_files(&target).await?;

        // Build description
        let description = request
            .user_hint
            .clone()
            .unwrap_or_else(|| target.description());

        Ok(ResolvedReview {
            target,
            prompt,
            description,
            diff,
            files,
        })
    }

    /// Get the diff for a target.
    async fn get_diff(&self, target: &ReviewTarget) -> Result<Option<String>> {
        let args: Vec<&str> = match target {
            ReviewTarget::UncommittedChanges => vec!["diff", "HEAD"],
            ReviewTarget::BaseBranch {
                merge_base: Some(base),
                ..
            } => vec!["diff", base],
            ReviewTarget::BaseBranch { branch, .. } => vec!["diff", branch],
            ReviewTarget::Commit { sha, .. } => vec!["show", sha],
            ReviewTarget::CommitRange { from, to } => vec!["diff", from, to],
            ReviewTarget::Custom { .. } => return Ok(None),
        };

        let output = git_command_with_timeout(&args, &self.repo_path).await?;

        if output.status.success() {
            let diff = String::from_utf8_lossy(&output.stdout).to_string();
            if !diff.trim().is_empty() {
                return Ok(Some(diff));
            }
        }

        Ok(None)
    }

    /// Get affected files for a target.
    async fn get_affected_files(&self, target: &ReviewTarget) -> Result<Vec<String>> {
        let args: Vec<&str> = match target {
            ReviewTarget::UncommittedChanges => vec!["diff", "--name-only", "HEAD"],
            ReviewTarget::BaseBranch {
                merge_base: Some(base),
                ..
            } => vec!["diff", "--name-only", base],
            ReviewTarget::BaseBranch { branch, .. } => vec!["diff", "--name-only", branch],
            ReviewTarget::Commit { sha, .. } => {
                vec!["diff-tree", "--no-commit-id", "--name-only", "-r", sha]
            }
            ReviewTarget::CommitRange { from, to } => vec!["diff", "--name-only", from, to],
            ReviewTarget::Custom { .. } => return Ok(Vec::new()),
        };

        let output = git_command_with_timeout(&args, &self.repo_path).await?;

        if output.status.success() {
            let files = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(String::from)
                .collect();
            return Ok(files);
        }

        Ok(Vec::new())
    }

    /// Check if there are changes to review.
    pub async fn has_changes(&self) -> Result<bool> {
        // Check for staged changes
        let staged =
            git_status_with_timeout(&["diff", "--cached", "--quiet"], &self.repo_path).await?;

        if !staged.success() {
            return Ok(true);
        }

        // Check for unstaged changes
        let unstaged = git_status_with_timeout(&["diff", "--quiet"], &self.repo_path).await?;

        if !unstaged.success() {
            return Ok(true);
        }

        // Check for untracked files
        let untracked = git_command_with_timeout(
            &["ls-files", "--others", "--exclude-standard"],
            &self.repo_path,
        )
        .await?;

        Ok(!untracked.stdout.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_review_request() {
        let request = ReviewRequest::new(ReviewTarget::uncommitted())
            .with_hint("Review my changes")
            .with_focus("security");

        assert!(request.user_hint.is_some());
        assert_eq!(request.focus_areas.len(), 1);
    }
}
