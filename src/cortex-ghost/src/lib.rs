//! Ghost commits for automatic undo in Cortex CLI.
//!
//! Creates invisible git commits at each turn that can be used
//! to undo changes without polluting the git history.

pub mod config;
pub mod ghost_commit;
pub mod restore;

pub use config::GhostConfig;
pub use ghost_commit::{GhostCommit, GhostCommitManager, GhostSnapshotReport};
pub use restore::restore_ghost_commit;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum GhostError {
    #[error("Not a git repository: {0}")]
    NotGitRepo(String),
    #[error("Git command failed: {0}")]
    GitFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("No ghost commit found for turn: {0}")]
    NotFound(String),
    #[error("Restore failed: {0}")]
    RestoreFailed(String),
    #[error("Git command '{command}' timed out after {timeout_secs}s")]
    GitTimeout { command: String, timeout_secs: u64 },
}

pub type Result<T> = std::result::Result<T, GhostError>;
