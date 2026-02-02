//! Git Hook Types.
//!
//! Defines the supported git hook types and their metadata.

use serde::{Deserialize, Serialize};

/// Supported git hook types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitHook {
    /// Pre-commit: runs before commit is created
    PreCommit,
    /// Prepare-commit-msg: runs before commit message editor
    PrepareCommitMsg,
    /// Commit-msg: validates commit message
    CommitMsg,
    /// Post-commit: runs after commit is created
    PostCommit,
    /// Pre-push: runs before push to remote
    PrePush,
    /// Pre-rebase: runs before rebase starts
    PreRebase,
}

impl GitHook {
    /// Get the hook filename.
    pub fn filename(&self) -> &'static str {
        match self {
            Self::PreCommit => "pre-commit",
            Self::PrepareCommitMsg => "prepare-commit-msg",
            Self::CommitMsg => "commit-msg",
            Self::PostCommit => "post-commit",
            Self::PrePush => "pre-push",
            Self::PreRebase => "pre-rebase",
        }
    }

    /// Get a description of the hook.
    pub fn description(&self) -> &'static str {
        match self {
            Self::PreCommit => "Runs before commit is created",
            Self::PrepareCommitMsg => "Prepares commit message before editor opens",
            Self::CommitMsg => "Validates commit message format",
            Self::PostCommit => "Runs after commit is created",
            Self::PrePush => "Runs before pushing to remote",
            Self::PreRebase => "Runs before rebase starts",
        }
    }

    /// Parse hook from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "pre-commit" => Some(Self::PreCommit),
            "prepare-commit-msg" => Some(Self::PrepareCommitMsg),
            "commit-msg" => Some(Self::CommitMsg),
            "post-commit" => Some(Self::PostCommit),
            "pre-push" => Some(Self::PrePush),
            "pre-rebase" => Some(Self::PreRebase),
            _ => None,
        }
    }

    /// Get all hook types.
    pub fn all() -> Vec<Self> {
        vec![
            Self::PreCommit,
            Self::PrepareCommitMsg,
            Self::CommitMsg,
            Self::PostCommit,
            Self::PrePush,
            Self::PreRebase,
        ]
    }
}

impl std::fmt::Display for GitHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.filename())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_filename() {
        assert_eq!(GitHook::PreCommit.filename(), "pre-commit");
        assert_eq!(GitHook::PrepareCommitMsg.filename(), "prepare-commit-msg");
        assert_eq!(GitHook::CommitMsg.filename(), "commit-msg");
    }

    #[test]
    fn test_hook_from_str() {
        assert_eq!(GitHook::from_str("pre-commit"), Some(GitHook::PreCommit));
        assert_eq!(GitHook::from_str("pre_commit"), Some(GitHook::PreCommit));
        assert_eq!(GitHook::from_str("PRE-COMMIT"), Some(GitHook::PreCommit));
        assert_eq!(GitHook::from_str("invalid"), None);
    }
}
