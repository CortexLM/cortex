//! Error types for patch operations.

use std::path::PathBuf;
use thiserror::Error;

/// Result type for patch operations.
pub type PatchResult<T> = Result<T, PatchError>;

/// Errors that can occur during patch parsing and application.
#[derive(Debug, Error)]
pub enum PatchError {
    /// Failed to parse the patch format.
    #[error("Failed to parse patch: {message}")]
    ParseError {
        message: String,
        line_number: Option<usize>,
    },

    /// Invalid patch header.
    #[error("Invalid patch header at line {line_number}: {message}")]
    InvalidHeader { message: String, line_number: usize },

    /// Invalid hunk header.
    #[error("Invalid hunk header: {header}")]
    InvalidHunkHeader { header: String },

    /// File not found for patching.
    #[error("File not found: {path}")]
    FileNotFound { path: PathBuf },

    /// Failed to read file.
    #[error("Failed to read file {path}: {source}")]
    ReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to write file.
    #[error("Failed to write file {path}: {source}")]
    WriteError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to create directory.
    #[error("Failed to create directory {path}: {source}")]
    CreateDirError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to delete file.
    #[error("Failed to delete file {path}: {source}")]
    DeleteError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Hunk application failed - context mismatch.
    #[error("Context mismatch at line {line}: expected '{expected}', found '{found}'")]
    ContextMismatch {
        line: usize,
        expected: String,
        found: String,
    },

    /// Hunk could not be located in the file.
    #[error("Could not locate hunk starting at line {original_line} in file {file}")]
    HunkNotFound { file: String, original_line: usize },

    /// Conflict detected - the file has been modified.
    #[error("Conflict detected in {file} at line {line}: {message}")]
    Conflict {
        file: String,
        line: usize,
        message: String,
    },

    /// Multiple hunks conflict with each other.
    #[error("Overlapping hunks detected in {file}")]
    OverlappingHunks { file: String },

    /// Backup operation failed.
    #[error("Backup failed for {path}: {message}")]
    BackupError { path: PathBuf, message: String },

    /// Restore operation failed.
    #[error("Restore failed for {path}: {message}")]
    RestoreError { path: PathBuf, message: String },

    /// Empty patch provided.
    #[error("Empty patch provided")]
    EmptyPatch,

    /// Invalid file path in patch.
    #[error("Invalid file path: {path}")]
    InvalidPath { path: String },

    /// General I/O error.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Multiple errors occurred.
    #[error("Multiple errors occurred:\n{}", .0.iter().map(|e| format!("  - {e}")).collect::<Vec<_>>().join("\n"))]
    MultipleErrors(Vec<PatchError>),
}

impl PatchError {
    /// Create a parse error with optional line number.
    pub fn parse(message: impl Into<String>, line_number: Option<usize>) -> Self {
        Self::ParseError {
            message: message.into(),
            line_number,
        }
    }

    /// Create a file not found error.
    pub fn file_not_found(path: impl Into<PathBuf>) -> Self {
        Self::FileNotFound { path: path.into() }
    }

    /// Create a context mismatch error.
    pub fn context_mismatch(
        line: usize,
        expected: impl Into<String>,
        found: impl Into<String>,
    ) -> Self {
        Self::ContextMismatch {
            line,
            expected: expected.into(),
            found: found.into(),
        }
    }

    /// Create a hunk not found error.
    pub fn hunk_not_found(file: impl Into<String>, original_line: usize) -> Self {
        Self::HunkNotFound {
            file: file.into(),
            original_line,
        }
    }

    /// Create a conflict error.
    pub fn conflict(file: impl Into<String>, line: usize, message: impl Into<String>) -> Self {
        Self::Conflict {
            file: file.into(),
            line,
            message: message.into(),
        }
    }

    /// Check if this is a recoverable error (e.g., context mismatch that might work with fuzzy matching).
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::ContextMismatch { .. } | Self::HunkNotFound { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = PatchError::parse("unexpected token", Some(42));
        assert!(err.to_string().contains("unexpected token"));

        let err = PatchError::file_not_found("/some/path");
        assert!(err.to_string().contains("/some/path"));

        let err = PatchError::context_mismatch(10, "expected line", "actual line");
        assert!(err.to_string().contains("expected line"));
        assert!(err.to_string().contains("actual line"));
    }

    #[test]
    fn test_is_recoverable() {
        let recoverable = PatchError::context_mismatch(1, "a", "b");
        assert!(recoverable.is_recoverable());

        let not_recoverable = PatchError::file_not_found("/path");
        assert!(!not_recoverable.is_recoverable());
    }
}
