//! Code review functionality for Cortex CLI.
//!
//! Provides the /review command to review:
//! - Uncommitted changes
//! - Changes against a base branch
//! - Specific commits

pub mod prompts;
pub mod review;
pub mod targets;

pub use prompts::build_review_prompt;
pub use review::{ReviewManager, ReviewRequest, ReviewResult};
pub use targets::ReviewTarget;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ReviewError {
    #[error("Not a git repository")]
    NotGitRepo,
    #[error("Git error: {0}")]
    GitError(String),
    #[error("No changes to review")]
    NoChanges,
    #[error("Invalid target: {0}")]
    InvalidTarget(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Git command '{command}' timed out after {timeout_secs}s")]
    GitTimeout { command: String, timeout_secs: u64 },
}

pub type Result<T> = std::result::Result<T, ReviewError>;
