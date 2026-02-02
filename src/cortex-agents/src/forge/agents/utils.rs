//! Shared utility functions for Forge validation agents.
//!
//! This module provides common file collection and text processing utilities
//! used by multiple validation agents (security, quality, dynamic).

use std::path::{Path, PathBuf};
use tokio::fs;

use super::{AgentError, ValidationContext};

// ============================================================================
// Constants
// ============================================================================

/// Maximum number of files to collect to prevent unbounded memory allocation.
pub const MAX_FILES_LIMIT: usize = 10_000;

// ============================================================================
// File Collection Functions
// ============================================================================

/// Collect source files from a directory, respecting context configuration.
///
/// This function recursively walks the directory tree, collecting files that:
/// - Match the include patterns in the context configuration
/// - Are not excluded by the exclude patterns
/// - Do not exceed the `MAX_FILES_LIMIT`
///
/// # Arguments
///
/// * `root` - Root directory to start collecting from
/// * `ctx` - Validation context containing include/exclude patterns
///
/// # Returns
///
/// A vector of file paths matching the criteria.
pub async fn collect_source_files(
    root: &Path,
    ctx: &ValidationContext,
) -> Result<Vec<PathBuf>, AgentError> {
    let mut files = Vec::new();
    collect_files_recursive(root, ctx, &mut files).await?;
    Ok(files)
}

/// Recursively collect files from a directory.
///
/// This function handles:
/// - Permission denied errors gracefully
/// - Directory exclusions based on context configuration
/// - File filtering based on include patterns
/// - A maximum file limit to prevent memory exhaustion
#[async_recursion::async_recursion]
pub async fn collect_files_recursive(
    dir: &Path,
    ctx: &ValidationContext,
    files: &mut Vec<PathBuf>,
) -> Result<(), AgentError> {
    // Early return if we've hit the limit
    if files.len() >= MAX_FILES_LIMIT {
        return Ok(());
    }

    let mut entries = match fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(e) => return Err(AgentError::Io(e)),
    };

    while let Some(entry) = entries.next_entry().await? {
        // Check limit before processing each entry
        if files.len() >= MAX_FILES_LIMIT {
            return Ok(());
        }

        let path = entry.path();

        // Check exclusions
        if let Ok(relative) = path.strip_prefix(&ctx.project_path) {
            if ctx.should_exclude(relative) {
                continue;
            }
        }

        if path.is_dir() {
            collect_files_recursive(&path, ctx, files).await?;
        } else if path.is_file() {
            if let Ok(relative) = path.strip_prefix(&ctx.project_path) {
                if ctx.matches_include(relative) {
                    files.push(path);
                }
            }
        }
    }

    Ok(())
}

/// Collect Rust source files (.rs) from a directory.
///
/// Similar to `collect_source_files` but specifically filters for Rust files.
/// This is used by agents that only analyze Rust code.
pub async fn collect_rust_files(
    root: &Path,
    ctx: &ValidationContext,
) -> Result<Vec<PathBuf>, AgentError> {
    let mut files = Vec::new();
    collect_rust_files_recursive(root, ctx, &mut files).await?;
    Ok(files)
}

/// Recursively collect Rust files from a directory.
#[async_recursion::async_recursion]
pub async fn collect_rust_files_recursive(
    dir: &Path,
    ctx: &ValidationContext,
    files: &mut Vec<PathBuf>,
) -> Result<(), AgentError> {
    // Early return if we've hit the limit
    if files.len() >= MAX_FILES_LIMIT {
        return Ok(());
    }

    let mut entries = match fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(e) => return Err(AgentError::Io(e)),
    };

    while let Some(entry) = entries.next_entry().await? {
        // Check limit before processing each entry
        if files.len() >= MAX_FILES_LIMIT {
            return Ok(());
        }

        let path = entry.path();

        // Check exclusions
        if let Ok(relative) = path.strip_prefix(&ctx.project_path) {
            if ctx.should_exclude(relative) {
                continue;
            }
        }

        if path.is_dir() {
            collect_rust_files_recursive(&path, ctx, files).await?;
        } else if path.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }

    Ok(())
}

// ============================================================================
// Text Processing Functions
// ============================================================================

/// Truncate a line for display purposes.
///
/// Trims whitespace and truncates to `max_len` characters, appending "..." if truncated.
///
/// # Arguments
///
/// * `line` - The line to truncate
/// * `max_len` - Maximum length (must be >= 3 to allow for "...")
///
/// # Returns
///
/// The truncated line as a new String.
pub fn truncate_line(line: &str, max_len: usize) -> String {
    let trimmed = line.trim();
    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..max_len - 3])
    }
}

/// Check if a file path represents a test file.
///
/// Uses robust heuristics to detect test files:
/// - Files in a `tests/` or `test/` directory
/// - Files ending with `_test.rs` or `_tests.rs`
/// - Files named `test.rs` or `tests.rs`
/// - Files containing `/tests/` or `/test/` in their path
///
/// # Arguments
///
/// * `file_path` - The path to check
///
/// # Returns
///
/// `true` if the file appears to be a test file.
pub fn is_test_path(file_path: &Path) -> bool {
    let path_str = file_path.to_string_lossy();

    // Check for standard test directories (with boundary detection)
    // The path separators ensure we don't match "contest/", "latest/", etc.
    if path_str.contains("/tests/")
        || path_str.contains("/test/")
        || path_str.starts_with("tests/")
        || path_str.starts_with("test/")
    {
        return true;
    }

    // Check file stem (name without extension)
    if let Some(file_stem) = file_path.file_stem() {
        let stem = file_stem.to_string_lossy();

        // Check for test file naming conventions
        if stem == "test" || stem == "tests" || stem.ends_with("_test") || stem.ends_with("_tests")
        {
            return true;
        }

        // Check for `mod tests` pattern (file named `tests.rs`)
        if stem == "mod" {
            if let Some(parent) = file_path.parent() {
                if let Some(parent_name) = parent.file_name() {
                    let parent_str = parent_name.to_string_lossy();
                    if parent_str == "tests" || parent_str == "test" {
                        return true;
                    }
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_line() {
        assert_eq!(truncate_line("short", 80), "short");
        let long = "a".repeat(100);
        let truncated = truncate_line(&long, 20);
        assert_eq!(truncated.len(), 20);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_truncate_line_with_whitespace() {
        assert_eq!(truncate_line("  padded  ", 80), "padded");
        assert_eq!(truncate_line("\t\ttabbed\t\t", 80), "tabbed");
    }

    #[test]
    fn test_is_test_path() {
        // Should detect test files
        assert!(is_test_path(Path::new("tests/unit.rs")));
        assert!(is_test_path(Path::new("src/tests/mod.rs")));
        assert!(is_test_path(Path::new("module_test.rs")));
        assert!(is_test_path(Path::new("foo_tests.rs")));
        assert!(is_test_path(Path::new("/project/tests/integration.rs")));
        assert!(is_test_path(Path::new("test/helper.rs")));

        // Should NOT detect non-test files that contain 'test' substring
        assert!(!is_test_path(Path::new("contest.rs")));
        assert!(!is_test_path(Path::new("contest_manager.rs")));
        assert!(!is_test_path(Path::new("latest_report.rs")));
        assert!(!is_test_path(Path::new("greatest.rs")));
        assert!(!is_test_path(Path::new("src/attestation.rs")));
        assert!(!is_test_path(Path::new("testable_trait.rs"))); // Not a test file itself

        // Regular source files
        assert!(!is_test_path(Path::new("src/main.rs")));
        assert!(!is_test_path(Path::new("src/lib.rs")));
        assert!(!is_test_path(Path::new("src/module/mod.rs")));
    }

    #[test]
    fn test_max_files_limit_constant() {
        assert_eq!(MAX_FILES_LIMIT, 10_000);
    }
}
