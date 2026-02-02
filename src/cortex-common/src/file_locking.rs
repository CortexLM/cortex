//! Cross-platform file locking utilities for preventing race conditions.
//!
//! This module provides safe file operations with advisory locking support
//! and atomic write capabilities to prevent TOCTOU race conditions.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// Error type for file locking operations.
#[derive(Debug)]
pub enum FileLockError {
    /// File not found.
    NotFound(PathBuf),
    /// Lock acquisition failed.
    LockFailed(String),
    /// Lock timeout exceeded.
    LockTimeout(PathBuf),
    /// I/O error during operation.
    Io(io::Error),
    /// Atomic write failed.
    AtomicWriteFailed(String),
}

impl std::fmt::Display for FileLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(path) => write!(f, "File not found: {}", path.display()),
            Self::LockFailed(msg) => write!(f, "Failed to acquire lock: {}", msg),
            Self::LockTimeout(path) => {
                write!(f, "Lock timeout exceeded for: {}", path.display())
            }
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::AtomicWriteFailed(msg) => write!(f, "Atomic write failed: {}", msg),
        }
    }
}

impl std::error::Error for FileLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for FileLockError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Result type for file locking operations.
pub type FileLockResult<T> = Result<T, FileLockError>;

/// Lock mode for file operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Shared (read) lock - multiple readers allowed.
    Shared,
    /// Exclusive (write) lock - single writer only.
    Exclusive,
}

/// Configuration for lock acquisition.
#[derive(Debug, Clone)]
pub struct LockConfig {
    /// Maximum time to wait for lock acquisition.
    pub timeout: Duration,
    /// Retry interval when lock is not immediately available.
    pub retry_interval: Duration,
    /// Whether to use blocking or non-blocking lock attempts.
    pub blocking: bool,
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            retry_interval: Duration::from_millis(50),
            blocking: false,
        }
    }
}

impl LockConfig {
    /// Create a new lock configuration with specified timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Default::default()
        }
    }

    /// Create a blocking lock configuration.
    pub fn blocking() -> Self {
        Self {
            blocking: true,
            ..Default::default()
        }
    }
}

/// A guard that releases the file lock when dropped.
pub struct FileLockGuard {
    file: File,
    path: PathBuf,
    _mode: LockMode,
}

impl FileLockGuard {
    /// Get a reference to the underlying file.
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Get a mutable reference to the underlying file.
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// Get the path of the locked file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the entire file contents.
    pub fn read_to_string(&mut self) -> io::Result<String> {
        use std::io::Seek;
        self.file.seek(io::SeekFrom::Start(0))?;
        let mut content = String::new();
        self.file.read_to_string(&mut content)?;
        Ok(content)
    }

    /// Write content to the file (truncates existing content).
    pub fn write_all(&mut self, content: &[u8]) -> io::Result<()> {
        use std::io::Seek;
        self.file.seek(io::SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        self.file.write_all(content)?;
        self.file.sync_all()?;
        Ok(())
    }
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        // Advisory locks are automatically released when the file is closed
        // but we explicitly unlock for clarity and cross-platform consistency
        let _ = unlock_file(&self.file);
    }
}

/// Acquire an advisory lock on a file.
///
/// # Arguments
/// * `path` - Path to the file to lock
/// * `mode` - Lock mode (shared or exclusive)
/// * `config` - Lock configuration
///
/// # Returns
/// A `FileLockGuard` that releases the lock when dropped.
pub fn acquire_lock(
    path: impl AsRef<Path>,
    mode: LockMode,
    config: &LockConfig,
) -> FileLockResult<FileLockGuard> {
    let path = path.as_ref().to_path_buf();

    let file = OpenOptions::new()
        .read(true)
        .write(matches!(mode, LockMode::Exclusive))
        .create(matches!(mode, LockMode::Exclusive))
        .open(&path)
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                FileLockError::NotFound(path.clone())
            } else {
                FileLockError::Io(e)
            }
        })?;

    if config.blocking {
        lock_file_blocking(&file, mode)?;
    } else {
        lock_file_with_timeout(&file, mode, config)?;
    }

    Ok(FileLockGuard {
        file,
        path,
        _mode: mode,
    })
}

/// Try to acquire a lock without waiting.
pub fn try_acquire_lock(
    path: impl AsRef<Path>,
    mode: LockMode,
) -> FileLockResult<Option<FileLockGuard>> {
    let path = path.as_ref().to_path_buf();

    let file = match OpenOptions::new()
        .read(true)
        .write(matches!(mode, LockMode::Exclusive))
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(FileLockError::NotFound(path));
        }
        Err(e) => return Err(FileLockError::Io(e)),
    };

    match try_lock_file(&file, mode) {
        Ok(true) => Ok(Some(FileLockGuard {
            file,
            path,
            _mode: mode,
        })),
        Ok(false) => Ok(None),
        Err(e) => Err(e),
    }
}

// Platform-specific locking implementations

#[cfg(unix)]
fn lock_file_blocking(file: &File, mode: LockMode) -> FileLockResult<()> {
    use std::os::unix::io::AsRawFd;

    let fd = file.as_raw_fd();
    let operation = match mode {
        LockMode::Shared => libc::LOCK_SH,
        LockMode::Exclusive => libc::LOCK_EX,
    };

    let result = unsafe { libc::flock(fd, operation) };
    if result == 0 {
        Ok(())
    } else {
        Err(FileLockError::LockFailed(format!(
            "flock failed: {}",
            io::Error::last_os_error()
        )))
    }
}

#[cfg(unix)]
fn try_lock_file(file: &File, mode: LockMode) -> FileLockResult<bool> {
    use std::os::unix::io::AsRawFd;

    let fd = file.as_raw_fd();
    let operation = match mode {
        LockMode::Shared => libc::LOCK_SH | libc::LOCK_NB,
        LockMode::Exclusive => libc::LOCK_EX | libc::LOCK_NB,
    };

    let result = unsafe { libc::flock(fd, operation) };
    if result == 0 {
        Ok(true)
    } else {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(false)
        } else {
            Err(FileLockError::LockFailed(format!("flock failed: {}", err)))
        }
    }
}

#[cfg(unix)]
fn unlock_file(file: &File) -> FileLockResult<()> {
    use std::os::unix::io::AsRawFd;

    let fd = file.as_raw_fd();
    let result = unsafe { libc::flock(fd, libc::LOCK_UN) };
    if result == 0 {
        Ok(())
    } else {
        Err(FileLockError::LockFailed(format!(
            "unlock failed: {}",
            io::Error::last_os_error()
        )))
    }
}

#[cfg(windows)]
fn lock_file_blocking(file: &File, mode: LockMode) -> FileLockResult<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx};

    let handle = file.as_raw_handle() as HANDLE;
    let flags = match mode {
        LockMode::Shared => 0,
        LockMode::Exclusive => LOCKFILE_EXCLUSIVE_LOCK,
    };

    // Lock entire file
    let mut overlapped =
        unsafe { std::mem::zeroed::<windows_sys::Win32::System::IO::OVERLAPPED>() };

    let result = unsafe { LockFileEx(handle, flags, 0, u32::MAX, u32::MAX, &mut overlapped) };

    if result != 0 {
        Ok(())
    } else {
        Err(FileLockError::LockFailed(format!(
            "LockFileEx failed: {}",
            io::Error::last_os_error()
        )))
    }
}

#[cfg(windows)]
fn try_lock_file(file: &File, mode: LockMode) -> FileLockResult<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };

    let handle = file.as_raw_handle() as HANDLE;
    let flags = match mode {
        LockMode::Shared => LOCKFILE_FAIL_IMMEDIATELY,
        LockMode::Exclusive => LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
    };

    let mut overlapped =
        unsafe { std::mem::zeroed::<windows_sys::Win32::System::IO::OVERLAPPED>() };

    let result = unsafe { LockFileEx(handle, flags, 0, u32::MAX, u32::MAX, &mut overlapped) };

    if result != 0 {
        Ok(true)
    } else {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
            Ok(false)
        } else {
            Err(FileLockError::LockFailed(format!(
                "LockFileEx failed: {}",
                err
            )))
        }
    }
}

#[cfg(windows)]
fn unlock_file(file: &File) -> FileLockResult<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;

    let handle = file.as_raw_handle() as HANDLE;
    let mut overlapped =
        unsafe { std::mem::zeroed::<windows_sys::Win32::System::IO::OVERLAPPED>() };

    let result = unsafe { UnlockFileEx(handle, 0, u32::MAX, u32::MAX, &mut overlapped) };

    if result != 0 {
        Ok(())
    } else {
        Err(FileLockError::LockFailed(format!(
            "UnlockFileEx failed: {}",
            io::Error::last_os_error()
        )))
    }
}

fn lock_file_with_timeout(file: &File, mode: LockMode, config: &LockConfig) -> FileLockResult<()> {
    let start = std::time::Instant::now();

    loop {
        match try_lock_file(file, mode)? {
            true => return Ok(()),
            false => {
                if start.elapsed() >= config.timeout {
                    return Err(FileLockError::LockTimeout(PathBuf::new()));
                }
                std::thread::sleep(config.retry_interval);
            }
        }
    }
}

/// Perform an atomic file write using write-to-temp-then-rename pattern.
///
/// This is the safest way to update a file as it ensures:
/// 1. The file is never partially written
/// 2. Readers always see complete content
/// 3. System crashes don't corrupt the file
///
/// # Arguments
/// * `path` - Target file path
/// * `content` - Content to write
///
/// # Returns
/// Result indicating success or failure.
pub fn atomic_write(path: impl AsRef<Path>, content: &[u8]) -> FileLockResult<()> {
    let path = path.as_ref();
    let parent = path.parent().ok_or_else(|| {
        FileLockError::AtomicWriteFailed("Cannot determine parent directory".to_string())
    })?;

    // Ensure parent directory exists
    if !parent.exists() {
        fs::create_dir_all(parent)?;
    }

    // Create temp file in same directory (for same-filesystem rename)
    let temp_path = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));

    // Write to temp file
    {
        let mut temp_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)?;

        temp_file.write_all(content)?;
        temp_file.sync_all()?;
    }

    // Atomic rename
    #[cfg(unix)]
    {
        fs::rename(&temp_path, path).map_err(|e| {
            // Clean up temp file on failure
            let _ = fs::remove_file(&temp_path);
            FileLockError::AtomicWriteFailed(format!("rename failed: {}", e))
        })?;
    }

    #[cfg(windows)]
    {
        // On Windows, rename may fail if target exists
        // Use replace_file for atomic replacement
        if path.exists() {
            // Try to use ReplaceFile for true atomic replacement
            match replace_file_windows(&temp_path, path) {
                Ok(()) => {}
                Err(_) => {
                    // Fallback: remove then rename (small race window)
                    let _ = fs::remove_file(path);
                    fs::rename(&temp_path, path).map_err(|e| {
                        let _ = fs::remove_file(&temp_path);
                        FileLockError::AtomicWriteFailed(format!("rename failed: {}", e))
                    })?;
                }
            }
        } else {
            fs::rename(&temp_path, path).map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                FileLockError::AtomicWriteFailed(format!("rename failed: {}", e))
            })?;
        }
    }

    Ok(())
}

#[cfg(windows)]
fn replace_file_windows(source: &Path, dest: &Path) -> FileLockResult<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    fn to_wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    let source_wide = to_wide(source.as_os_str());
    let dest_wide = to_wide(dest.as_os_str());

    let result = unsafe {
        ReplaceFileW(
            dest_wide.as_ptr(),
            source_wide.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };

    if result != 0 {
        Ok(())
    } else {
        Err(FileLockError::AtomicWriteFailed(format!(
            "ReplaceFileW failed: {}",
            io::Error::last_os_error()
        )))
    }
}

/// Perform a locked read-modify-write operation atomically.
///
/// This function:
/// 1. Acquires an exclusive lock on the file
/// 2. Reads the current content
/// 3. Applies the modification function
/// 4. Writes the result atomically
///
/// # Arguments
/// * `path` - Path to the file
/// * `modify` - Function that transforms the content
///
/// # Returns
/// The result of the modification function.
pub fn locked_read_modify_write<T, F>(path: impl AsRef<Path>, modify: F) -> FileLockResult<T>
where
    F: FnOnce(&str) -> (String, T),
{
    let path = path.as_ref();
    let config = LockConfig::default();

    // Acquire exclusive lock
    let mut guard = acquire_lock(path, LockMode::Exclusive, &config)?;

    // Read current content
    let content = guard.read_to_string()?;

    // Apply modification
    let (new_content, result) = modify(&content);

    // Perform atomic write
    atomic_write(path, new_content.as_bytes())?;

    Ok(result)
}

/// Async version of atomic write for use with tokio.
#[cfg(feature = "async")]
pub async fn atomic_write_async(
    path: impl AsRef<Path>,
    content: impl AsRef<[u8]>,
) -> FileLockResult<()> {
    let path = path.as_ref().to_path_buf();
    let content = content.as_ref().to_vec();

    tokio::task::spawn_blocking(move || atomic_write(&path, &content))
        .await
        .map_err(|e| FileLockError::AtomicWriteFailed(format!("spawn_blocking failed: {}", e)))?
}

/// A file lock manager for coordinating access across multiple operations.
///
/// This is useful when you need to perform multiple operations on a file
/// while holding the lock.
pub struct FileLockManager {
    locks: std::sync::Mutex<std::collections::HashMap<PathBuf, Arc<std::sync::Mutex<()>>>>,
}

impl FileLockManager {
    /// Create a new lock manager.
    pub fn new() -> Self {
        Self {
            locks: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Get a process-local mutex for coordinating access to a file.
    ///
    /// This is in addition to the filesystem-level advisory lock and helps
    /// coordinate access within the same process.
    pub fn get_lock(&self, path: impl AsRef<Path>) -> Arc<std::sync::Mutex<()>> {
        let path = path.as_ref().to_path_buf();
        let mut locks = self.locks.lock().unwrap();
        locks
            .entry(path)
            .or_insert_with(|| Arc::new(std::sync::Mutex::new(())))
            .clone()
    }

    /// Execute an operation with both process-local and file-system locks.
    pub fn with_lock<T, F>(&self, path: impl AsRef<Path>, mode: LockMode, f: F) -> FileLockResult<T>
    where
        F: FnOnce(&mut FileLockGuard) -> FileLockResult<T>,
    {
        let path = path.as_ref();
        let process_lock = self.get_lock(path);

        // Acquire process-local lock first
        let _guard = process_lock.lock().unwrap();

        // Then acquire file-system lock
        let mut file_guard = acquire_lock(path, mode, &LockConfig::default())?;

        f(&mut file_guard)
    }
}

impl Default for FileLockManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Global file lock manager instance.
static GLOBAL_LOCK_MANAGER: once_cell::sync::Lazy<FileLockManager> =
    once_cell::sync::Lazy::new(FileLockManager::new);

/// Get the global file lock manager.
pub fn global_lock_manager() -> &'static FileLockManager {
    &GLOBAL_LOCK_MANAGER
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use tempfile::tempdir;

    #[test]
    fn test_atomic_write() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");

        atomic_write(&path, b"Hello, World!").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "Hello, World!");
    }

    #[test]
    fn test_atomic_write_overwrite() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");

        fs::write(&path, "Original").unwrap();
        atomic_write(&path, b"Updated").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "Updated");
    }

    #[test]
    fn test_file_lock_exclusive() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "test").unwrap();

        let config = LockConfig::default();
        let guard = acquire_lock(&path, LockMode::Exclusive, &config).unwrap();
        assert!(guard.path().exists());
    }

    #[test]
    fn test_file_lock_shared() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "test").unwrap();

        let config = LockConfig::default();

        // Multiple shared locks should be allowed
        let guard1 = acquire_lock(&path, LockMode::Shared, &config).unwrap();
        let guard2 = acquire_lock(&path, LockMode::Shared, &config).unwrap();

        assert!(guard1.path().exists());
        assert!(guard2.path().exists());
    }

    #[test]
    fn test_try_acquire_lock_exclusive() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "test").unwrap();

        let _guard = try_acquire_lock(&path, LockMode::Exclusive).unwrap();
        assert!(_guard.is_some());
    }

    #[test]
    fn test_lock_manager() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "0").unwrap();

        let manager = FileLockManager::new();
        let counter = AtomicUsize::new(0);

        // Simulate concurrent access
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                let manager_lock = manager.get_lock(&path);
                let _counter_ref = &counter;

                thread::spawn(move || {
                    let _guard = manager_lock.lock().unwrap();
                    // Simulate some work
                    thread::sleep(Duration::from_millis(10));
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_locked_read_modify_write() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "Hello").unwrap();

        let result = locked_read_modify_write(&path, |content| {
            let new_content = format!("{}, World!", content);
            (new_content, content.len())
        })
        .unwrap();

        assert_eq!(result, 5);
        assert_eq!(fs::read_to_string(&path).unwrap(), "Hello, World!");
    }
}
