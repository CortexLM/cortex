//! Error types for cortex-storage.

use std::path::PathBuf;
use thiserror::Error;

/// Storage error types.
#[derive(Debug, Error)]
pub enum StorageError {
    /// IO error during file operations.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Session not found.
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    /// Invalid path.
    #[error("Invalid path: {0}")]
    InvalidPath(PathBuf),

    /// Home directory not found.
    #[error("Could not determine home/data directory")]
    HomeDirNotFound,

    /// Storage not initialized.
    #[error("Storage not initialized")]
    NotInitialized,
}

/// Result type for storage operations.
pub type Result<T> = std::result::Result<T, StorageError>;
