#![allow(warnings, clippy::all)]
//! Snapshot and revert functionality for Cortex CLI.
//!
//! Provides automatic snapshots before file modifications and the ability
//! to revert to previous states.

pub mod diff;
pub mod revert;
pub mod snapshot;
pub mod storage;

pub use diff::{DiffHunk, FileDiff};
pub use revert::{RevertManager, RevertPoint};
pub use snapshot::{Snapshot, SnapshotManager};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SnapshotError {
    #[error("Snapshot not found: {0}")]
    NotFound(String),
    #[error("Git error: {0}")]
    Git(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to create snapshot: {0}")]
    CreateFailed(String),
    #[error("Failed to restore snapshot: {0}")]
    RestoreFailed(String),
    #[error("Git command '{command}' timed out after {timeout_secs}s")]
    GitTimeout { command: String, timeout_secs: u64 },
}

pub type Result<T> = std::result::Result<T, SnapshotError>;
