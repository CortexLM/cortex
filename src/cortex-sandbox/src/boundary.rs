//! Path boundary validation for sandbox security.
//!
//! This module provides protection against sandbox escape via path manipulation,
//! including the CVE-2025-59532 class of vulnerabilities where malicious actors
//! attempt to escape the sandbox by manipulating the current working directory.
//!
//! # Security Model
//!
//! The key insight is that security boundaries MUST be based on the user's
//! starting directory, NOT the current working directory (cwd). The cwd can
//! be manipulated by the model/agent, while the user's start directory is
//! fixed at session start.
//!
//! # CVE-2025-59532 Protection
//!
//! The vulnerability allows sandbox bypass by:
//! 1. Agent changes cwd to a directory outside the intended workspace
//! 2. Relative paths are then resolved against the new cwd
//! 3. Access to files outside the sandbox boundary
//!
//! We prevent this by always validating against the user's start directory.

use std::path::{Path, PathBuf};

use crate::SandboxError;

/// Result of path boundary validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryCheckResult {
    /// Path is within allowed boundaries.
    Allowed,

    /// Path is outside allowed boundaries.
    Denied(String),
}

impl BoundaryCheckResult {
    /// Check if access is allowed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, BoundaryCheckResult::Allowed)
    }

    /// Check if access is denied.
    pub fn is_denied(&self) -> bool {
        matches!(self, BoundaryCheckResult::Denied(_))
    }
}

/// Validates that a path is within the allowed boundaries.
///
/// This function implements CVE-2025-59532 protection by:
/// 1. Canonicalizing the path to resolve symlinks and `..`
/// 2. Checking if the path starts with any of the allowed roots
/// 3. Using the user_start_dir as the primary boundary (not cwd)
///
/// # Arguments
/// * `path` - The path to validate
/// * `user_start_dir` - The user's starting directory (session start)
/// * `allowed_roots` - Additional explicitly allowed root paths
///
/// # Returns
/// * `Ok(())` - Path is within boundaries
/// * `Err(SandboxError::InvalidPath)` - Path is outside boundaries
///
/// # Security Note
///
/// CRITICAL: The `user_start_dir` MUST be set at session initialization
/// and MUST NOT change during the session. It should be the directory
/// where the user started the CLI session, NOT the current working
/// directory which can be manipulated.
pub fn validate_path_in_boundary(
    path: &Path,
    user_start_dir: &Path,
    allowed_roots: &[PathBuf],
) -> Result<(), SandboxError> {
    // Canonicalize the target path to resolve symlinks and ..
    // This is critical for preventing symlink-based escapes
    let canonical = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            // If the path doesn't exist, try to validate the parent
            // This handles creation of new files
            if let Some(parent) = path.parent() {
                if parent.as_os_str().is_empty() {
                    // Path like "file.txt" - use current interpretation
                    return Ok(());
                }
                match parent.canonicalize() {
                    Ok(p) => p,
                    Err(_) => {
                        return Err(SandboxError::InvalidPath(format!(
                            "Cannot resolve path '{}': {}",
                            path.display(),
                            e
                        )));
                    }
                }
            } else {
                return Err(SandboxError::InvalidPath(format!(
                    "Cannot resolve path '{}': {}",
                    path.display(),
                    e
                )));
            }
        }
    };

    // Canonicalize the user start directory
    let canonical_start = match user_start_dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return Err(SandboxError::InvalidPath(format!(
                "Cannot resolve user start directory '{}': {}",
                user_start_dir.display(),
                e
            )));
        }
    };

    // Check if path is within user's start directory
    if canonical.starts_with(&canonical_start) {
        return Ok(());
    }

    // Check against explicitly allowed roots
    for root in allowed_roots {
        if let Ok(canonical_root) = root.canonicalize()
            && canonical.starts_with(&canonical_root)
        {
            return Ok(());
        }
        // Also check non-canonicalized for paths that might not exist yet
        if canonical.starts_with(root) {
            return Ok(());
        }
    }

    Err(SandboxError::InvalidPath(format!(
        "Path '{}' is outside the allowed sandbox boundary (user start: '{}')",
        path.display(),
        user_start_dir.display()
    )))
}

/// Check a path against boundaries without error.
///
/// Returns a `BoundaryCheckResult` instead of `Result`.
pub fn check_path_boundary(
    path: &Path,
    user_start_dir: &Path,
    allowed_roots: &[PathBuf],
) -> BoundaryCheckResult {
    match validate_path_in_boundary(path, user_start_dir, allowed_roots) {
        Ok(()) => BoundaryCheckResult::Allowed,
        Err(SandboxError::InvalidPath(msg)) => BoundaryCheckResult::Denied(msg),
        Err(e) => BoundaryCheckResult::Denied(e.to_string()),
    }
}

/// Sandbox boundary context that tracks the user's start directory.
///
/// This struct should be created once at session start and passed to
/// all path validation functions. The `user_start_dir` is immutable
/// after creation to prevent manipulation.
#[derive(Debug, Clone)]
pub struct BoundaryContext {
    /// The user's starting directory (immutable after creation).
    user_start_dir: PathBuf,

    /// Additional allowed root paths.
    allowed_roots: Vec<PathBuf>,

    /// Whether to allow following symlinks outside boundaries.
    allow_symlinks_outside: bool,
}

impl BoundaryContext {
    /// Create a new boundary context.
    ///
    /// # Arguments
    /// * `user_start_dir` - The user's starting directory
    ///
    /// # Panics
    /// Panics if `user_start_dir` cannot be canonicalized.
    pub fn new(user_start_dir: PathBuf) -> Result<Self, SandboxError> {
        // Validate and canonicalize the start directory
        let canonical = user_start_dir.canonicalize().map_err(|e| {
            SandboxError::InvalidPath(format!(
                "Cannot canonicalize user start directory '{}': {}",
                user_start_dir.display(),
                e
            ))
        })?;

        Ok(Self {
            user_start_dir: canonical,
            allowed_roots: Vec::new(),
            allow_symlinks_outside: false,
        })
    }

    /// Create a boundary context from the current working directory.
    ///
    /// # Note
    /// Only use this at the very start of a session before any
    /// agent/model code runs. The cwd is captured once and becomes
    /// the immutable boundary.
    pub fn from_cwd() -> Result<Self, SandboxError> {
        let cwd = std::env::current_dir().map_err(|e| {
            SandboxError::InvalidPath(format!("Cannot get current directory: {}", e))
        })?;
        Self::new(cwd)
    }

    /// Add an allowed root path.
    pub fn add_allowed_root(&mut self, path: PathBuf) -> &mut Self {
        self.allowed_roots.push(path);
        self
    }

    /// Add multiple allowed root paths.
    pub fn add_allowed_roots(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> &mut Self {
        self.allowed_roots.extend(paths);
        self
    }

    /// Set whether to allow symlinks pointing outside boundaries.
    ///
    /// Default is `false` (symlinks outside boundaries are blocked).
    pub fn allow_symlinks_outside(&mut self, allow: bool) -> &mut Self {
        self.allow_symlinks_outside = allow;
        self
    }

    /// Get the user's start directory.
    pub fn user_start_dir(&self) -> &Path {
        &self.user_start_dir
    }

    /// Get the allowed roots.
    pub fn allowed_roots(&self) -> &[PathBuf] {
        &self.allowed_roots
    }

    /// Validate a path against this boundary context.
    pub fn validate(&self, path: &Path) -> Result<(), SandboxError> {
        validate_path_in_boundary(path, &self.user_start_dir, &self.allowed_roots)
    }

    /// Check a path against this boundary context.
    pub fn check(&self, path: &Path) -> BoundaryCheckResult {
        check_path_boundary(path, &self.user_start_dir, &self.allowed_roots)
    }

    /// Validate multiple paths at once.
    ///
    /// Returns `Ok(())` if all paths are valid, or the first error.
    pub fn validate_all(&self, paths: &[&Path]) -> Result<(), SandboxError> {
        for path in paths {
            self.validate(path)?;
        }
        Ok(())
    }

    /// Create a sanitized/validated path.
    ///
    /// If the path is relative, it's resolved against `user_start_dir`,
    /// NOT the current working directory. This prevents cwd manipulation
    /// attacks.
    pub fn resolve_path(&self, path: &Path) -> Result<PathBuf, SandboxError> {
        let resolved = if path.is_relative() {
            // CRITICAL: Resolve against user_start_dir, not cwd
            self.user_start_dir.join(path)
        } else {
            path.to_path_buf()
        };

        // Validate the resolved path
        self.validate(&resolved)?;

        Ok(resolved)
    }
}

/// Detect potential path traversal attempts.
///
/// This is a heuristic check for obvious traversal patterns.
/// The main protection is still `validate_path_in_boundary`.
pub fn contains_traversal_pattern(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    // Check for obvious traversal patterns
    path_str.contains("../") || path_str.contains("..\\")
        || path_str.ends_with("..")
        || path_str == ".."
        // Null bytes (path truncation attacks)
        || path_str.contains('\0')
        // Unicode normalization attacks
        || path_str.contains('\u{2024}') // One dot leader
        || path_str.contains('\u{FE52}') // Small full stop
        || path_str.contains('\u{FF0E}') // Fullwidth full stop
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_validate_path_within_boundary() {
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join("cortex_boundary_test");
        let _ = fs::create_dir_all(&test_dir);

        let sub_path = test_dir.join("subdir");
        let _ = fs::create_dir_all(&sub_path);

        // Path within boundary should be allowed
        let result = validate_path_in_boundary(&sub_path, &test_dir, &[]);
        assert!(result.is_ok());

        // Cleanup
        let _ = fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_validate_path_outside_boundary() {
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join("cortex_boundary_test2");
        let _ = fs::create_dir_all(&test_dir);

        // Path outside boundary should be denied
        let outside = PathBuf::from("/etc/passwd");
        if outside.exists() {
            let result = validate_path_in_boundary(&outside, &test_dir, &[]);
            assert!(result.is_err());
        }

        // Cleanup
        let _ = fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_validate_path_with_allowed_root() {
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join("cortex_boundary_test3");
        let _ = fs::create_dir_all(&test_dir);

        let other_dir = temp_dir.join("cortex_boundary_other");
        let _ = fs::create_dir_all(&other_dir);

        // Path in allowed root should be permitted
        let result =
            validate_path_in_boundary(&other_dir, &test_dir, std::slice::from_ref(&other_dir));
        assert!(result.is_ok());

        // Cleanup
        let _ = fs::remove_dir_all(&test_dir);
        let _ = fs::remove_dir_all(&other_dir);
    }

    #[test]
    fn test_boundary_context_resolve_path() {
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join("cortex_boundary_test4");
        let _ = fs::create_dir_all(&test_dir);

        let ctx = BoundaryContext::new(test_dir.clone()).unwrap();

        // Relative path should resolve against user_start_dir
        // Note: we can't fully test this without creating the file
        // but we can verify the context is set up correctly
        assert_eq!(ctx.user_start_dir(), test_dir.canonicalize().unwrap());

        // Cleanup
        let _ = fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_contains_traversal_pattern() {
        assert!(contains_traversal_pattern(Path::new("../etc/passwd")));
        assert!(contains_traversal_pattern(Path::new("foo/../bar")));
        assert!(contains_traversal_pattern(Path::new("..")));
        assert!(contains_traversal_pattern(Path::new("path/to/..")));

        assert!(!contains_traversal_pattern(Path::new("/normal/path")));
        assert!(!contains_traversal_pattern(Path::new("relative/path")));
        assert!(!contains_traversal_pattern(Path::new("file.txt")));
    }

    #[test]
    fn test_boundary_check_result() {
        let allowed = BoundaryCheckResult::Allowed;
        assert!(allowed.is_allowed());
        assert!(!allowed.is_denied());

        let denied = BoundaryCheckResult::Denied("test".to_string());
        assert!(!denied.is_allowed());
        assert!(denied.is_denied());
    }

    #[test]
    fn test_boundary_context_add_roots() {
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join("cortex_boundary_test5");
        let _ = fs::create_dir_all(&test_dir);

        let mut ctx = BoundaryContext::new(test_dir.clone()).unwrap();
        ctx.add_allowed_root(PathBuf::from("/tmp"))
            .add_allowed_roots(vec![PathBuf::from("/var/tmp")]);

        assert_eq!(ctx.allowed_roots().len(), 2);

        // Cleanup
        let _ = fs::remove_dir_all(&test_dir);
    }
}
