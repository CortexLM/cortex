//! Working directory change guard.
//!
//! Provides RAII-style guard for changing working directory that ensures
//! the original directory is restored on exit or error.
//!
//! # Issue Addressed
//! - #2796: Working directory not restored after error when using --cwd option

use std::env;
use std::io;
use std::path::{Path, PathBuf};

/// A guard that restores the working directory when dropped.
///
/// This ensures that if an operation changes the working directory and then
/// fails, the original directory is restored, preventing user confusion.
///
/// # Examples
/// ```ignore
/// use cortex_common::cwd_guard::CwdGuard;
///
/// fn process_in_directory(dir: &Path) -> Result<(), Error> {
///     let _guard = CwdGuard::new(dir)?;
///     // Working directory is now `dir`
///     
///     do_something_that_might_fail()?;
///     
///     // Guard is dropped here, restoring original directory
///     // even if do_something_that_might_fail() returned an error
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct CwdGuard {
    /// The original working directory to restore
    original: PathBuf,
    /// Whether restoration has been disabled
    disabled: bool,
}

impl CwdGuard {
    /// Create a new guard, changing to the specified directory.
    ///
    /// # Arguments
    /// * `new_dir` - The directory to change to
    ///
    /// # Returns
    /// A `Result` containing the guard, or an IO error if the directory
    /// change failed.
    ///
    /// # Examples
    /// ```ignore
    /// let guard = CwdGuard::new("/tmp/workspace")?;
    /// // Now in /tmp/workspace
    /// // Original directory will be restored when guard is dropped
    /// ```
    pub fn new(new_dir: impl AsRef<Path>) -> io::Result<Self> {
        let original = env::current_dir()?;
        env::set_current_dir(new_dir.as_ref())?;

        Ok(Self {
            original,
            disabled: false,
        })
    }

    /// Create a guard that saves the current directory without changing it.
    ///
    /// This is useful when you might change directories later but want to
    /// ensure restoration regardless.
    ///
    /// # Returns
    /// A `Result` containing the guard.
    pub fn save_current() -> io::Result<Self> {
        let original = env::current_dir()?;

        Ok(Self {
            original,
            disabled: false,
        })
    }

    /// Get the original directory that will be restored.
    pub fn original(&self) -> &Path {
        &self.original
    }

    /// Get the current working directory.
    pub fn current() -> io::Result<PathBuf> {
        env::current_dir()
    }

    /// Disable restoration when the guard is dropped.
    ///
    /// Use this if you want to keep the new working directory after
    /// successful completion.
    pub fn disable_restore(&mut self) {
        self.disabled = true;
    }

    /// Enable restoration (undo `disable_restore`).
    pub fn enable_restore(&mut self) {
        self.disabled = false;
    }

    /// Manually restore the original directory early.
    ///
    /// This is called automatically on drop, but can be called manually
    /// if you need to restore earlier.
    ///
    /// # Returns
    /// An `io::Result` indicating success or failure of the directory change.
    pub fn restore_now(&mut self) -> io::Result<()> {
        if !self.disabled {
            env::set_current_dir(&self.original)?;
            self.disabled = true; // Prevent double-restore on drop
        }
        Ok(())
    }

    /// Change to a new directory while keeping the same restoration point.
    ///
    /// # Arguments
    /// * `new_dir` - The new directory to change to
    ///
    /// # Returns
    /// An `io::Result` indicating success or failure.
    pub fn change_to(&self, new_dir: impl AsRef<Path>) -> io::Result<()> {
        env::set_current_dir(new_dir)
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        if !self.disabled {
            // Ignore errors on drop - we can't do much about them
            // and panicking in drop is bad
            let _ = env::set_current_dir(&self.original);
        }
    }
}

/// Execute a closure in a specific directory, restoring on completion.
///
/// This is a convenience function for simple use cases where you don't
/// need access to the guard.
///
/// # Arguments
/// * `dir` - The directory to execute in
/// * `f` - The closure to execute
///
/// # Returns
/// The result of the closure, or an IO error if directory change failed.
///
/// # Examples
/// ```ignore
/// use cortex_common::cwd_guard::in_directory;
///
/// let result = in_directory("/tmp/workspace", || {
///     // Do work in /tmp/workspace
///     Ok(compute_something())
/// })?;
/// // Back in original directory
/// ```
pub fn in_directory<T, F>(dir: impl AsRef<Path>, f: F) -> io::Result<T>
where
    F: FnOnce() -> T,
{
    let _guard = CwdGuard::new(dir)?;
    Ok(f())
}

/// Execute a fallible closure in a specific directory, restoring on completion.
///
/// # Arguments
/// * `dir` - The directory to execute in
/// * `f` - The closure to execute
///
/// # Returns
/// The result of the closure, or an error.
pub fn in_directory_result<T, E, F>(dir: impl AsRef<Path>, f: F) -> Result<T, E>
where
    E: From<io::Error>,
    F: FnOnce() -> Result<T, E>,
{
    let _guard = CwdGuard::new(dir)?;
    f()
}

/// Check if the current working directory exists and is accessible (#2744).
///
/// This function detects when the CWD has been deleted by another process
/// and returns a clear error instead of letting subsequent operations fail
/// with cryptic errors.
///
/// # Returns
/// - `Ok(PathBuf)` - The valid current working directory
/// - `Err` - A descriptive error if the CWD is invalid
///
/// # Examples
/// ```ignore
/// use cortex_common::cwd_guard::validate_cwd;
///
/// match validate_cwd() {
///     Ok(cwd) => println!("Working in: {}", cwd.display()),
///     Err(e) => eprintln!("CWD error: {}", e),
/// }
/// ```
pub fn validate_cwd() -> io::Result<PathBuf> {
    // Try to get the current directory
    let cwd = match env::current_dir() {
        Ok(path) => path,
        Err(e) => {
            // If we can't get the current directory, it likely doesn't exist
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Current working directory is invalid or has been deleted: {}. \
                     Please change to a valid directory using 'cd' or restart the application.",
                    e
                ),
            ));
        }
    };

    // Verify the directory actually exists and is accessible
    if !cwd.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "Current working directory '{}' no longer exists. \
                 It may have been deleted by another process. \
                 Please change to a valid directory using 'cd' or restart the application.",
                cwd.display()
            ),
        ));
    }

    // Try to read the directory to verify it's accessible
    match std::fs::read_dir(&cwd) {
        Ok(_) => Ok(cwd),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "Cannot access current working directory '{}': permission denied. \
                 Please check directory permissions or change to an accessible directory.",
                cwd.display()
            ),
        )),
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!(
                "Current working directory '{}' is not accessible: {}. \
                 Please change to a valid directory.",
                cwd.display(),
                e
            ),
        )),
    }
}

/// Check if a specific directory exists and is accessible.
///
/// This is useful for validating a target directory before operations.
///
/// # Arguments
/// * `dir` - The directory to validate
///
/// # Returns
/// - `Ok(())` - The directory is valid and accessible
/// - `Err` - A descriptive error if the directory is invalid
pub fn validate_directory(dir: impl AsRef<Path>) -> io::Result<()> {
    let path = dir.as_ref();

    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Directory '{}' does not exist.", path.display()),
        ));
    }

    if !path.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Path '{}' is not a directory.", path.display()),
        ));
    }

    // Try to read the directory to verify it's accessible
    match std::fs::read_dir(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "Cannot access directory '{}': permission denied.",
                path.display()
            ),
        )),
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!("Cannot access directory '{}': {}", path.display(), e),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    /// Helper to get normalized current directory for comparison.
    /// On Windows, uses dunce::canonicalize to handle 8.3 paths and UNC paths.
    fn normalized_current_dir() -> PathBuf {
        let cwd = env::current_dir().unwrap();
        dunce::canonicalize(&cwd).unwrap_or(cwd)
    }

    /// Helper to normalize a path for comparison.
    /// On Windows, uses dunce::canonicalize to handle 8.3 paths and UNC paths.
    fn normalize_path(path: &Path) -> PathBuf {
        dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    #[test]
    #[serial]
    fn test_cwd_guard_restores() {
        let original = normalized_current_dir();
        let temp_dir = TempDir::new().unwrap();
        let expected_temp_path = normalize_path(temp_dir.path());

        {
            let _guard = CwdGuard::new(temp_dir.path()).unwrap();
            assert_eq!(normalized_current_dir(), expected_temp_path);
        }

        // Should be back to original after guard dropped
        assert_eq!(normalized_current_dir(), original);
    }

    #[test]
    #[serial]
    fn test_cwd_guard_restores_on_panic() {
        let original = normalized_current_dir();
        let temp_dir = TempDir::new().unwrap();

        let result = std::panic::catch_unwind(|| {
            let _guard = CwdGuard::new(temp_dir.path()).unwrap();
            panic!("test panic");
        });

        assert!(result.is_err());
        // Should still be back to original
        assert_eq!(normalized_current_dir(), original);
    }

    #[test]
    #[serial]
    fn test_cwd_guard_disabled() {
        let original = normalized_current_dir();
        let temp_dir = TempDir::new().unwrap();
        let temp_path = normalize_path(temp_dir.path());

        {
            let mut guard = CwdGuard::new(temp_dir.path()).unwrap();
            guard.disable_restore();
        }

        // Should NOT be back to original
        assert_eq!(normalized_current_dir(), temp_path);

        // Restore manually
        env::set_current_dir(&original).unwrap();
    }

    #[test]
    #[serial]
    fn test_cwd_guard_restore_now() {
        let original = normalized_current_dir();
        let temp_dir = TempDir::new().unwrap();

        let mut guard = CwdGuard::new(temp_dir.path()).unwrap();

        // Manually restore
        guard.restore_now().unwrap();
        assert_eq!(normalized_current_dir(), original);

        // Guard drop should not panic (it's disabled after restore_now)
    }

    #[test]
    #[serial]
    fn test_in_directory() {
        let original = normalized_current_dir();
        let temp_dir = TempDir::new().unwrap();
        let temp_path = normalize_path(temp_dir.path());

        let result = in_directory(temp_dir.path(), normalized_current_dir).unwrap();

        assert_eq!(result, temp_path);
        assert_eq!(normalized_current_dir(), original);
    }

    #[test]
    #[serial]
    fn test_in_directory_result() {
        let original = normalized_current_dir();
        let temp_dir = TempDir::new().unwrap();

        let result: Result<(), io::Error> =
            in_directory_result(temp_dir.path(), || Err(io::Error::other("test error")));

        assert!(result.is_err());
        // Should still be back to original
        assert_eq!(normalized_current_dir(), original);
    }

    #[test]
    #[serial]
    fn test_save_current() {
        let original = normalized_current_dir();
        let temp_dir = TempDir::new().unwrap();

        {
            let guard = CwdGuard::save_current().unwrap();
            assert_eq!(normalize_path(guard.original()), original);

            // Manually change directory
            env::set_current_dir(temp_dir.path()).unwrap();
        }

        // Should be back to original after guard dropped
        assert_eq!(normalized_current_dir(), original);
    }
}
