//! Shell environment snapshotting for Cortex CLI.
//!
//! This crate captures and restores shell state (variables, functions, aliases, options)
//! to avoid re-running login scripts for each command execution.
//!
//! # Features
//!
//! - Capture shell state (Zsh, Bash, Sh)
//! - Restore state in new shell sessions
//! - Automatic cleanup of stale snapshots
//! - Validation of snapshot integrity
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    ShellSnapshot                             │
//! │  ┌─────────────────────────────────────────────────────────┐│
//! │  │  path: PathBuf (snapshot file location)                  ││
//! │  │  shell_type: ShellType (zsh, bash, sh)                  ││
//! │  │  session_id: ThreadId (owning session)                  ││
//! │  │  created_at: DateTime (creation time)                   ││
//! │  └─────────────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_shell_snapshot::{ShellSnapshot, ShellType};
//!
//! // Create a snapshot
//! let snapshot = ShellSnapshot::capture(
//!     ShellType::Zsh,
//!     "/path/to/cortex_home",
//!     session_id,
//! ).await?;
//!
//! // Later, restore the snapshot
//! let restore_script = snapshot.restore_script()?;
//! ```

pub mod capture;
pub mod cleanup;
pub mod config;
pub mod scripts;
pub mod shell_type;
pub mod snapshot;

pub use capture::{CaptureOptions, capture_shell_state};
pub use cleanup::{SNAPSHOT_RETENTION, cleanup_stale_snapshots};
pub use config::SnapshotConfig;
pub use shell_type::ShellType;
pub use snapshot::{ShellSnapshot, SnapshotMetadata};

use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// Default timeout for snapshot capture.
pub const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default retention period for snapshots (7 days).
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(60 * 60 * 24 * 7);

/// Snapshot directory name.
pub const SNAPSHOT_DIR: &str = "shell_snapshots";

/// Variables to exclude from export (always changing, security, etc.).
pub const EXCLUDED_EXPORT_VARS: &[&str] = &[
    "PWD",
    "OLDPWD",
    "_",
    "SHLVL",
    "RANDOM",
    "LINENO",
    "SECONDS",
    "HISTCMD",
    "BASH_COMMAND",
    "COLUMNS",
    "LINES",
    // Security-sensitive
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "GPG_AGENT_INFO",
];

/// Errors for shell snapshotting.
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// Snapshot file not found.
    #[error("Snapshot not found: {0}")]
    NotFound(PathBuf),

    /// Invalid snapshot format.
    #[error("Invalid snapshot format: {0}")]
    InvalidFormat(String),

    /// Shell type not supported.
    #[error("Unsupported shell type: {0}")]
    UnsupportedShell(String),

    /// Capture timeout.
    #[error("Snapshot capture timed out")]
    Timeout,

    /// Capture failed.
    #[error("Snapshot capture failed: {0}")]
    CaptureFailed(String),

    /// Validation failed.
    #[error("Snapshot validation failed: {0}")]
    ValidationFailed(String),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, SnapshotError>;
