//! Path utilities for safe path handling and validation.
//!
//! This module provides utilities for:
//! - Normalizing paths (resolving `.` and `..` components)
//! - Expanding home directory (`~`)
//! - Validating paths are within allowed roots
//! - Ensuring parent directories exist
//!
//! # Security
//! These utilities help prevent path traversal attacks by:
//! - Normalizing paths before validation
//! - Checking paths stay within allowed roots
//! - Validating symlink targets
//!
//! # Examples
//!
//! ```rust,ignore
//! use cortex_common::path_utils::*;
//! use std::path::Path;
//!
//! // Normalize a path
//! let normalized = normalize_path(Path::new("/a/b/../c"));
//! assert_eq!(normalized, std::path::PathBuf::from("/a/c"));
//!
//! // Expand home directory
//! let expanded = expand_home_path(Path::new("~/documents"));
//! // Returns something like /home/user/documents
//!
//! // Validate path is safe
//! let root = Path::new("/workspace");
//! let safe = validate_path_safe(Path::new("/workspace/src/main.rs"), root)?;
//! ```

use std::io;
use std::path::{Component, Path, PathBuf};

/// Errors that can occur during path operations.
#[derive(Debug, Clone)]
pub enum PathError {
    /// Path traversal detected outside allowed root.
    PathTraversal { path: String, root: String },
    /// Failed to canonicalize path.
    CanonicalizationFailed { path: String, reason: String },
    /// Failed to create parent directory.
    CreateDirFailed { path: String, reason: String },
    /// Symlink points outside allowed root.
    SymlinkEscape {
        link: String,
        target: String,
        root: String,
    },
    /// Path does not exist and allow_nonexistent is false.
    PathNotFound { path: String },
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::PathTraversal { path, root } => {
                write!(f, "Path '{}' is outside allowed root '{}'", path, root)
            }
            PathError::CanonicalizationFailed { path, reason } => {
                write!(f, "Failed to canonicalize '{}': {}", path, reason)
            }
            PathError::CreateDirFailed { path, reason } => {
                write!(f, "Failed to create directory '{}': {}", path, reason)
            }
            PathError::SymlinkEscape { link, target, root } => {
                write!(
                    f,
                    "Symlink '{}' points to '{}' which is outside root '{}'",
                    link, target, root
                )
            }
            PathError::PathNotFound { path } => {
                write!(f, "Path '{}' does not exist", path)
            }
        }
    }
}

impl std::error::Error for PathError {}

/// Result type for path operations.
pub type PathResult<T> = Result<T, PathError>;

/// Normalizes a path by resolving `.` and `..` components without filesystem access.
///
/// This function:
/// - Removes `.` (current directory) components
/// - Resolves `..` (parent directory) components
/// - Does NOT access the filesystem
/// - Works with both absolute and relative paths
///
/// # Arguments
/// * `path` - The path to normalize
///
/// # Returns
/// A normalized `PathBuf` with traversal sequences resolved.
///
/// # Examples
/// ```rust,ignore
/// use cortex_common::path_utils::normalize_path;
/// use std::path::Path;
///
/// let path = Path::new("/a/b/../c/./d");
/// let normalized = normalize_path(path);
/// assert_eq!(normalized, std::path::PathBuf::from("/a/c/d"));
/// ```
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Go up one level, but don't go above root
                if !normalized.pop() {
                    // If we can't pop, we're at root level
                    // For relative paths this could be problematic
                    if !path.is_absolute() {
                        normalized.push("..");
                    }
                }
            }
            Component::CurDir => {
                // Current directory - skip it
            }
            _ => {
                normalized.push(component);
            }
        }
    }

    normalized
}

/// Expands the home directory (`~`) in a path.
///
/// This function:
/// - Replaces `~` at the start of a path with the user's home directory
/// - Returns the path unchanged if it doesn't start with `~`
/// - Returns an error if home directory cannot be determined
///
/// # Arguments
/// * `path` - The path to expand
///
/// # Returns
/// * `Ok(PathBuf)` - The path with `~` expanded to home directory
/// * `Err(PathError)` - If home directory cannot be determined
///
/// # Examples
/// ```rust,ignore
/// use cortex_common::path_utils::expand_home_path;
/// use std::path::Path;
///
/// let path = Path::new("~/documents/file.txt");
/// let expanded = expand_home_path(path)?;
/// // Returns something like /home/user/documents/file.txt
/// ```
pub fn expand_home_path(path: &Path) -> PathResult<PathBuf> {
    let path_str = path.to_string_lossy();

    if !path_str.starts_with('~') {
        return Ok(path.to_path_buf());
    }

    let home = dirs::home_dir().ok_or_else(|| PathError::CanonicalizationFailed {
        path: path.display().to_string(),
        reason: "Could not determine home directory".to_string(),
    })?;

    if path_str == "~" {
        Ok(home)
    } else if path_str.starts_with("~/") {
        Ok(home.join(&path_str[2..]))
    } else {
        // Path like ~user/something - not supported, return as-is
        Ok(path.to_path_buf())
    }
}

/// Validates that a path is within a specified root directory.
///
/// This function:
/// 1. Normalizes both paths to resolve `.` and `..`
/// 2. Checks that the normalized path starts with the root
/// 3. Optionally validates symlinks don't escape
/// 4. Optionally requires the path to exist
///
/// # Arguments
/// * `path` - The path to validate
/// * `root` - The allowed root directory
///
/// # Returns
/// * `Ok(PathBuf)` - The validated safe path
/// * `Err(PathError)` - If validation fails
///
/// # Examples
/// ```rust,ignore
/// use cortex_common::path_utils::validate_path_safe;
/// use std::path::Path;
///
/// let root = Path::new("/workspace");
/// let safe = validate_path_safe(Path::new("/workspace/src/main.rs"), root)?;
/// ```
pub fn validate_path_safe(path: &Path, root: &Path) -> PathResult<PathBuf> {
    // Get canonical root if it exists, otherwise normalize
    let canonical_root = if root.exists() {
        root.canonicalize()
            .map_err(|e| PathError::CanonicalizationFailed {
                path: root.display().to_string(),
                reason: e.to_string(),
            })?
    } else {
        normalize_path(root)
    };

    // Handle the target path
    let validated_path = if path.exists() {
        // Path exists - canonicalize it
        path.canonicalize()
            .map_err(|e| PathError::CanonicalizationFailed {
                path: path.display().to_string(),
                reason: e.to_string(),
            })?
    } else {
        // Path doesn't exist - normalize it
        // If the path is relative, make it absolute relative to root
        let absolute_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            canonical_root.join(path)
        };
        let normalized = normalize_path(&absolute_path);

        // If the parent directory exists, canonicalize it to resolve symlinks
        // (important for macOS where /var -> /private/var)
        if let Some(parent) = normalized.parent() {
            if parent.exists() {
                if let Ok(canonical_parent) = parent.canonicalize() {
                    if let Some(file_name) = normalized.file_name() {
                        canonical_parent.join(file_name)
                    } else {
                        normalized
                    }
                } else {
                    normalized
                }
            } else {
                normalized
            }
        } else {
            normalized
        }
    };

    // Final validation: ensure path is within root
    if !validated_path.starts_with(&canonical_root) {
        return Err(PathError::PathTraversal {
            path: validated_path.display().to_string(),
            root: canonical_root.display().to_string(),
        });
    }

    // Check symlinks for existing paths
    if path.exists()
        && let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
        && let Ok(target) = std::fs::read_link(path)
    {
        let absolute_target = if target.is_absolute() {
            target
        } else {
            path.parent().map(|p| p.join(&target)).unwrap_or(target)
        };

        let target_canonical = if absolute_target.exists() {
            absolute_target
                .canonicalize()
                .map_err(|e| PathError::CanonicalizationFailed {
                    path: absolute_target.display().to_string(),
                    reason: e.to_string(),
                })?
        } else {
            normalize_path(&absolute_target)
        };

        if !target_canonical.starts_with(&canonical_root) {
            return Err(PathError::SymlinkEscape {
                link: path.display().to_string(),
                target: target_canonical.display().to_string(),
                root: canonical_root.display().to_string(),
            });
        }
    }

    Ok(validated_path)
}

/// Ensures that the parent directory of a path exists, creating it if necessary.
///
/// This function:
/// - Creates all parent directories if they don't exist
/// - Does nothing if parent already exists
/// - Returns the path if successful
/// - Returns an error if creation fails
///
/// # Arguments
/// * `path` - The path whose parent directory should be ensured
///
/// # Returns
/// * `Ok(PathBuf)` - The original path
/// * `Err(PathError)` - If parent directory creation fails
///
/// # Examples
/// ```rust,ignore
/// use cortex_common::path_utils::ensure_parent_dir;
/// use std::path::Path;
///
/// let path = Path::new("/tmp/new_dir/file.txt");
/// let result = ensure_parent_dir(path)?;
/// // /tmp/new_dir/ now exists
/// ```
pub fn ensure_parent_dir(path: &Path) -> PathResult<PathBuf> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|e| PathError::CreateDirFailed {
            path: parent.display().to_string(),
            reason: e.to_string(),
        })?;
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    #[cfg_attr(windows, ignore = "Unix path format not applicable on Windows")]
    fn test_normalize_path_simple() {
        let path = Path::new("/a/b/../c");
        assert_eq!(normalize_path(path), PathBuf::from("/a/c"));
    }

    #[test]
    #[cfg_attr(windows, ignore = "Unix path format not applicable on Windows")]
    fn test_normalize_path_multiple_parent_dirs() {
        let path = Path::new("/a/b/c/../../d");
        assert_eq!(normalize_path(path), PathBuf::from("/a/d"));
    }

    #[test]
    #[cfg_attr(windows, ignore = "Unix path format not applicable on Windows")]
    fn test_normalize_path_current_dir() {
        let path = Path::new("/a/./b/./c");
        assert_eq!(normalize_path(path), PathBuf::from("/a/b/c"));
    }

    #[test]
    #[cfg_attr(windows, ignore = "Unix path format not applicable on Windows")]
    fn test_normalize_path_mixed() {
        let path = Path::new("/a/./b/../c/./d/../e");
        assert_eq!(normalize_path(path), PathBuf::from("/a/c/e"));
    }

    #[test]
    fn test_normalize_path_relative() {
        let path = Path::new("a/b/../c");
        assert_eq!(normalize_path(path), PathBuf::from("a/c"));
    }

    #[test]
    fn test_normalize_path_relative_parent() {
        let path = Path::new("a/b/../../c");
        // When we go above the root of a relative path, we can't go further
        // So a/b/../../c becomes c (we pop a, then b, then try to pop again but can't)
        assert_eq!(normalize_path(path), PathBuf::from("c"));
    }

    #[test]
    fn test_expand_home_path_no_tilde() {
        let path = Path::new("/home/user/documents");
        let expanded = expand_home_path(path).unwrap();
        assert_eq!(expanded, PathBuf::from("/home/user/documents"));
    }

    #[test]
    fn test_expand_home_path_with_tilde() {
        let path = Path::new("~/documents");
        let expanded = expand_home_path(path).unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(expanded, home.join("documents"));
    }

    #[test]
    fn test_expand_home_path_tilde_only() {
        let path = Path::new("~");
        let expanded = expand_home_path(path).unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(expanded, home);
    }

    #[test]
    fn test_expand_home_path_tilde_in_middle() {
        // Tilde in the middle of a path should NOT be expanded
        // This tests the case where a username contains a tilde (e.g., /home/test~user)
        let path = Path::new("/home/test~user/.cortex");
        let expanded = expand_home_path(path).unwrap();
        // Path should be unchanged since tilde is not at the start
        assert_eq!(expanded, PathBuf::from("/home/test~user/.cortex"));
    }

    #[test]
    fn test_expand_home_path_tilde_in_filename() {
        // Tilde in a filename should NOT be expanded
        let path = Path::new("/home/user/backup~file.txt");
        let expanded = expand_home_path(path).unwrap();
        assert_eq!(expanded, PathBuf::from("/home/user/backup~file.txt"));
    }

    #[test]
    fn test_validate_path_safe_within_root() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test file
        fs::write(root.join("file.txt"), "test").unwrap();

        // Valid path should pass
        let result = validate_path_safe(&root.join("file.txt"), root);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_safe_traversal_attempt() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test structure
        fs::create_dir_all(root.join("subdir")).unwrap();

        // Path traversal should fail
        let traversal = root.join("subdir/../../../etc/passwd");
        let result = validate_path_safe(&traversal, root);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_path_safe_nonexistent_within_root() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Non-existent but valid path should pass
        let new_file = root.join("new_file.txt");
        let result = validate_path_safe(&new_file, root);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_safe_nonexistent_traversal() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Non-existent path with traversal should fail
        let traversal = root.join("../outside.txt");
        let result = validate_path_safe(&traversal, root);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_path_safe_relative_path() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create subdir
        fs::create_dir_all(root.join("subdir")).unwrap();

        // Relative path within root should pass
        let result = validate_path_safe(Path::new("file.txt"), root);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ensure_parent_dir_creates_single_level() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("newdir").join("file.txt");

        // Parent should not exist yet
        assert!(!path.parent().unwrap().exists());

        // Ensure parent
        let result = ensure_parent_dir(&path);
        assert!(result.is_ok());

        // Parent should now exist
        assert!(path.parent().unwrap().exists());
    }

    #[test]
    fn test_ensure_parent_dir_creates_multiple_levels() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir
            .path()
            .join("a")
            .join("b")
            .join("c")
            .join("file.txt");

        // Ensure parent
        let result = ensure_parent_dir(&path);
        assert!(result.is_ok());

        // All parents should exist
        assert!(path.parent().unwrap().exists());
    }

    #[test]
    fn test_ensure_parent_dir_already_exists() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("file.txt");

        // Parent already exists
        assert!(path.parent().unwrap().exists());

        // Ensure parent should still succeed
        let result = ensure_parent_dir(&path);
        assert!(result.is_ok());
    }

    #[test]
    #[cfg_attr(windows, ignore = "Unix root path not applicable on Windows")]
    fn test_ensure_parent_dir_root_path() {
        let path = Path::new("/file.txt");

        // Root path should succeed (parent is root)
        let result = ensure_parent_dir(path);
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_validate_path_safe_symlink_within_root() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create a file and a symlink within root
        let file = root.join("file.txt");
        fs::write(&file, "test").unwrap();
        let symlink = root.join("link.txt");
        std::os::unix::fs::symlink(&file, &symlink).unwrap();

        // Symlink within root should pass
        let result = validate_path_safe(&symlink, root);
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_validate_path_safe_symlink_escape() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create a file outside root
        let outside_file = temp_dir.path().parent().unwrap().join("outside.txt");
        fs::write(&outside_file, "outside").unwrap();

        // Create a symlink inside root pointing outside
        let escape_link = root.join("escape_link.txt");
        std::os::unix::fs::symlink(&outside_file, &escape_link).unwrap();

        // Symlink escaping root should fail
        let result = validate_path_safe(&escape_link, root);
        assert!(result.is_err());
    }
}
