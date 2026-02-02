//! Error types for cortex-update.

use std::path::PathBuf;
use thiserror::Error;

/// Result type for update operations.
pub type UpdateResult<T> = std::result::Result<T, UpdateError>;

/// Errors that can occur during update operations.
#[derive(Debug, Error)]
pub enum UpdateError {
    // Network errors
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Failed to connect to update server: {message}")]
    ConnectionFailed { message: String },

    #[error("Update server returned error {status}: {message}")]
    ServerError { status: u16, message: String },

    // Version errors
    #[error("Invalid version format: {version}")]
    InvalidVersion { version: String },

    #[error("Current version {current} is newer than latest {latest}")]
    CurrentVersionNewer { current: String, latest: String },

    #[error("Version {version} not found")]
    VersionNotFound { version: String },

    // Download errors
    #[error("Download failed: {message}")]
    DownloadFailed { message: String },

    #[error("No asset available for platform {platform}")]
    NoPlatformAsset { platform: String },

    // Verification errors
    #[error("SHA256 verification failed: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("Signature verification failed")]
    SignatureInvalid,

    // Installation errors
    #[error("Installation failed: {message}")]
    InstallFailed { message: String },

    #[error("Unsupported installation method: {method}")]
    UnsupportedMethod { method: String },

    #[error("Package manager command failed: {command} (exit code: {code})")]
    CommandFailed { command: String, code: i32 },

    #[error("Permission denied: {path}")]
    PermissionDenied { path: PathBuf },

    #[error("Binary replacement failed: {message}")]
    ReplaceFailed { message: String },

    #[error("Update requires restart: {message}")]
    RequiresRestart { message: String },

    // Archive errors
    #[error("Failed to extract archive: {message}")]
    ExtractionFailed { message: String },

    #[error("Binary not found in archive")]
    BinaryNotFound,

    // File system errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to create temp directory")]
    TempDirFailed,

    // Config errors
    #[error("Failed to load config: {message}")]
    ConfigError { message: String },

    #[error("Failed to save cache: {message}")]
    CacheError { message: String },

    // Serialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    // Cancelled
    #[error("Update cancelled by user")]
    Cancelled,
}

impl UpdateError {
    /// Check if this error is retriable.
    pub fn is_retriable(&self) -> bool {
        match self {
            Self::Network(_) | Self::ConnectionFailed { .. } => true,
            Self::ServerError { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Check if this error is a network error.
    pub fn is_network_error(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::ConnectionFailed { .. } | Self::ServerError { .. }
        )
    }
}
