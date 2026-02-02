//! File safety utilities for Cortex.
//!
//! This module provides file validation functionality to prevent reading
//! potentially dangerous files like:
//! - Block devices (e.g., /dev/sda)
//! - Character devices (e.g., /dev/null, /dev/random)
//! - FIFOs (named pipes)
//! - Sockets
//! - Special filesystem paths (/dev/, /proc/, /sys/)
//! - Extremely large files

use std::path::{Path, PathBuf};

/// Maximum file size allowed for reading (100MB by default).
pub const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Errors that can occur during file validation.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// Attempted to read a block device.
    #[error("Cannot read block device: {0}")]
    BlockDevice(PathBuf),

    /// Attempted to read a character device.
    #[error("Cannot read character device: {0}")]
    CharDevice(PathBuf),

    /// Attempted to read a FIFO (named pipe).
    #[error("Cannot read FIFO/named pipe: {0}")]
    Fifo(PathBuf),

    /// Attempted to read a socket.
    #[error("Cannot read socket: {0}")]
    Socket(PathBuf),

    /// Attempted to read from a special system path.
    #[error("Cannot read special system path: {0}")]
    SpecialPath(PathBuf),

    /// File exceeds maximum allowed size.
    #[error("File too large: {path} ({size} bytes, max {max} bytes)")]
    TooLarge { path: PathBuf, size: u64, max: u64 },

    /// IO error during validation.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Validate that a file path is safe to read.
///
/// This function checks:
/// 1. The file type (rejects devices, FIFOs, sockets)
/// 2. The file path (rejects /dev/, /proc/, /sys/)
/// 3. The file size (rejects files larger than `MAX_FILE_SIZE`)
///
/// # Arguments
///
/// * `path` - The path to validate
///
/// # Returns
///
/// * `Ok(())` if the file is safe to read
/// * `Err(FileError)` if the file should not be read
///
/// # Examples
///
/// ```no_run
/// use cortex_utils_file_safety::validate_file_for_read;
/// use std::path::Path;
///
/// // Regular file should be OK
/// let result = validate_file_for_read(Path::new("Cargo.toml"));
/// assert!(result.is_ok());
///
/// // Device files should fail (on Unix)
/// #[cfg(unix)]
/// {
///     let result = validate_file_for_read(Path::new("/dev/null"));
///     assert!(result.is_err());
/// }
/// ```
pub fn validate_file_for_read(path: &Path) -> Result<(), FileError> {
    validate_file_for_read_with_limit(path, MAX_FILE_SIZE)
}

/// Validate that a file path is safe to read with a custom size limit.
///
/// # Arguments
///
/// * `path` - The path to validate
/// * `max_size` - Maximum file size in bytes (0 for no limit)
///
/// # Returns
///
/// * `Ok(())` if the file is safe to read
/// * `Err(FileError)` if the file should not be read
pub fn validate_file_for_read_with_limit(path: &Path, max_size: u64) -> Result<(), FileError> {
    let metadata = std::fs::metadata(path)?;

    // Check file type on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        let file_type = metadata.file_type();

        if file_type.is_block_device() {
            return Err(FileError::BlockDevice(path.to_path_buf()));
        }
        if file_type.is_char_device() {
            return Err(FileError::CharDevice(path.to_path_buf()));
        }
        if file_type.is_fifo() {
            return Err(FileError::Fifo(path.to_path_buf()));
        }
        if file_type.is_socket() {
            return Err(FileError::Socket(path.to_path_buf()));
        }
    }

    // Check for special paths (cross-platform)
    let path_str = path.to_string_lossy();

    // Unix-specific special paths
    #[cfg(unix)]
    {
        if path_str.starts_with("/dev/")
            || path_str.starts_with("/proc/")
            || path_str.starts_with("/sys/")
        {
            return Err(FileError::SpecialPath(path.to_path_buf()));
        }
    }

    // Windows-specific special paths
    #[cfg(windows)]
    {
        // Windows device paths like \\.\PhysicalDrive0, CON, NUL, etc.
        let path_upper = path_str.to_uppercase();
        if path_upper.starts_with("\\\\.\\")
            || path_upper.starts_with("\\\\?\\")
            || matches!(
                path_upper.trim_end_matches([':', '\\', '/']),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            )
        {
            return Err(FileError::SpecialPath(path.to_path_buf()));
        }
    }

    // Check file size (0 means no limit)
    if max_size > 0 && metadata.len() > max_size {
        return Err(FileError::TooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            max: max_size,
        });
    }

    Ok(())
}

/// Check if a path appears to be a special system path.
///
/// This is a quick check that doesn't require filesystem access.
/// It's useful for pre-filtering paths before attempting validation.
///
/// # Arguments
///
/// * `path` - The path to check
///
/// # Returns
///
/// * `true` if the path appears to be a special system path
/// * `false` otherwise
pub fn is_special_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    #[cfg(unix)]
    {
        if path_str.starts_with("/dev/")
            || path_str.starts_with("/proc/")
            || path_str.starts_with("/sys/")
        {
            return true;
        }
    }

    #[cfg(windows)]
    {
        let path_upper = path_str.to_uppercase();
        if path_upper.starts_with("\\\\.\\") || path_upper.starts_with("\\\\?\\") {
            return true;
        }

        // Check for DOS device names
        let filename = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        if matches!(
            filename.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            return true;
        }
    }

    false
}

/// Atomically write content to a file.
///
/// This function prevents file corruption if the process is killed during write.
/// It works by:
/// 1. Writing to a temporary file in the same directory
/// 2. Syncing the temporary file to disk (fsync)
/// 3. Atomically renaming the temp file to the target path
///
/// This ensures that the target file either has the old content or the new content,
/// never a partial/corrupt state.
///
/// # Arguments
///
/// * `path` - The target file path
/// * `content` - The content to write
///
/// # Returns
///
/// * `Ok(())` on success
/// * `Err(std::io::Error)` on failure
///
/// # Examples
///
/// ```no_run
/// use cortex_utils_file_safety::atomic_write;
/// use std::path::Path;
///
/// atomic_write(Path::new("config.toml"), b"key = \"value\"\n").expect("Failed to write");
/// ```
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<(), std::io::Error> {
    use std::fs::{File, OpenOptions};
    use std::io::Write;

    // Get parent directory for temp file
    let parent = path.parent().unwrap_or(Path::new("."));

    // Create parent directory if it doesn't exist
    std::fs::create_dir_all(parent)?;

    // Generate temp file name in the same directory
    let temp_name = format!(
        ".{}.tmp.{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config"),
        std::process::id()
    );
    let temp_path = parent.join(&temp_name);

    // Write to temp file
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)?;

        file.write_all(content)?;

        // Sync to disk to ensure data is written before rename
        file.sync_all()?;
    }

    // Atomic rename (on POSIX systems, rename is atomic if src and dst are on same filesystem)
    std::fs::rename(&temp_path, path)?;

    // On some systems, we may want to sync the directory too
    #[cfg(unix)]
    {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

/// Atomically write a string to a file.
///
/// Convenience wrapper around [`atomic_write`] for string content.
pub fn atomic_write_string(path: &Path, content: &str) -> Result<(), std::io::Error> {
    atomic_write(path, content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_regular_file_is_safe() {
        let temp = tempfile::NamedTempFile::new().expect("Failed to create temp file");
        assert!(validate_file_for_read(temp.path()).is_ok());
    }

    #[test]
    fn test_nonexistent_file_returns_error() {
        let result = validate_file_for_read(Path::new("/nonexistent/path/to/file.txt"));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FileError::Io(_)));
    }

    #[cfg(unix)]
    #[test]
    fn test_dev_null_is_rejected() {
        // /dev/null is a character device
        let result = validate_file_for_read(Path::new("/dev/null"));
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_proc_path_is_rejected() {
        // /proc/self/status exists on Linux
        if Path::new("/proc/self/status").exists() {
            let result = validate_file_for_read(Path::new("/proc/self/status"));
            assert!(result.is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_sys_path_is_rejected() {
        // A common sysfs path
        if Path::new("/sys/kernel/hostname").exists() {
            let result = validate_file_for_read(Path::new("/sys/kernel/hostname"));
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_file_too_large() {
        // Create a temp file and test with a very small limit
        let mut temp = tempfile::NamedTempFile::new().expect("Failed to create temp file");
        write!(temp, "Some content here").expect("Failed to write");
        temp.flush().expect("Failed to flush");

        let result = validate_file_for_read_with_limit(temp.path(), 5); // 5 byte limit
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FileError::TooLarge { .. }));
    }

    #[test]
    fn test_file_within_size_limit() {
        let mut temp = tempfile::NamedTempFile::new().expect("Failed to create temp file");
        write!(temp, "Small").expect("Failed to write");
        temp.flush().expect("Failed to flush");

        let result = validate_file_for_read_with_limit(temp.path(), 100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_zero_size_limit_allows_any_size() {
        let mut temp = tempfile::NamedTempFile::new().expect("Failed to create temp file");
        write!(
            temp,
            "Some content that would normally exceed a small limit"
        )
        .expect("Failed to write");
        temp.flush().expect("Failed to flush");

        let result = validate_file_for_read_with_limit(temp.path(), 0);
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_is_special_path_unix() {
        assert!(is_special_path(Path::new("/dev/null")));
        assert!(is_special_path(Path::new("/proc/self/status")));
        assert!(is_special_path(Path::new("/sys/kernel/hostname")));
        assert!(!is_special_path(Path::new("/home/user/file.txt")));
        assert!(!is_special_path(Path::new("/tmp/test.txt")));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_special_path_windows() {
        assert!(is_special_path(Path::new("\\\\.\\PhysicalDrive0")));
        assert!(is_special_path(Path::new("\\\\?\\C:\\test")));
        assert!(is_special_path(Path::new("CON")));
        assert!(is_special_path(Path::new("NUL")));
        assert!(is_special_path(Path::new("COM1")));
        assert!(!is_special_path(Path::new("C:\\Users\\test\\file.txt")));
    }

    #[test]
    fn test_atomic_write() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = temp_dir.path().join("test_config.toml");

        let content = "key = \"value\"\nother = 42";
        atomic_write_string(&file_path, content).expect("Failed to atomic write");

        // Verify content was written correctly
        let read_content = std::fs::read_to_string(&file_path).expect("Failed to read");
        assert_eq!(read_content, content);

        // Verify no temp file remains
        let temp_pattern = ".test_config.toml.tmp.".to_string();
        let dir_contents: Vec<_> = std::fs::read_dir(temp_dir.path())
            .expect("Failed to read dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(&temp_pattern))
            .collect();
        assert!(dir_contents.is_empty(), "Temp file should be removed");
    }

    #[test]
    fn test_atomic_write_creates_parent_dirs() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = temp_dir
            .path()
            .join("nested")
            .join("deep")
            .join("config.toml");

        let content = "test = true";
        atomic_write_string(&file_path, content).expect("Failed to atomic write");

        // Verify content was written correctly
        let read_content = std::fs::read_to_string(&file_path).expect("Failed to read");
        assert_eq!(read_content, content);
    }

    #[test]
    fn test_atomic_write_overwrites_existing() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = temp_dir.path().join("existing.toml");

        // Write initial content
        std::fs::write(&file_path, "old content").expect("Failed to write");

        // Overwrite with atomic write
        let new_content = "new content";
        atomic_write_string(&file_path, new_content).expect("Failed to atomic write");

        // Verify new content
        let read_content = std::fs::read_to_string(&file_path).expect("Failed to read");
        assert_eq!(read_content, new_content);
    }
}
