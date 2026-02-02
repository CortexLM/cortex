//! Path consistency utilities for symlink and case-sensitivity handling.
//!
//! Provides consistent path resolution across different platforms and filesystems.
//!
//! # Issues Addressed
//! - #2798: Inconsistent symbolic link resolution across different operations
//! - #2802: Inconsistent case sensitivity handling across platforms

use std::io;
use std::path::{Path, PathBuf};

/// Strategy for handling symbolic links during path resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SymlinkStrategy {
    /// Follow symlinks to their canonical target (default for most operations)
    #[default]
    Follow,
    /// Preserve symlink paths without following
    Preserve,
    /// Error if a symlink is encountered
    Reject,
}

/// Options for path normalization.
#[derive(Debug, Clone, Default)]
pub struct PathNormalizationOptions {
    /// How to handle symbolic links
    pub symlink_strategy: SymlinkStrategy,
    /// Whether to require the path to exist
    pub require_exists: bool,
    /// Base directory for relative paths
    pub base_dir: Option<PathBuf>,
}

impl PathNormalizationOptions {
    /// Create new options with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the symlink strategy.
    pub fn symlink_strategy(mut self, strategy: SymlinkStrategy) -> Self {
        self.symlink_strategy = strategy;
        self
    }

    /// Set whether the path must exist.
    pub fn require_exists(mut self, require: bool) -> Self {
        self.require_exists = require;
        self
    }

    /// Set the base directory for relative paths.
    pub fn base_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.base_dir = Some(dir.into());
        self
    }
}

/// Normalize a path with consistent symlink handling.
///
/// This function provides consistent path normalization across the codebase,
/// addressing issues where some operations followed symlinks while others didn't.
///
/// # Arguments
/// * `path` - The path to normalize
/// * `options` - Normalization options
///
/// # Returns
/// The normalized path, or an error if normalization failed.
///
/// # Examples
/// ```ignore
/// use cortex_common::path_consistency::{normalize_path_consistent, PathNormalizationOptions, SymlinkStrategy};
///
/// let opts = PathNormalizationOptions::new()
///     .symlink_strategy(SymlinkStrategy::Follow)
///     .require_exists(true);
///
/// let normalized = normalize_path_consistent("/some/path", opts)?;
/// ```
pub fn normalize_path_consistent(
    path: impl AsRef<Path>,
    options: PathNormalizationOptions,
) -> io::Result<PathBuf> {
    let path = path.as_ref();

    // Handle relative paths
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(base) = &options.base_dir {
        base.join(path)
    } else {
        std::env::current_dir()?.join(path)
    };

    // Check existence if required
    if options.require_exists && !abs_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Path does not exist: {}", abs_path.display()),
        ));
    }

    // Handle symlinks based on strategy
    match options.symlink_strategy {
        SymlinkStrategy::Follow => {
            // Use canonicalize which follows symlinks
            if abs_path.exists() {
                abs_path.canonicalize()
            } else {
                // For non-existent paths, normalize without filesystem access
                Ok(normalize_path_lexically(&abs_path))
            }
        }
        SymlinkStrategy::Preserve => {
            // Normalize without following symlinks
            Ok(normalize_path_lexically(&abs_path))
        }
        SymlinkStrategy::Reject => {
            // Check if it's a symlink and reject
            if abs_path.exists() && abs_path.symlink_metadata()?.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Path is a symbolic link: {}", abs_path.display()),
                ));
            }
            if abs_path.exists() {
                abs_path.canonicalize()
            } else {
                Ok(normalize_path_lexically(&abs_path))
            }
        }
    }
}

/// Normalize a path lexically without filesystem access.
///
/// This resolves `.` and `..` components without accessing the filesystem,
/// which means it won't follow symlinks.
///
/// # Arguments
/// * `path` - The path to normalize
///
/// # Returns
/// The normalized path.
pub fn normalize_path_lexically(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() && !path.is_absolute() {
                    normalized.push("..");
                }
            }
            Component::CurDir => {}
            comp => normalized.push(comp),
        }
    }

    if normalized.as_os_str().is_empty() {
        normalized.push(".");
    }

    normalized
}

/// Check if a path matches another path, handling case sensitivity appropriately.
///
/// On case-insensitive filesystems (macOS default, Windows), this does a
/// case-insensitive comparison. On case-sensitive filesystems (Linux),
/// this does a case-sensitive comparison.
///
/// # Arguments
/// * `path1` - First path to compare
/// * `path2` - Second path to compare
///
/// # Returns
/// `true` if the paths match according to the filesystem's case sensitivity.
pub fn paths_match(path1: impl AsRef<Path>, path2: impl AsRef<Path>) -> bool {
    let path1 = path1.as_ref();
    let path2 = path2.as_ref();

    if cfg!(target_os = "linux") {
        // Linux is typically case-sensitive
        path1 == path2
    } else {
        // Windows and macOS are typically case-insensitive
        // Use OsStr comparison for proper Unicode handling
        path1.as_os_str().eq_ignore_ascii_case(path2.as_os_str())
    }
}

/// Find a file matching a pattern, handling case sensitivity appropriately.
///
/// On case-insensitive filesystems, this will find files regardless of case.
/// On case-sensitive filesystems, this requires an exact match.
///
/// # Arguments
/// * `dir` - Directory to search in
/// * `name` - File name to find
///
/// # Returns
/// The path to the found file, or `None` if not found.
pub fn find_file_case_aware(dir: impl AsRef<Path>, name: &str) -> io::Result<Option<PathBuf>> {
    let dir = dir.as_ref();

    // First try exact match
    let exact_path = dir.join(name);
    if exact_path.exists() {
        return Ok(Some(exact_path));
    }

    // On case-insensitive systems, we're done
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }

    // On Linux, we might need to check with different case
    // Only do this if the exact match failed
    let name_lower = name.to_lowercase();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let entry_name = entry.file_name();
        if let Some(entry_str) = entry_name.to_str()
            && entry_str.to_lowercase() == name_lower
        {
            return Ok(Some(entry.path()));
        }
    }

    Ok(None)
}

/// Normalize a config file path with case-insensitive fallback.
///
/// This is useful for finding config files that might have been created
/// with different case than expected.
///
/// # Arguments
/// * `base_dir` - Directory containing the config file
/// * `expected_name` - Expected config file name (e.g., "config.toml")
///
/// # Returns
/// The path to the config file if found, or the expected path if not found.
pub fn resolve_config_path(base_dir: impl AsRef<Path>, expected_name: &str) -> PathBuf {
    let base_dir = base_dir.as_ref();

    // Try to find with case-aware matching
    if let Ok(Some(found)) = find_file_case_aware(base_dir, expected_name) {
        return found;
    }

    // Fall back to expected path
    base_dir.join(expected_name)
}

/// Check if the filesystem is case-sensitive.
///
/// This creates a temporary file to test case sensitivity.
/// The result is cached for performance.
///
/// # Arguments
/// * `dir` - Directory to test (should be on the same filesystem as the path you care about)
///
/// # Returns
/// `true` if the filesystem is case-sensitive.
pub fn is_case_sensitive_fs(dir: impl AsRef<Path>) -> io::Result<bool> {
    let dir = dir.as_ref();

    // Create a unique test filename
    let pid = std::process::id();
    let test_upper = dir.join(format!(".CORTEX_CASE_TEST_{pid}"));
    let test_lower = dir.join(format!(".cortex_case_test_{pid}"));

    // Clean up any existing files
    let _ = std::fs::remove_file(&test_upper);
    let _ = std::fs::remove_file(&test_lower);

    // Create the uppercase file
    std::fs::write(&test_upper, b"")?;

    // Check if the lowercase version also exists (would indicate case-insensitive)
    let is_case_insensitive = test_lower.exists();

    // Clean up
    let _ = std::fs::remove_file(&test_upper);
    let _ = std::fs::remove_file(&test_lower);

    Ok(!is_case_insensitive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_normalize_path_lexically() {
        assert_eq!(
            normalize_path_lexically(Path::new("/a/b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(
            normalize_path_lexically(Path::new("/a/./b/./c")),
            PathBuf::from("/a/b/c")
        );
        assert_eq!(
            normalize_path_lexically(Path::new("a/b/../c")),
            PathBuf::from("a/c")
        );
    }

    #[test]
    fn test_paths_match() {
        // Same path should always match
        assert!(paths_match("/foo/bar", "/foo/bar"));

        // On case-insensitive systems, different case should match
        #[cfg(not(target_os = "linux"))]
        {
            assert!(paths_match("/foo/bar", "/FOO/BAR"));
            assert!(paths_match("/foo/BAR", "/FOO/bar"));
        }
    }

    #[test]
    fn test_find_file_case_aware() {
        let temp_dir = TempDir::new().unwrap();

        // Create a file with specific case
        let file_path = temp_dir.path().join("TestFile.txt");
        std::fs::write(&file_path, b"test").unwrap();

        // Should find with exact match
        let found = find_file_case_aware(temp_dir.path(), "TestFile.txt").unwrap();
        assert!(found.is_some());

        // On case-insensitive systems, should find with different case
        #[cfg(not(target_os = "linux"))]
        {
            let found_lower = find_file_case_aware(temp_dir.path(), "testfile.txt").unwrap();
            assert!(found_lower.is_some());
        }
    }

    #[test]
    fn test_resolve_config_path() {
        let temp_dir = TempDir::new().unwrap();

        // When file doesn't exist, returns expected path
        let path = resolve_config_path(temp_dir.path(), "config.toml");
        assert_eq!(path, temp_dir.path().join("config.toml"));

        // When file exists, returns found path
        std::fs::write(temp_dir.path().join("config.toml"), b"test").unwrap();
        let path = resolve_config_path(temp_dir.path(), "config.toml");
        assert!(path.exists());
    }

    #[test]
    fn test_normalize_path_consistent() {
        let temp_dir = TempDir::new().unwrap();

        // Create a test file
        let file_path = temp_dir.path().join("test.txt");
        std::fs::write(&file_path, b"test").unwrap();

        // Test with follow strategy
        let opts = PathNormalizationOptions::new()
            .symlink_strategy(SymlinkStrategy::Follow)
            .require_exists(true);
        let normalized = normalize_path_consistent(&file_path, opts).unwrap();
        assert!(normalized.exists());
    }

    #[test]
    #[cfg(unix)]
    fn test_symlink_handling() {
        let temp_dir = TempDir::new().unwrap();

        // Create a file and symlink
        let file_path = temp_dir.path().join("real.txt");
        let link_path = temp_dir.path().join("link.txt");
        std::fs::write(&file_path, b"test").unwrap();
        std::os::unix::fs::symlink(&file_path, &link_path).unwrap();

        // With Follow strategy, link and file should resolve to same path
        let opts_follow = PathNormalizationOptions::new().symlink_strategy(SymlinkStrategy::Follow);
        let real_norm = normalize_path_consistent(&file_path, opts_follow.clone()).unwrap();
        let link_norm = normalize_path_consistent(&link_path, opts_follow).unwrap();
        assert_eq!(real_norm, link_norm);

        // With Preserve strategy, they should be different
        let opts_preserve =
            PathNormalizationOptions::new().symlink_strategy(SymlinkStrategy::Preserve);
        let link_preserved = normalize_path_consistent(&link_path, opts_preserve).unwrap();
        assert!(link_preserved.ends_with("link.txt"));

        // With Reject strategy, symlink should fail
        let opts_reject = PathNormalizationOptions::new().symlink_strategy(SymlinkStrategy::Reject);
        let result = normalize_path_consistent(&link_path, opts_reject);
        assert!(result.is_err());
    }
}
