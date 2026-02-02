//! Cortex Apply Patch - Production-grade file patching utilities.
//!
//! This crate provides comprehensive patch parsing and application with:
//! - Unified diff parsing (standard and git formats)
//! - Fuzzy matching for moved lines
//! - Context-aware application
//! - Conflict detection and reporting
//! - Dry-run mode
//! - Undo capability (backup/restore)
//!
//! # Example
//!
//! ```no_run
//! use cortex_apply_patch::parse_and_apply;
//! use std::path::Path;
//!
//! let patch = r#"--- a/file.txt
//! +++ b/file.txt
//! @@ -1,3 +1,4 @@
//!  line 1
//! +new line
//!  line 2
//!  line 3
//! "#;
//!
//! let result = parse_and_apply(patch, Path::new("."));
//! ```

mod applier;
mod backup;
mod error;
mod fuzzy;
mod hunk;
mod parser;

pub use applier::{FileReport, HunkReport, HunkStatus, PatchOptions, PatchReport, apply_patch};
pub use backup::{BackupManager, BackupSet};
pub use error::{PatchError, PatchResult};
pub use fuzzy::FuzzyMatcher;
pub use hunk::{FileChange, Hunk, HunkLine};
pub use parser::{PatchFormat, parse_patch};

use std::path::Path;

/// Parse and apply a patch to the filesystem.
///
/// This is the main entry point for patch application. It parses the patch content,
/// determines the format, and applies changes to files relative to the given working directory.
///
/// # Arguments
///
/// * `patch` - The patch content (unified diff, git diff, or search/replace format)
/// * `cwd` - The working directory (patch file paths are relative to this)
///
/// # Returns
///
/// A vector of modified file paths on success, or an error on failure.
pub fn parse_and_apply(patch: &str, cwd: &Path) -> anyhow::Result<Vec<String>> {
    parse_and_apply_with_options(patch, cwd, PatchOptions::default())
}

/// Parse and apply a patch with custom options.
///
/// # Arguments
///
/// * `patch` - The patch content
/// * `cwd` - The working directory
/// * `options` - Configuration options for patch application
///
/// # Returns
///
/// A vector of modified file paths on success, or an error on failure.
pub fn parse_and_apply_with_options(
    patch: &str,
    cwd: &Path,
    options: PatchOptions,
) -> anyhow::Result<Vec<String>> {
    let file_changes = parse_patch(patch)?;

    if file_changes.is_empty() {
        return Ok(vec![]);
    }

    let report = apply_patch(&file_changes, cwd, &options)?;

    // Collect successfully modified files
    let modified: Vec<String> = report
        .files
        .iter()
        .filter(|f| f.success)
        .filter_map(|f| f.path.clone())
        .collect();

    // If any files failed and we're not in dry-run mode, return an error
    let failed: Vec<&FileReport> = report.files.iter().filter(|f| !f.success).collect();
    if !failed.is_empty() && !options.dry_run {
        let errors: Vec<String> = failed
            .iter()
            .map(|f| {
                format!(
                    "{}: {}",
                    f.path.as_deref().unwrap_or("unknown"),
                    f.error.as_deref().unwrap_or("unknown error")
                )
            })
            .collect();
        anyhow::bail!(
            "Failed to apply patch to some files:\n{}",
            errors.join("\n")
        );
    }

    Ok(modified)
}

/// Get a detailed report of patch application without modifying files.
///
/// This is useful for previewing what changes would be made.
pub fn dry_run(patch: &str, cwd: &Path) -> anyhow::Result<PatchReport> {
    let file_changes = parse_patch(patch)?;
    let options = PatchOptions {
        dry_run: true,
        ..Default::default()
    };
    Ok(apply_patch(&file_changes, cwd, &options)?)
}

/// Apply a patch with backup support for undo capability.
///
/// Returns the backup set that can be used to restore files to their original state.
pub fn apply_with_backup(
    patch: &str,
    cwd: &Path,
    backup_dir: &Path,
) -> anyhow::Result<(Vec<String>, BackupSet)> {
    let file_changes = parse_patch(patch)?;

    if file_changes.is_empty() {
        return Ok((vec![], BackupSet::empty()));
    }

    // Create backups first
    let mut backup_manager = BackupManager::new(backup_dir.to_path_buf());
    let backup_set = backup_manager.create_backup(&file_changes, cwd)?;

    // Apply the patch
    let options = PatchOptions {
        dry_run: false,
        create_backup: false, // We already created the backup
        ..Default::default()
    };

    match apply_patch(&file_changes, cwd, &options) {
        Ok(report) => {
            let modified: Vec<String> = report
                .files
                .iter()
                .filter(|f| f.success)
                .filter_map(|f| f.path.clone())
                .collect();

            // Check for failures
            let failed: Vec<&FileReport> = report.files.iter().filter(|f| !f.success).collect();
            if !failed.is_empty() {
                // Restore from backup on partial failure
                if let Err(restore_err) = backup_manager.restore(&backup_set, cwd) {
                    anyhow::bail!(
                        "Patch failed and restore also failed. Manual intervention required.\nPatch errors: {:?}\nRestore error: {}",
                        failed
                            .iter()
                            .map(|f| f.error.as_deref().unwrap_or("unknown"))
                            .collect::<Vec<_>>(),
                        restore_err
                    );
                }
                let errors: Vec<String> = failed
                    .iter()
                    .map(|f| {
                        f.error
                            .clone()
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect();
                anyhow::bail!(
                    "Failed to apply patch (restored from backup):\n{}",
                    errors.join("\n")
                );
            }

            Ok((modified, backup_set))
        }
        Err(e) => {
            // Restore from backup on error
            let _ = backup_manager.restore(&backup_set, cwd);
            Err(e.into())
        }
    }
}

/// Undo a previously applied patch using a backup set.
pub fn undo_patch(backup_set: &BackupSet, backup_dir: &Path, cwd: &Path) -> anyhow::Result<()> {
    let backup_manager = BackupManager::new(backup_dir.to_path_buf());
    Ok(backup_manager.restore(backup_set, cwd)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_and_apply_simple() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let patch = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,4 @@
 line 1
+new line
 line 2
 line 3
"#;

        let result = parse_and_apply(patch, temp.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("test.txt"));

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("new line"));
    }

    #[test]
    fn test_dry_run_no_changes() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let patch = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,4 @@
 line 1
+new line
 line 2
 line 3
"#;

        let report = dry_run(patch, temp.path()).unwrap();
        assert!(report.files[0].success);

        // File should be unchanged
        let content = fs::read_to_string(&file_path).unwrap();
        assert!(!content.contains("new line"));
    }

    #[test]
    fn test_empty_patch() {
        let temp = TempDir::new().unwrap();
        let result = parse_and_apply("", temp.path()).unwrap();
        assert!(result.is_empty());
    }
}
