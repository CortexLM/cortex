//! Review target types.

use serde::{Deserialize, Serialize};

/// Target for code review.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum ReviewTarget {
    /// Review uncommitted changes (staged, unstaged, untracked).
    #[default]
    UncommittedChanges,
    /// Review changes against a base branch.
    BaseBranch {
        /// The base branch name (e.g., "main", "master").
        branch: String,
        /// The merge base SHA (computed automatically).
        #[serde(skip_serializing_if = "Option::is_none")]
        merge_base: Option<String>,
    },
    /// Review a specific commit.
    Commit {
        /// Commit SHA.
        sha: String,
        /// Commit title (first line of message).
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// Review a range of commits.
    CommitRange {
        /// Start SHA (exclusive).
        from: String,
        /// End SHA (inclusive).
        to: String,
    },
    /// Custom review with user-provided instructions.
    Custom {
        /// Custom review instructions.
        instructions: String,
    },
}

impl ReviewTarget {
    /// Create a target for uncommitted changes.
    pub fn uncommitted() -> Self {
        Self::UncommittedChanges
    }

    /// Create a target for changes against a base branch.
    pub fn against_branch(branch: impl Into<String>) -> Self {
        Self::BaseBranch {
            branch: branch.into(),
            merge_base: None,
        }
    }

    /// Create a target for a specific commit.
    pub fn commit(sha: impl Into<String>) -> Self {
        Self::Commit {
            sha: sha.into(),
            title: None,
        }
    }

    /// Create a target for a commit range.
    pub fn range(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self::CommitRange {
            from: from.into(),
            to: to.into(),
        }
    }

    /// Create a custom review target.
    pub fn custom(instructions: impl Into<String>) -> Self {
        Self::Custom {
            instructions: instructions.into(),
        }
    }

    /// Get a user-friendly description.
    pub fn description(&self) -> String {
        match self {
            Self::UncommittedChanges => "current uncommitted changes".to_string(),
            Self::BaseBranch { branch, .. } => format!("changes against '{}'", branch),
            Self::Commit { sha, title } => {
                let short_sha: String = sha.chars().take(7).collect();
                if let Some(title) = title {
                    format!("commit {}: {}", short_sha, title)
                } else {
                    format!("commit {}", short_sha)
                }
            }
            Self::CommitRange { from, to } => {
                let short_from: String = from.chars().take(7).collect();
                let short_to: String = to.chars().take(7).collect();
                format!("commits {}..{}", short_from, short_to)
            }
            Self::Custom { .. } => "custom review".to_string(),
        }
    }
}
