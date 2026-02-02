//! File permission utilities with umask support.
//!
//! Provides functions for creating files that respect the user's umask setting.
//!
//! # Issue Addressed
//! - #2801: File creation ignores user umask setting

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// Create a file respecting the user's umask setting.
///
/// On Unix systems, this creates a file with mode 0666 which will then be
/// modified by the process umask. On Windows, this creates a normal file.
///
/// # Arguments
/// * `path` - Path to the file to create
///
/// # Returns
/// A `Result` containing the created file handle.
///
/// # Examples
/// ```ignore
/// use cortex_common::file_permissions::create_file_with_umask;
///
/// let file = create_file_with_umask("output.txt")?;
/// // File is created with permissions respecting user's umask
/// ```
pub fn create_file_with_umask(path: impl AsRef<Path>) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        // Set mode to 0666 - the umask will automatically mask off bits
        // For example, with umask 0077, the resulting mode will be 0600
        options.mode(0o666);
    }

    options.open(path)
}

/// Create a file with specific permissions (ignoring umask).
///
/// On Unix systems, this creates a file with exactly the specified mode,
/// bypassing the umask. Use with caution - prefer `create_file_with_umask`
/// for most cases.
///
/// # Arguments
/// * `path` - Path to the file to create
/// * `mode` - Unix file mode (e.g., 0o600 for owner-only read/write)
///
/// # Returns
/// A `Result` containing the created file handle.
#[cfg(unix)]
pub fn create_file_with_mode(path: impl AsRef<Path>, mode: u32) -> io::Result<File> {
    use std::os::unix::fs::PermissionsExt;

    let file = create_file_with_umask(&path)?;

    // Explicitly set the permissions after creation
    let permissions = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(path, permissions)?;

    Ok(file)
}

#[cfg(not(unix))]
pub fn create_file_with_mode(path: impl AsRef<Path>, _mode: u32) -> io::Result<File> {
    // On non-Unix systems, just create a normal file
    create_file_with_umask(path)
}

/// Create a directory respecting the user's umask setting.
///
/// On Unix systems, this creates a directory with mode 0777 which will then be
/// modified by the process umask.
///
/// # Arguments
/// * `path` - Path to the directory to create
///
/// # Returns
/// A `Result` indicating success or failure.
pub fn create_dir_with_umask(path: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        std::fs::DirBuilder::new()
            .mode(0o777) // umask will be applied
            .create(path)
    }

    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

/// Create directories recursively respecting the user's umask setting.
///
/// # Arguments
/// * `path` - Path to the directory to create (including parents)
///
/// # Returns
/// A `Result` indicating success or failure.
pub fn create_dir_all_with_umask(path: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o777) // umask will be applied to each directory
            .create(path)
    }

    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

/// Open or create a file for appending, respecting umask.
///
/// # Arguments
/// * `path` - Path to the file
///
/// # Returns
/// A `Result` containing the file handle opened for appending.
pub fn open_for_append_with_umask(path: impl AsRef<Path>) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).append(true);

    #[cfg(unix)]
    {
        options.mode(0o666);
    }

    options.open(path)
}

/// Get the current umask value.
///
/// Note: On Unix, this temporarily sets the umask to get the current value,
/// then restores it. This is thread-safe only if no other threads are
/// modifying the umask simultaneously.
///
/// # Returns
/// The current umask value, or 0 on non-Unix systems.
#[cfg(unix)]
pub fn get_umask() -> u32 {
    // Unfortunately, there's no way to read umask without setting it
    // We set it to 0, get the old value, then restore it
    unsafe {
        let old_umask = libc::umask(0);
        libc::umask(old_umask);
        old_umask as u32
    }
}

#[cfg(not(unix))]
pub fn get_umask() -> u32 {
    0
}

/// Calculate the effective mode after applying umask.
///
/// # Arguments
/// * `base_mode` - The base mode (e.g., 0o666)
/// * `umask` - The umask value
///
/// # Returns
/// The effective mode after applying the umask.
pub fn apply_umask(base_mode: u32, umask: u32) -> u32 {
    base_mode & !umask
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_create_file_with_umask() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        let mut file = create_file_with_umask(&file_path).unwrap();
        file.write_all(b"test").unwrap();
        drop(file);

        assert!(file_path.exists());
    }

    #[test]
    fn test_create_dir_with_umask() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path().join("subdir");

        create_dir_with_umask(&dir_path).unwrap();
        assert!(dir_path.is_dir());
    }

    #[test]
    fn test_create_dir_all_with_umask() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path().join("a").join("b").join("c");

        create_dir_all_with_umask(&dir_path).unwrap();
        assert!(dir_path.is_dir());
    }

    #[test]
    fn test_open_for_append_with_umask() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("append.txt");

        {
            let mut file = open_for_append_with_umask(&file_path).unwrap();
            file.write_all(b"first").unwrap();
        }

        {
            let mut file = open_for_append_with_umask(&file_path).unwrap();
            file.write_all(b"second").unwrap();
        }

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "firstsecond");
    }

    #[test]
    fn test_apply_umask() {
        // With umask 0022, 0666 becomes 0644
        assert_eq!(apply_umask(0o666, 0o022), 0o644);

        // With umask 0077, 0666 becomes 0600
        assert_eq!(apply_umask(0o666, 0o077), 0o600);

        // With umask 0, mode is unchanged
        assert_eq!(apply_umask(0o666, 0o000), 0o666);
    }

    #[cfg(unix)]
    #[test]
    fn test_create_file_with_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("private.txt");

        let file = create_file_with_mode(&file_path, 0o600).unwrap();
        drop(file);

        let metadata = std::fs::metadata(&file_path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
