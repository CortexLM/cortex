//! Patch application logic.

use crate::error::{PatchError, PatchResult};
use crate::fuzzy::{FuzzyConfig, FuzzyMatcher, MatchQuality};
use crate::hunk::{FileChange, Hunk, HunkLine};
use std::fs;
use std::path::{Path, PathBuf};

/// Options for patch application.
#[derive(Debug, Clone, Default)]
pub struct PatchOptions {
    /// If true, don't actually modify files.
    pub dry_run: bool,
    /// If true, create backups before modifying files.
    pub create_backup: bool,
    /// Fuzzy matching configuration.
    pub fuzzy_config: FuzzyConfig,
    /// If true, fail on first error instead of continuing.
    pub fail_fast: bool,
    /// If true, apply hunks even if some context doesn't match (dangerous).
    pub force: bool,
    /// Strip prefix from paths (like `patch -p1`).
    pub strip_prefix: usize,
}

impl PatchOptions {
    /// Create options for dry-run mode.
    pub fn dry_run() -> Self {
        Self {
            dry_run: true,
            ..Default::default()
        }
    }

    /// Set the strip prefix level.
    pub fn with_strip_prefix(mut self, level: usize) -> Self {
        self.strip_prefix = level;
        self
    }
}

/// Report of patch application.
#[derive(Debug, Clone)]
pub struct PatchReport {
    /// Reports for each file.
    pub files: Vec<FileReport>,
    /// Total number of hunks applied.
    pub hunks_applied: usize,
    /// Total number of hunks that failed.
    pub hunks_failed: usize,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

impl PatchReport {
    /// Create an empty report.
    pub fn new(dry_run: bool) -> Self {
        Self {
            files: Vec::new(),
            hunks_applied: 0,
            hunks_failed: 0,
            dry_run,
        }
    }

    /// Check if all patches were applied successfully.
    pub fn all_successful(&self) -> bool {
        self.hunks_failed == 0 && self.files.iter().all(|f| f.success)
    }

    /// Get a summary string.
    pub fn summary(&self) -> String {
        let action = if self.dry_run {
            "Would apply"
        } else {
            "Applied"
        };
        let mut summary = format!(
            "{} {} hunk(s) to {} file(s)",
            action,
            self.hunks_applied,
            self.files.len()
        );

        if self.hunks_failed > 0 {
            summary.push_str(&format!(" ({} hunk(s) failed)", self.hunks_failed));
        }

        summary
    }
}

/// Report for a single file.
#[derive(Debug, Clone)]
pub struct FileReport {
    /// The file path.
    pub path: Option<String>,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Operation type.
    pub operation: FileOperation,
    /// Error message if failed.
    pub error: Option<String>,
    /// Individual hunk reports.
    pub hunks: Vec<HunkReport>,
}

/// Type of file operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperation {
    /// File was created.
    Create,
    /// File was modified.
    Modify,
    /// File was deleted.
    Delete,
    /// File was renamed.
    Rename,
}

/// Report for a single hunk.
#[derive(Debug, Clone)]
pub struct HunkReport {
    /// Hunk index (0-based).
    pub index: usize,
    /// Application status.
    pub status: HunkStatus,
    /// Original line number from hunk header.
    pub original_line: usize,
    /// Actual line number where applied (if applicable).
    pub applied_line: Option<usize>,
    /// Match quality.
    pub match_quality: Option<MatchQuality>,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Status of hunk application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkStatus {
    /// Applied successfully.
    Applied,
    /// Applied at a different offset.
    AppliedWithOffset,
    /// Applied with fuzzy matching.
    AppliedFuzzy,
    /// Already applied (content matches new state).
    AlreadyApplied,
    /// Failed to apply.
    Failed,
    /// Conflict detected.
    Conflict,
}

/// Apply parsed file changes to the filesystem.
pub fn apply_patch(
    file_changes: &[FileChange],
    cwd: &Path,
    options: &PatchOptions,
) -> PatchResult<PatchReport> {
    let mut report = PatchReport::new(options.dry_run);
    let fuzzy_matcher = FuzzyMatcher::new(options.fuzzy_config.clone());
    let mut errors = Vec::new();

    for change in file_changes {
        match apply_file_change(change, cwd, options, &fuzzy_matcher) {
            Ok(file_report) => {
                report.hunks_applied += file_report
                    .hunks
                    .iter()
                    .filter(|h| {
                        matches!(
                            h.status,
                            HunkStatus::Applied
                                | HunkStatus::AppliedWithOffset
                                | HunkStatus::AppliedFuzzy
                                | HunkStatus::AlreadyApplied
                        )
                    })
                    .count();
                report.hunks_failed += file_report
                    .hunks
                    .iter()
                    .filter(|h| matches!(h.status, HunkStatus::Failed | HunkStatus::Conflict))
                    .count();
                report.files.push(file_report);
            }
            Err(e) => {
                let path = change
                    .effective_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                report.files.push(FileReport {
                    path: Some(path.clone()),
                    success: false,
                    operation: determine_operation(change),
                    error: Some(e.to_string()),
                    hunks: Vec::new(),
                });
                report.hunks_failed += change.hunks.len();

                if options.fail_fast {
                    errors.push(e);
                    break;
                } else {
                    errors.push(e);
                }
            }
        }
    }

    if !errors.is_empty() && options.fail_fast {
        return Err(errors.remove(0));
    }

    Ok(report)
}

/// Apply changes to a single file.
fn apply_file_change(
    change: &FileChange,
    cwd: &Path,
    options: &PatchOptions,
    fuzzy_matcher: &FuzzyMatcher,
) -> PatchResult<FileReport> {
    let operation = determine_operation(change);

    // Handle binary files
    if change.is_binary {
        return Ok(FileReport {
            path: change.effective_path().map(|p| p.display().to_string()),
            success: false,
            operation,
            error: Some("Binary files are not supported".to_string()),
            hunks: Vec::new(),
        });
    }

    // Handle file deletion
    if change.is_deleted {
        return apply_file_deletion(change, cwd, options);
    }

    // Handle new file
    if change.is_new_file {
        return apply_new_file(change, cwd, options);
    }

    // Handle file modification
    apply_file_modification(change, cwd, options, fuzzy_matcher)
}

/// Determine the type of operation for a file change.
fn determine_operation(change: &FileChange) -> FileOperation {
    if change.is_new_file {
        FileOperation::Create
    } else if change.is_deleted {
        FileOperation::Delete
    } else if change.is_rename {
        FileOperation::Rename
    } else {
        FileOperation::Modify
    }
}

/// Apply a file deletion.
fn apply_file_deletion(
    change: &FileChange,
    cwd: &Path,
    options: &PatchOptions,
) -> PatchResult<FileReport> {
    let path = change
        .old_path
        .as_ref()
        .ok_or_else(|| PatchError::InvalidPath {
            path: "missing old path for deletion".to_string(),
        })?;

    let full_path = resolve_path(cwd, path, options.strip_prefix);
    let path_str = path.display().to_string();

    if !options.dry_run && full_path.exists() {
        fs::remove_file(&full_path).map_err(|e| PatchError::DeleteError {
            path: full_path.clone(),
            source: e,
        })?;
    }

    Ok(FileReport {
        path: Some(path_str),
        success: true,
        operation: FileOperation::Delete,
        error: None,
        hunks: Vec::new(),
    })
}

/// Apply a new file creation.
fn apply_new_file(
    change: &FileChange,
    cwd: &Path,
    options: &PatchOptions,
) -> PatchResult<FileReport> {
    let path = change
        .new_path
        .as_ref()
        .ok_or_else(|| PatchError::InvalidPath {
            path: "missing new path for creation".to_string(),
        })?;

    let full_path = resolve_path(cwd, path, options.strip_prefix);
    let path_str = path.display().to_string();

    let content = build_new_content(&change.hunks);

    if !options.dry_run {
        // Create parent directories
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).map_err(|e| PatchError::CreateDirError {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        fs::write(&full_path, &content).map_err(|e| PatchError::WriteError {
            path: full_path.clone(),
            source: e,
        })?;
    }

    let hunk_reports: Vec<HunkReport> = change
        .hunks
        .iter()
        .enumerate()
        .map(|(i, h)| HunkReport {
            index: i,
            status: HunkStatus::Applied,
            original_line: h.new_start,
            applied_line: Some(h.new_start),
            match_quality: Some(MatchQuality::Exact),
            error: None,
        })
        .collect();

    Ok(FileReport {
        path: Some(path_str),
        success: true,
        operation: FileOperation::Create,
        error: None,
        hunks: hunk_reports,
    })
}

/// Apply modifications to an existing file.
fn apply_file_modification(
    change: &FileChange,
    cwd: &Path,
    options: &PatchOptions,
    fuzzy_matcher: &FuzzyMatcher,
) -> PatchResult<FileReport> {
    let path = change
        .effective_path()
        .ok_or_else(|| PatchError::InvalidPath {
            path: "missing path for modification".to_string(),
        })?;

    let full_path = resolve_path(cwd, path, options.strip_prefix);
    let path_str = path.display().to_string();

    // Read the existing file
    if !full_path.exists() {
        return Err(PatchError::FileNotFound { path: full_path });
    }

    let original_content = fs::read_to_string(&full_path).map_err(|e| PatchError::ReadError {
        path: full_path.clone(),
        source: e,
    })?;

    let original_lines: Vec<String> = original_content.lines().map(String::from).collect();

    // Check for overlapping hunks
    if change.has_overlapping_hunks() {
        return Err(PatchError::OverlappingHunks { file: path_str });
    }

    // Apply hunks
    let (new_content, hunk_reports) =
        apply_hunks_to_lines(&original_lines, &change.hunks, fuzzy_matcher, options)?;

    // Check if any hunks failed
    let any_failed = hunk_reports
        .iter()
        .any(|r| matches!(r.status, HunkStatus::Failed | HunkStatus::Conflict));

    if any_failed && !options.force {
        return Ok(FileReport {
            path: Some(path_str),
            success: false,
            operation: FileOperation::Modify,
            error: Some("Some hunks failed to apply".to_string()),
            hunks: hunk_reports,
        });
    }

    // Write the modified content
    if !options.dry_run {
        fs::write(&full_path, &new_content).map_err(|e| PatchError::WriteError {
            path: full_path,
            source: e,
        })?;
    }

    Ok(FileReport {
        path: Some(path_str),
        success: true,
        operation: FileOperation::Modify,
        error: None,
        hunks: hunk_reports,
    })
}

/// Apply hunks to lines and return the new content.
fn apply_hunks_to_lines(
    original_lines: &[String],
    hunks: &[Hunk],
    fuzzy_matcher: &FuzzyMatcher,
    options: &PatchOptions,
) -> PatchResult<(String, Vec<HunkReport>)> {
    let mut result_lines = original_lines.to_vec();
    let mut hunk_reports = Vec::new();
    let mut line_offset: isize = 0;

    // Sort hunks by original position (ascending) for forward application
    let mut sorted_indices: Vec<usize> = (0..hunks.len()).collect();
    sorted_indices.sort_by_key(|&i| hunks[i].old_start);

    for &hunk_idx in &sorted_indices {
        let hunk = &hunks[hunk_idx];

        // Calculate suggested start position with accumulated offset
        let suggested_start = if hunk.old_start > 0 {
            ((hunk.old_start as isize - 1) + line_offset).max(0) as usize
        } else {
            0
        };

        let match_lines = hunk.match_lines();

        // Try to find the position for this hunk
        let position_result =
            fuzzy_matcher.find_position(&result_lines, &match_lines, suggested_start);

        let report = match position_result {
            Some((actual_start, quality)) => {
                // Check if the hunk is already applied
                let result_lines_after = hunk.result_lines();
                if is_already_applied(&result_lines, &result_lines_after, actual_start) {
                    HunkReport {
                        index: hunk_idx,
                        status: HunkStatus::AlreadyApplied,
                        original_line: hunk.old_start,
                        applied_line: Some(actual_start + 1),
                        match_quality: Some(quality),
                        error: None,
                    }
                } else {
                    // Apply the hunk
                    let lines_to_remove = match_lines.len();
                    let replacement: Vec<String> =
                        hunk.result_lines().into_iter().map(String::from).collect();
                    let lines_added = replacement.len();

                    let end_idx = (actual_start + lines_to_remove).min(result_lines.len());
                    result_lines.splice(actual_start..end_idx, replacement);

                    // Update line offset for subsequent hunks
                    line_offset += lines_added as isize - lines_to_remove as isize;

                    let status = match &quality {
                        MatchQuality::Exact => HunkStatus::Applied,
                        MatchQuality::Offset(_) => HunkStatus::AppliedWithOffset,
                        MatchQuality::Fuzzy(_) => HunkStatus::AppliedFuzzy,
                    };

                    HunkReport {
                        index: hunk_idx,
                        status,
                        original_line: hunk.old_start,
                        applied_line: Some(actual_start + 1),
                        match_quality: Some(quality),
                        error: None,
                    }
                }
            }
            None => {
                // Failed to find a matching position
                if options.force {
                    // In force mode, try to apply at the original position anyway
                    let target_start = suggested_start.min(result_lines.len());
                    let replacement: Vec<String> =
                        hunk.result_lines().into_iter().map(String::from).collect();

                    // Just insert without removing (risky!)
                    for (i, line) in replacement.into_iter().enumerate() {
                        result_lines.insert(target_start + i, line);
                    }

                    line_offset += hunk.lines_added() as isize;

                    HunkReport {
                        index: hunk_idx,
                        status: HunkStatus::AppliedFuzzy,
                        original_line: hunk.old_start,
                        applied_line: Some(target_start + 1),
                        match_quality: None,
                        error: Some("Forced application without context match".to_string()),
                    }
                } else {
                    HunkReport {
                        index: hunk_idx,
                        status: HunkStatus::Failed,
                        original_line: hunk.old_start,
                        applied_line: None,
                        match_quality: None,
                        error: Some(format!(
                            "Could not find matching context for hunk at line {}",
                            hunk.old_start
                        )),
                    }
                }
            }
        };

        hunk_reports.push(report);
    }

    // Re-sort reports by original hunk index
    hunk_reports.sort_by_key(|r| r.index);

    // Build final content
    let mut content = result_lines.join("\n");
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }

    Ok((content, hunk_reports))
}

/// Check if a hunk has already been applied.
fn is_already_applied(file_lines: &[String], result_lines: &[&str], start: usize) -> bool {
    if start + result_lines.len() > file_lines.len() {
        return false;
    }

    for (i, expected) in result_lines.iter().enumerate() {
        if file_lines[start + i].trim() != expected.trim() {
            return false;
        }
    }

    true
}

/// Build content for a new file from hunks.
fn build_new_content(hunks: &[Hunk]) -> String {
    let mut content = String::new();

    for hunk in hunks {
        for line in &hunk.lines {
            match line {
                HunkLine::Add(s) | HunkLine::Context(s) => {
                    content.push_str(s);
                    content.push('\n');
                }
                HunkLine::Remove(_) => {}
            }
        }
    }

    content
}

/// Resolve a path with optional prefix stripping.
fn resolve_path(cwd: &Path, path: &Path, strip_prefix: usize) -> PathBuf {
    let path_str = path.to_string_lossy();
    let stripped: String = path_str
        .split('/')
        .skip(strip_prefix)
        .collect::<Vec<_>>()
        .join("/");

    if stripped.is_empty() {
        cwd.join(path)
    } else {
        cwd.join(&stripped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_unified_diff;
    use tempfile::TempDir;

    #[test]
    fn test_apply_simple_patch() {
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
        let changes = parse_unified_diff(patch).unwrap();
        let report = apply_patch(&changes, temp.path(), &PatchOptions::default()).unwrap();

        assert!(report.all_successful());
        assert_eq!(report.hunks_applied, 1);

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("new line"));
    }

    #[test]
    fn test_apply_patch_dry_run() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        let original = "line 1\nline 2\nline 3\n";
        fs::write(&file_path, original).unwrap();

        let patch = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,4 @@
 line 1
+new line
 line 2
 line 3
"#;
        let changes = parse_unified_diff(patch).unwrap();
        let options = PatchOptions::dry_run();
        let report = apply_patch(&changes, temp.path(), &options).unwrap();

        assert!(report.all_successful());
        assert!(report.dry_run);

        // File should be unchanged
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, original);
    }

    #[test]
    fn test_apply_new_file() {
        let temp = TempDir::new().unwrap();

        let patch = r#"--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1,2 @@
+line 1
+line 2
"#;
        let changes = parse_unified_diff(patch).unwrap();
        let report = apply_patch(&changes, temp.path(), &PatchOptions::default()).unwrap();

        assert!(report.all_successful());

        let file_path = temp.path().join("new_file.txt");
        assert!(file_path.exists());

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("line 1"));
        assert!(content.contains("line 2"));
    }

    #[test]
    fn test_apply_delete_file() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("delete_me.txt");
        fs::write(&file_path, "content").unwrap();
        assert!(file_path.exists());

        let patch = r#"--- a/delete_me.txt
+++ /dev/null
@@ -1 +0,0 @@
-content
"#;
        let changes = parse_unified_diff(patch).unwrap();
        let report = apply_patch(&changes, temp.path(), &PatchOptions::default()).unwrap();

        assert!(report.all_successful());
        assert!(!file_path.exists());
    }

    #[test]
    fn test_fuzzy_match_offset() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        // Add extra lines at the beginning
        fs::write(&file_path, "extra 1\nextra 2\nline 1\nline 2\nline 3\n").unwrap();

        let patch = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,4 @@
 line 1
+new line
 line 2
 line 3
"#;
        let changes = parse_unified_diff(patch).unwrap();
        let report = apply_patch(&changes, temp.path(), &PatchOptions::default()).unwrap();

        assert!(report.all_successful());
        assert!(matches!(
            report.files[0].hunks[0].status,
            HunkStatus::AppliedWithOffset
        ));

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("new line"));
    }

    #[test]
    fn test_already_applied() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        // Content already has the change - a simple replacement scenario
        // The patch replaces "old" with "new" on line 2
        fs::write(&file_path, "line 1\nnew\nline 3\n").unwrap();

        // Patch that would change "old" to "new" - but file already has "new"
        let patch = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,3 @@
 line 1
-old
+new
 line 3
"#;
        let changes = parse_unified_diff(patch).unwrap();
        let report = apply_patch(&changes, temp.path(), &PatchOptions::default()).unwrap();

        // The hunk will fail because context doesn't match ("old" is not present)
        // This is expected behavior - detecting already-applied patches requires
        // checking if the result matches, not the context
        assert!(!report.all_successful());
        assert!(matches!(
            report.files[0].hunks[0].status,
            HunkStatus::Failed
        ));
    }

    #[test]
    fn test_failed_hunk() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        // Completely different content
        fs::write(&file_path, "completely different content\n").unwrap();

        let patch = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,4 @@
 line 1
+new line
 line 2
 line 3
"#;
        let changes = parse_unified_diff(patch).unwrap();
        let report = apply_patch(&changes, temp.path(), &PatchOptions::default()).unwrap();

        assert!(!report.all_successful());
        assert_eq!(report.hunks_failed, 1);
        assert!(matches!(
            report.files[0].hunks[0].status,
            HunkStatus::Failed
        ));
    }

    #[test]
    fn test_strip_prefix() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\n").unwrap();

        // Patch has paths with prefix that needs stripping
        let patch = r#"--- a/prefix/test.txt
+++ b/prefix/test.txt
@@ -1,2 +1,3 @@
 line 1
+new line
 line 2
"#;
        let changes = parse_unified_diff(patch).unwrap();
        let options = PatchOptions::default().with_strip_prefix(1);
        let report = apply_patch(&changes, temp.path(), &options).unwrap();

        assert!(report.all_successful());
    }

    #[test]
    fn test_report_summary() {
        let mut report = PatchReport::new(false);
        report.hunks_applied = 5;
        report.files.push(FileReport {
            path: Some("test.txt".to_string()),
            success: true,
            operation: FileOperation::Modify,
            error: None,
            hunks: Vec::new(),
        });

        let summary = report.summary();
        assert!(summary.contains("5 hunk(s)"));
        assert!(summary.contains("1 file(s)"));
    }
}
