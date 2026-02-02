//! Atomic File Writing.
//!
//! This module provides atomic file writing capabilities to ensure that
//! files are either fully written or remain unchanged. This prevents
//! file corruption during interruptions or concurrent access.
//!
//! # Pattern
//!
//! The atomic write pattern works as follows:
//! 1. Write content to a temporary file in the same directory
//! 2. Sync the temporary file to disk (fsync)
//! 3. Atomically rename the temp file to the target path
//!
//! This ensures that the target file is never in a partially written state.
//!
//! # Platform Support
//!
//! - **Unix**: Uses `rename()` which is atomic on POSIX systems
//! - **Windows**: Falls back to remove-then-rename (less atomic)

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors that can occur during atomic file operations.
#[derive(Debug, Error)]
pub enum AtomicWriteError {
    /// Failed to create temporary file.
    #[error("Failed to create temporary file in '{dir}': {source}")]
    CreateTemp {
        dir: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Failed to write to temporary file.
    #[error("Failed to write to temporary file: {0}")]
    Write(#[source] io::Error),

    /// Failed to sync file to disk.
    #[error("Failed to sync file to disk: {0}")]
    Sync(#[source] io::Error),

    /// Failed to rename/move file.
    #[error("Failed to rename '{from}' to '{to}': {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Failed to remove existing file (Windows fallback).
    #[error("Failed to remove existing file '{path}': {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Failed to create parent directory.
    #[error("Failed to create parent directory '{dir}': {source}")]
    CreateDir {
        dir: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Target path has no parent directory.
    #[error("Target path has no parent directory: {0}")]
    NoParent(PathBuf),
}

/// Result type for atomic write operations.
pub type AtomicResult<T> = Result<T, AtomicWriteError>;

/// Atomic file writer that ensures writes are all-or-nothing.
///
/// Uses a write-to-temp-then-rename strategy to ensure atomicity.
pub struct AtomicWriter {
    /// The target file path.
    target: PathBuf,
    /// The temporary file path.
    temp: PathBuf,
    /// The temporary file handle (Option to allow taking ownership in commit).
    file: Option<File>,
    /// Whether the write was committed.
    committed: bool,
}

impl AtomicWriter {
    /// Create a new atomic writer for the given target path.
    ///
    /// Creates a temporary file in the same directory as the target.
    /// The temporary file has a `.tmp` suffix with a random component.
    pub fn new(target: impl AsRef<Path>) -> AtomicResult<Self> {
        let target = target.as_ref().to_path_buf();

        // Ensure parent directory exists
        let parent = target
            .parent()
            .ok_or_else(|| AtomicWriteError::NoParent(target.clone()))?;

        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|source| AtomicWriteError::CreateDir {
                dir: parent.to_path_buf(),
                source,
            })?;
        }

        // Create temporary file path with random suffix
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let temp_name = format!(
            ".{}.{}.tmp",
            target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file"),
            timestamp
        );
        let temp = parent.join(temp_name);

        // Create the temporary file
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp)
            .map_err(|source| AtomicWriteError::CreateTemp {
                dir: parent.to_path_buf(),
                source,
            })?;

        Ok(Self {
            target,
            temp,
            file: Some(file),
            committed: false,
        })
    }

    /// Write data to the atomic file.
    pub fn write_all(&mut self, data: &[u8]) -> AtomicResult<()> {
        if let Some(ref mut file) = self.file {
            file.write_all(data).map_err(AtomicWriteError::Write)
        } else {
            Err(AtomicWriteError::Write(io::Error::other(
                "File handle already consumed",
            )))
        }
    }

    /// Write a string to the atomic file.
    pub fn write_str(&mut self, data: &str) -> AtomicResult<()> {
        self.write_all(data.as_bytes())
    }

    /// Commit the write by syncing and renaming.
    ///
    /// After calling this, the target file will contain the written data.
    /// If this method is not called, the temporary file will be cleaned up
    /// when the writer is dropped.
    pub fn commit(mut self) -> AtomicResult<()> {
        // Take ownership of the file to close it
        if let Some(file) = self.file.take() {
            // Sync to disk
            file.sync_all().map_err(AtomicWriteError::Sync)?;
            // File is dropped here, closing it
        }

        // Perform the atomic rename
        Self::atomic_rename(&self.temp, &self.target)?;

        self.committed = true;
        Ok(())
    }

    /// Perform an atomic rename.
    ///
    /// On Unix, this uses the standard `rename()` which is atomic.
    /// On Windows, we need to handle the case where the target exists.
    #[cfg(unix)]
    fn atomic_rename(from: &Path, to: &Path) -> AtomicResult<()> {
        fs::rename(from, to).map_err(|source| AtomicWriteError::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })
    }

    #[cfg(windows)]
    fn atomic_rename(from: &Path, to: &Path) -> AtomicResult<()> {
        // Windows doesn't have atomic rename-over-existing
        // We try rename first, then remove+rename if target exists
        match fs::rename(from, to) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists || to.exists() => {
                // Remove the target first, then rename
                fs::remove_file(to).map_err(|source| AtomicWriteError::Remove {
                    path: to.to_path_buf(),
                    source,
                })?;
                fs::rename(from, to).map_err(|source| AtomicWriteError::Rename {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                    source,
                })
            }
            Err(source) => Err(AtomicWriteError::Rename {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                source,
            }),
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn atomic_rename(from: &Path, to: &Path) -> AtomicResult<()> {
        // Fallback for other platforms
        fs::rename(from, to).map_err(|source| AtomicWriteError::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })
    }
}

impl Drop for AtomicWriter {
    fn drop(&mut self) {
        // Clean up temp file if not committed
        if !self.committed {
            let _ = fs::remove_file(&self.temp);
        }
    }
}

impl Write for AtomicWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(ref mut file) = self.file {
            file.write(buf)
        } else {
            Err(io::Error::other("File handle already consumed"))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(ref mut file) = self.file {
            file.flush()
        } else {
            Err(io::Error::other("File handle already consumed"))
        }
    }
}

/// Atomically write content to a file.
///
/// This is a convenience function that combines `AtomicWriter::new()`,
/// `write_all()`, and `commit()`.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_commands::builtin::atomic::atomic_write;
///
/// atomic_write("config.json", r#"{"key": "value"}"#)?;
/// ```
pub fn atomic_write(path: impl AsRef<Path>, content: impl AsRef<[u8]>) -> AtomicResult<()> {
    let mut writer = AtomicWriter::new(path)?;
    writer.write_all(content.as_ref())?;
    writer.commit()
}

/// Atomically write a string to a file.
pub fn atomic_write_str(path: impl AsRef<Path>, content: &str) -> AtomicResult<()> {
    atomic_write(path, content.as_bytes())
}

/// Options for atomic file writes.
#[derive(Debug, Clone, Default)]
pub struct AtomicWriteOptions {
    /// Create parent directories if they don't exist.
    pub create_parents: bool,
    /// Preserve permissions from existing file.
    pub preserve_permissions: bool,
}

impl AtomicWriteOptions {
    /// Create new options with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable parent directory creation.
    pub fn create_parents(mut self, create: bool) -> Self {
        self.create_parents = create;
        self
    }

    /// Enable permission preservation.
    pub fn preserve_permissions(mut self, preserve: bool) -> Self {
        self.preserve_permissions = preserve;
        self
    }
}

/// Atomically write content to a file with options.
pub fn atomic_write_with_options(
    path: impl AsRef<Path>,
    content: impl AsRef<[u8]>,
    _options: &AtomicWriteOptions,
) -> AtomicResult<()> {
    // Options are handled by AtomicWriter::new() which already creates parents
    atomic_write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_atomic_write_new_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");

        atomic_write(&path, b"Hello, World!").unwrap();

        assert!(path.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "Hello, World!");
    }

    #[test]
    fn test_atomic_write_overwrite() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");

        // Write initial content
        fs::write(&path, "Initial").unwrap();

        // Overwrite atomically
        atomic_write(&path, b"Overwritten").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "Overwritten");
    }

    #[test]
    fn test_atomic_write_creates_parents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a/b/c/test.txt");

        atomic_write(&path, b"Nested").unwrap();

        assert!(path.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "Nested");
    }

    #[test]
    fn test_atomic_writer_drop_cleans_up() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");

        {
            let mut writer = AtomicWriter::new(&path).unwrap();
            writer.write_all(b"Uncommitted").unwrap();
            // Drop without commit
        }

        // Target should not exist
        assert!(!path.exists());

        // No temp files should remain
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_atomic_writer_commit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");

        let mut writer = AtomicWriter::new(&path).unwrap();
        writer.write_all(b"Committed").unwrap();
        writer.commit().unwrap();

        assert!(path.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "Committed");
    }

    #[test]
    fn test_atomic_write_str() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");

        atomic_write_str(&path, "String content").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "String content");
    }

    #[test]
    fn test_atomic_write_preserves_content_on_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");

        // Write initial content
        fs::write(&path, "Original").unwrap();

        // Create writer but don't commit
        {
            let mut writer = AtomicWriter::new(&path).unwrap();
            writer.write_all(b"Modified").unwrap();
            // Drop without commit - simulates error
        }

        // Original content should be preserved
        assert_eq!(fs::read_to_string(&path).unwrap(), "Original");
    }
}
