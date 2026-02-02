//! Path traversal protection and validation utilities.
//!
//! This module provides robust path validation to prevent directory traversal attacks
//! and ensure file operations stay within allowed directories.
//!
//! # Security Features
//! - Canonicalization of paths before validation
//! - Detection of path traversal sequences (`.`, `..`)
//! - Symlink following prevention (optionally)
//! - Prefix-based allowed directory validation
//!
//! # Example
//! ```rust,ignore
//! use cortex_engine::security::path_safety::{validate_path_within_root, PathValidationError};
//!
//! let root = std::path::Path::new("/workspace");
//! let file_path = std::path::Path::new("/workspace/src/main.rs");
//!
//! // This succeeds - path is within root
//! validate_path_within_root(file_path, root).unwrap();
//!
//! // This fails - path traversal attempt
//! let malicious = std::path::Path::new("/workspace/../etc/passwd");
//! assert!(validate_path_within_root(malicious, root).is_err());
//! ```

use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during path validation.
#[derive(Debug, Error)]
pub enum PathValidationError {
    /// The path attempts to traverse outside allowed directories.
    #[error("Path traversal detected: {path} is outside allowed root {root}")]
    PathTraversal { path: String, root: String },

    /// The path contains forbidden sequences after normalization.
    #[error("Path contains forbidden sequence: {sequence} in {path}")]
    ForbiddenSequence { path: String, sequence: String },

    /// The path could not be canonicalized.
    #[error("Failed to canonicalize path {path}: {source}")]
    CanonicalizationFailed {
        path: String,
        #[source]
        source: io::Error,
    },

    /// The symlink target is outside the allowed directory.
    #[error("Symlink {link} resolves to {target} which is outside allowed root {root}")]
    SymlinkEscape {
        link: String,
        target: String,
        root: String,
    },

    /// The path is not absolute.
    #[error("Path must be absolute: {path}")]
    NotAbsolute { path: String },

    /// The root directory does not exist.
    #[error("Root directory does not exist: {root}")]
    RootNotFound { root: String },
}

/// Result type for path validation operations.
pub type PathValidationResult<T> = Result<T, PathValidationError>;

/// Options for path validation behavior.
#[derive(Debug, Clone)]
pub struct PathValidationOptions {
    /// Whether to follow symlinks during validation.
    /// If false, symlinks are validated but not followed.
    pub follow_symlinks: bool,

    /// Whether to allow paths to non-existent files.
    /// If true, only validates the path structure without checking existence.
    pub allow_nonexistent: bool,

    /// Additional forbidden path components (beyond `.` and `..`).
    pub forbidden_components: Vec<String>,
}

impl Default for PathValidationOptions {
    fn default() -> Self {
        Self {
            follow_symlinks: false,
            allow_nonexistent: true,
            forbidden_components: Vec::new(),
        }
    }
}

impl PathValidationOptions {
    /// Create options that require paths to exist.
    pub fn require_existence() -> Self {
        Self {
            follow_symlinks: false,
            allow_nonexistent: false,
            forbidden_components: Vec::new(),
        }
    }

    /// Create options that follow symlinks (less secure).
    pub fn follow_symlinks() -> Self {
        Self {
            follow_symlinks: true,
            allow_nonexistent: true,
            forbidden_components: Vec::new(),
        }
    }
}

/// Normalizes a path by resolving `.` and `..` components without accessing the filesystem.
///
/// This is useful for validating paths that may not exist yet.
/// The result is a path with all `.` and `..` components resolved.
///
/// # Arguments
/// * `path` - The path to normalize
///
/// # Returns
/// A normalized `PathBuf` with traversal sequences resolved.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                // Go up one level, but don't go above root
                if !normalized.pop() {
                    // If we can't pop, we're at root level
                    // For relative paths this could be problematic
                    normalized.push("..");
                }
            }
            std::path::Component::CurDir => {
                // Current directory - skip it
            }
            _ => {
                normalized.push(component);
            }
        }
    }

    normalized
}

/// Validates that a path is within a specified root directory.
///
/// This function:
/// 1. Normalizes both paths to resolve `.` and `..`
/// 2. Checks that the normalized path starts with the root
/// 3. Optionally validates symlinks don't escape
///
/// # Arguments
/// * `path` - The path to validate
/// * `root` - The allowed root directory
///
/// # Returns
/// * `Ok(PathBuf)` - The canonicalized/normalized safe path
/// * `Err(PathValidationError)` - If validation fails
///
/// # Example
/// ```rust,ignore
/// let safe_path = validate_path_within_root(
///     Path::new("/workspace/src/file.rs"),
///     Path::new("/workspace")
/// )?;
/// ```
pub fn validate_path_within_root(path: &Path, root: &Path) -> PathValidationResult<PathBuf> {
    validate_path_within_root_with_options(path, root, &PathValidationOptions::default())
}

/// Validates a path within a root directory with custom options.
///
/// # Arguments
/// * `path` - The path to validate
/// * `root` - The allowed root directory
/// * `options` - Validation options
///
/// # Returns
/// * `Ok(PathBuf)` - The validated safe path
/// * `Err(PathValidationError)` - If validation fails
pub fn validate_path_within_root_with_options(
    path: &Path,
    root: &Path,
    options: &PathValidationOptions,
) -> PathValidationResult<PathBuf> {
    // First, try to canonicalize the root if it exists
    let canonical_root = if root.exists() {
        root.canonicalize()
            .map_err(|e| PathValidationError::CanonicalizationFailed {
                path: root.display().to_string(),
                source: e,
            })?
    } else {
        normalize_path(root)
    };

    // Check for forbidden components in the original path
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            let name_str = name.to_string_lossy();
            for forbidden in &options.forbidden_components {
                if name_str == *forbidden {
                    return Err(PathValidationError::ForbiddenSequence {
                        path: path.display().to_string(),
                        sequence: forbidden.clone(),
                    });
                }
            }
        }
    }

    // Now handle the target path
    let validated_path = if path.exists() {
        // Path exists - we can canonicalize it
        if options.follow_symlinks {
            // Follow symlinks and get the real path
            path.canonicalize()
                .map_err(|e| PathValidationError::CanonicalizationFailed {
                    path: path.display().to_string(),
                    source: e,
                })?
        } else {
            // Check if it's a symlink
            let metadata = std::fs::symlink_metadata(path).map_err(|e| {
                PathValidationError::CanonicalizationFailed {
                    path: path.display().to_string(),
                    source: e,
                }
            })?;

            if metadata.file_type().is_symlink() {
                // Get the symlink target and validate it
                let target = std::fs::read_link(path).map_err(|e| {
                    PathValidationError::CanonicalizationFailed {
                        path: path.display().to_string(),
                        source: e,
                    }
                })?;

                // Resolve the target relative to the symlink's parent
                let absolute_target = if target.is_absolute() {
                    target
                } else {
                    path.parent().map(|p| p.join(&target)).unwrap_or(target)
                };

                let canonical_target = if absolute_target.exists() {
                    absolute_target.canonicalize().map_err(|e| {
                        PathValidationError::CanonicalizationFailed {
                            path: absolute_target.display().to_string(),
                            source: e,
                        }
                    })?
                } else {
                    normalize_path(&absolute_target)
                };

                // Validate the symlink target is within root
                if !canonical_target.starts_with(&canonical_root) {
                    return Err(PathValidationError::SymlinkEscape {
                        link: path.display().to_string(),
                        target: canonical_target.display().to_string(),
                        root: canonical_root.display().to_string(),
                    });
                }

                // Return the canonicalized path of the symlink itself
                path.canonicalize()
                    .map_err(|e| PathValidationError::CanonicalizationFailed {
                        path: path.display().to_string(),
                        source: e,
                    })?
            } else {
                path.canonicalize()
                    .map_err(|e| PathValidationError::CanonicalizationFailed {
                        path: path.display().to_string(),
                        source: e,
                    })?
            }
        }
    } else if options.allow_nonexistent {
        // Path doesn't exist - normalize it manually
        // If the path is relative, make it absolute relative to root
        let absolute_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            canonical_root.join(path)
        };
        normalize_path(&absolute_path)
    } else {
        return Err(PathValidationError::CanonicalizationFailed {
            path: path.display().to_string(),
            source: io::Error::new(io::ErrorKind::NotFound, "Path does not exist"),
        });
    };

    // Final validation: ensure path is within root
    if !validated_path.starts_with(&canonical_root) {
        return Err(PathValidationError::PathTraversal {
            path: validated_path.display().to_string(),
            root: canonical_root.display().to_string(),
        });
    }

    Ok(validated_path)
}

/// Validates that a path is within any of the specified allowed roots.
///
/// # Arguments
/// * `path` - The path to validate
/// * `allowed_roots` - A list of allowed root directories
///
/// # Returns
/// * `Ok(PathBuf)` - The canonicalized safe path
/// * `Err(PathValidationError)` - If validation fails for all roots
pub fn validate_path_within_any_root(
    path: &Path,
    allowed_roots: &[PathBuf],
) -> PathValidationResult<PathBuf> {
    validate_path_within_any_root_with_options(
        path,
        allowed_roots,
        &PathValidationOptions::default(),
    )
}

/// Validates that a path is within any of the allowed roots with custom options.
pub fn validate_path_within_any_root_with_options(
    path: &Path,
    allowed_roots: &[PathBuf],
    options: &PathValidationOptions,
) -> PathValidationResult<PathBuf> {
    if allowed_roots.is_empty() {
        return Err(PathValidationError::RootNotFound {
            root: "(no roots provided)".to_string(),
        });
    }

    let mut last_error = None;
    for root in allowed_roots {
        match validate_path_within_root_with_options(path, root, options) {
            Ok(validated) => return Ok(validated),
            Err(e) => last_error = Some(e),
        }
    }

    // Return the last error if all validations failed
    Err(
        last_error.unwrap_or_else(|| PathValidationError::PathTraversal {
            path: path.display().to_string(),
            root: "(no roots provided)".to_string(),
        }),
    )
}

/// Checks if a path contains any path traversal sequences.
///
/// This is a quick check that doesn't access the filesystem.
/// For thorough validation, use `validate_path_within_root`.
///
/// # Arguments
/// * `path` - The path to check
///
/// # Returns
/// `true` if the path contains `..` components, `false` otherwise
pub fn contains_traversal(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Sanitizes a filename by removing potentially dangerous characters.
///
/// This removes:
/// - Path separators (`/`, `\`)
/// - Parent directory markers (`.`)
/// - Null bytes
/// - Control characters
///
/// # Arguments
/// * `filename` - The filename to sanitize
///
/// # Returns
/// A sanitized filename safe for use in file operations
pub fn sanitize_filename(filename: &str) -> String {
    filename
        .chars()
        .filter(|c| {
            !matches!(c, '/' | '\\' | '\0') && !c.is_control() && *c != ':' // Windows drive separator
        })
        .collect::<String>()
        .trim_start_matches('.') // Remove leading dots
        .to_string()
}

/// Validates a path for ZIP extraction to prevent Zip Slip attacks.
///
/// This ensures that extracted files cannot escape the destination directory
/// through malicious paths like `../../../etc/passwd`.
///
/// # Arguments
/// * `zip_entry_path` - The path from within the ZIP archive
/// * `dest_dir` - The destination extraction directory
///
/// # Returns
/// * `Ok(PathBuf)` - The safe absolute path for extraction
/// * `Err(PathValidationError)` - If the path would escape the destination
pub fn validate_zip_entry_path(
    zip_entry_path: &Path,
    dest_dir: &Path,
) -> PathValidationResult<PathBuf> {
    // Check for absolute paths (they should be relative in ZIP)
    if zip_entry_path.is_absolute() {
        return Err(PathValidationError::ForbiddenSequence {
            path: zip_entry_path.display().to_string(),
            sequence: "absolute path in ZIP entry".to_string(),
        });
    }

    // Check for traversal attempts
    if contains_traversal(zip_entry_path) {
        return Err(PathValidationError::PathTraversal {
            path: zip_entry_path.display().to_string(),
            root: dest_dir.display().to_string(),
        });
    }

    // Build the full path and normalize it
    let full_path = dest_dir.join(zip_entry_path);
    let normalized = normalize_path(&full_path);

    // Get canonical dest_dir if it exists, otherwise normalize it
    let canonical_dest = if dest_dir.exists() {
        dest_dir
            .canonicalize()
            .map_err(|e| PathValidationError::CanonicalizationFailed {
                path: dest_dir.display().to_string(),
                source: e,
            })?
    } else {
        normalize_path(dest_dir)
    };

    // Verify the normalized path is within dest_dir
    if !normalized.starts_with(&canonical_dest) {
        return Err(PathValidationError::PathTraversal {
            path: zip_entry_path.display().to_string(),
            root: dest_dir.display().to_string(),
        });
    }

    Ok(normalized)
}

/// Resolves a path relative to a working directory with traversal protection.
///
/// If the input path is absolute, it's validated directly.
/// If relative, it's joined to the working directory first.
///
/// # Arguments
/// * `path` - The path to resolve (absolute or relative)
/// * `cwd` - The current working directory for relative paths
/// * `allowed_root` - The root directory that paths must stay within
///
/// # Returns
/// * `Ok(PathBuf)` - The resolved and validated path
/// * `Err(PathValidationError)` - If validation fails
pub fn resolve_and_validate_path(
    path: &Path,
    cwd: &Path,
    allowed_root: &Path,
) -> PathValidationResult<PathBuf> {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    validate_path_within_root(&absolute_path, allowed_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    #[cfg_attr(windows, ignore = "Unix path format not applicable on Windows")]
    fn test_normalize_path() {
        // Simple normalization
        let path = Path::new("/a/b/../c");
        assert_eq!(normalize_path(path), PathBuf::from("/a/c"));

        // Multiple parent dirs
        let path = Path::new("/a/b/c/../../d");
        assert_eq!(normalize_path(path), PathBuf::from("/a/d"));

        // Current dir
        let path = Path::new("/a/./b/./c");
        assert_eq!(normalize_path(path), PathBuf::from("/a/b/c"));

        // Mixed
        let path = Path::new("/a/./b/../c/./d/../e");
        assert_eq!(normalize_path(path), PathBuf::from("/a/c/e"));
    }

    #[test]
    fn test_contains_traversal() {
        assert!(contains_traversal(Path::new("../file")));
        assert!(contains_traversal(Path::new("dir/../file")));
        assert!(contains_traversal(Path::new("/a/b/../c")));
        assert!(!contains_traversal(Path::new("/a/b/c")));
        assert!(!contains_traversal(Path::new("file.txt")));
        assert!(!contains_traversal(Path::new("./file.txt")));
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("file.txt"), "file.txt");
        assert_eq!(sanitize_filename("../file.txt"), "file.txt");
        assert_eq!(sanitize_filename("dir/file.txt"), "dirfile.txt");
        assert_eq!(sanitize_filename("..\\..\\file.txt"), "file.txt");
        assert_eq!(sanitize_filename(".hidden"), "hidden");
        assert_eq!(sanitize_filename("C:file.txt"), "Cfile.txt");
    }

    #[test]
    fn test_validate_path_within_root() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test structure
        fs::create_dir_all(root.join("subdir")).unwrap();
        fs::write(root.join("file.txt"), "test").unwrap();
        fs::write(root.join("subdir/nested.txt"), "test").unwrap();

        // Valid paths should pass
        assert!(validate_path_within_root(&root.join("file.txt"), root).is_ok());
        assert!(validate_path_within_root(&root.join("subdir/nested.txt"), root).is_ok());

        // Path traversal should fail
        let traversal = root.join("subdir/../../../etc/passwd");
        assert!(validate_path_within_root(&traversal, root).is_err());
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Windows canonicalization uses extended path prefix causing comparison issues"
    )]
    #[cfg_attr(target_os = "macos", ignore = "macOS symlinks /var -> /private/var")]
    fn test_validate_zip_entry_path() {
        let temp_dir = TempDir::new().unwrap();
        let dest = temp_dir.path();

        // Valid entries
        assert!(validate_zip_entry_path(Path::new("file.txt"), dest).is_ok());
        assert!(validate_zip_entry_path(Path::new("dir/file.txt"), dest).is_ok());
        assert!(validate_zip_entry_path(Path::new("a/b/c/file.txt"), dest).is_ok());

        // Invalid entries (traversal)
        assert!(validate_zip_entry_path(Path::new("../file.txt"), dest).is_err());
        assert!(validate_zip_entry_path(Path::new("dir/../../file.txt"), dest).is_err());
        assert!(validate_zip_entry_path(Path::new("../../../etc/passwd"), dest).is_err());

        // Absolute paths in ZIP are forbidden
        assert!(validate_zip_entry_path(Path::new("/etc/passwd"), dest).is_err());
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Windows 8.3 path names cause path comparison issues"
    )]
    #[cfg_attr(target_os = "macos", ignore = "macOS symlinks /var -> /private/var")]
    fn test_validate_nonexistent_path() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Non-existent but valid path should pass with default options
        let new_file = root.join("new_file.txt");
        assert!(validate_path_within_root(&new_file, root).is_ok());

        // Non-existent path with traversal should fail
        let traversal = root.join("../outside.txt");
        assert!(validate_path_within_root(&traversal, root).is_err());
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Windows 8.3 path names cause path comparison issues"
    )]
    #[cfg_attr(target_os = "macos", ignore = "macOS symlinks /var -> /private/var")]
    fn test_resolve_and_validate_path() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let cwd = root.join("subdir");
        fs::create_dir_all(&cwd).unwrap();

        // Relative path should resolve within root
        let result = resolve_and_validate_path(Path::new("file.txt"), &cwd, root);
        assert!(result.is_ok());

        // Traversal should fail
        let result = resolve_and_validate_path(Path::new("../../outside.txt"), &cwd, root);
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_validation() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create a file and a symlink within root
        let file = root.join("file.txt");
        fs::write(&file, "test").unwrap();
        let symlink = root.join("link.txt");
        std::os::unix::fs::symlink(&file, &symlink).unwrap();

        // Symlink within root should pass
        assert!(validate_path_within_root(&symlink, root).is_ok());

        // Create a symlink pointing outside root
        let outside_target = temp_dir.path().parent().unwrap().join("outside.txt");
        fs::write(&outside_target, "outside").unwrap();
        let escape_link = root.join("escape_link.txt");
        std::os::unix::fs::symlink(&outside_target, &escape_link).unwrap();

        // Symlink escaping root should fail with default options
        let result = validate_path_within_root(&escape_link, root);
        assert!(result.is_err());
        if let Err(PathValidationError::SymlinkEscape { .. }) = result {
            // Expected
        } else {
            panic!("Expected SymlinkEscape error");
        }
    }
}
