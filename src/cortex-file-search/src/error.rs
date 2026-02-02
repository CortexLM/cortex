//! Error types for file search operations.

use std::path::PathBuf;

/// Result type alias for file search operations.
pub type SearchResult<T> = std::result::Result<T, SearchError>;

/// Errors that can occur during file search operations.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// The specified root directory does not exist.
    #[error("Root directory does not exist: {0}")]
    RootNotFound(PathBuf),

    /// The specified path is not a directory.
    #[error("Path is not a directory: {0}")]
    NotADirectory(PathBuf),

    /// Failed to read directory contents.
    #[error("Failed to read directory '{path}': {source}")]
    ReadDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to read file contents.
    #[error("Failed to read file '{path}': {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to parse glob pattern.
    #[error("Invalid glob pattern '{pattern}': {reason}")]
    InvalidGlobPattern { pattern: String, reason: String },

    /// Index has not been built yet.
    #[error("File index has not been built. Call build_index() first.")]
    IndexNotBuilt,

    /// Index is currently being built.
    #[error("File index is currently being built. Please wait.")]
    IndexBuilding,

    /// Failed to acquire lock on internal state.
    #[error("Failed to acquire lock on internal state")]
    LockFailed,

    /// Search query is empty.
    #[error("Search query cannot be empty")]
    EmptyQuery,

    /// I/O error during file operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Generic error wrapper.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl SearchError {
    /// Creates a new `RootNotFound` error.
    pub fn root_not_found(path: impl Into<PathBuf>) -> Self {
        Self::RootNotFound(path.into())
    }

    /// Creates a new `NotADirectory` error.
    pub fn not_a_directory(path: impl Into<PathBuf>) -> Self {
        Self::NotADirectory(path.into())
    }

    /// Creates a new `ReadDirectory` error.
    pub fn read_directory(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::ReadDirectory {
            path: path.into(),
            source,
        }
    }

    /// Creates a new `ReadFile` error.
    pub fn read_file(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::ReadFile {
            path: path.into(),
            source,
        }
    }

    /// Creates a new `InvalidGlobPattern` error.
    pub fn invalid_glob(pattern: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidGlobPattern {
            pattern: pattern.into(),
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = SearchError::root_not_found("/nonexistent");
        assert!(err.to_string().contains("/nonexistent"));

        let err = SearchError::EmptyQuery;
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let search_err: SearchError = io_err.into();
        assert!(matches!(search_err, SearchError::Io(_)));
    }
}
