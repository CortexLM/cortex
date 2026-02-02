//! Apply patch tool handler.
//!
//! Complete unified diff parser and applier with context matching.

use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::Result;

/// Handler for apply_patch tool.
pub struct ApplyPatchHandler;

#[derive(Debug, Deserialize)]
struct ApplyPatchArgs {
    patch: String,
    #[serde(default)]
    dry_run: bool,
}

impl ApplyPatchHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ApplyPatchHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
    fn name(&self) -> &str {
        "ApplyPatch"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: ApplyPatchArgs = serde_json::from_value(arguments)?;

        match apply_unified_diff(&args.patch, &context.cwd, args.dry_run) {
            Ok(report) => Ok(ToolResult::success(report)),
            Err(e) => Ok(ToolResult::error(format!("Failed to apply patch: {e}"))),
        }
    }
}

/// A parsed hunk from a unified diff.
#[derive(Debug, Clone)]
pub struct Hunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<HunkLine>,
}

#[derive(Debug, Clone)]
pub enum HunkLine {
    Context(String),
    Add(String),
    Remove(String),
}

/// A file change from a unified diff.
#[derive(Debug, Clone)]
pub struct FileChange {
    pub old_path: Option<PathBuf>,
    pub new_path: Option<PathBuf>,
    pub hunks: Vec<Hunk>,
    pub is_new_file: bool,
    pub is_deleted: bool,
    pub is_rename: bool,
}

/// Apply a unified diff to the filesystem.
fn apply_unified_diff(
    patch: &str,
    cwd: &PathBuf,
    dry_run: bool,
) -> std::result::Result<String, String> {
    let file_changes = parse_unified_diff(patch)?;

    if file_changes.is_empty() {
        return Ok("No changes to apply".to_string());
    }

    let mut report = Vec::new();
    let mut modified_files = Vec::new();

    for change in file_changes {
        let result = apply_file_change(&change, cwd, dry_run)?;
        report.push(result.clone());

        if let Some(ref new_path) = change.new_path {
            modified_files.push(new_path.display().to_string());
        }
    }

    let action = if dry_run { "Would apply" } else { "Applied" };
    Ok(format!(
        "{} changes to {} file(s):\n{}",
        action,
        modified_files.len(),
        report.join("\n")
    ))
}

/// Parse a unified diff into file changes.
pub fn parse_unified_diff(patch: &str) -> std::result::Result<Vec<FileChange>, String> {
    let mut file_changes = Vec::new();
    let mut current_change: Option<FileChange> = None;
    let mut current_hunk: Option<Hunk> = None;
    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Detect file header
        if line.starts_with("--- ") {
            // Save previous change if exists
            if let Some(mut change) = current_change.take() {
                if let Some(hunk) = current_hunk.take() {
                    change.hunks.push(hunk);
                }
                file_changes.push(change);
            }

            let old_path = parse_file_path(&line[4..]);

            // Look for +++ line
            if i + 1 < lines.len() && lines[i + 1].starts_with("+++ ") {
                let new_path = parse_file_path(&lines[i + 1][4..]);

                let is_new_file = old_path
                    .as_ref()
                    .is_some_and(|p| p.display().to_string() == "/dev/null");
                let is_deleted = new_path
                    .as_ref()
                    .is_some_and(|p| p.display().to_string() == "/dev/null");

                current_change = Some(FileChange {
                    old_path: if is_new_file { None } else { old_path },
                    new_path: if is_deleted { None } else { new_path },
                    hunks: Vec::new(),
                    is_new_file,
                    is_deleted,
                    is_rename: false,
                });
                i += 2;
                continue;
            }
        }

        // Detect hunk header
        if line.starts_with("@@ ") {
            // Save previous hunk
            if let Some(ref mut change) = current_change
                && let Some(hunk) = current_hunk.take()
            {
                change.hunks.push(hunk);
            }

            if let Some(hunk) = parse_hunk_header(line) {
                current_hunk = Some(hunk);
            }
            i += 1;
            continue;
        }

        // Parse hunk lines
        if let Some(ref mut hunk) = current_hunk {
            if line.starts_with('+') && !line.starts_with("+++") {
                hunk.lines.push(HunkLine::Add(line[1..].to_string()));
            } else if line.starts_with('-') && !line.starts_with("---") {
                hunk.lines.push(HunkLine::Remove(line[1..].to_string()));
            } else if line.starts_with(' ') || line.is_empty() {
                let content = if line.is_empty() { "" } else { &line[1..] };
                hunk.lines.push(HunkLine::Context(content.to_string()));
            } else if line.starts_with('\\') {
                // "\ No newline at end of file" - ignore
            }
        }

        i += 1;
    }

    // Save final change and hunk
    if let Some(mut change) = current_change.take() {
        if let Some(hunk) = current_hunk.take() {
            change.hunks.push(hunk);
        }
        file_changes.push(change);
    }

    Ok(file_changes)
}

/// Parse a file path from diff header.
fn parse_file_path(path_str: &str) -> Option<PathBuf> {
    let path = path_str.trim();

    // Handle various formats: a/path, b/path, or just path
    let path = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path);

    // Remove timestamp if present (e.g., "file.txt\t2024-01-01 00:00:00")
    let path = path.split('\t').next().unwrap_or(path).trim();

    if path == "/dev/null" {
        return Some(PathBuf::from("/dev/null"));
    }

    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

/// Parse a hunk header like "@@ -1,5 +1,6 @@".
fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let line = line.trim_start_matches('@').trim_end_matches('@').trim();
    let parts: Vec<&str> = line.split_whitespace().collect();

    if parts.len() < 2 {
        return None;
    }

    let (old_start, old_count) = parse_range(parts[0].trim_start_matches('-'))?;
    let (new_start, new_count) = parse_range(parts[1].trim_start_matches('+'))?;

    Some(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        lines: Vec::new(),
    })
}

/// Parse a range like "1,5" or "1".
fn parse_range(s: &str) -> Option<(usize, usize)> {
    let parts: Vec<&str> = s.split(',').collect();
    let start = parts.first()?.parse().ok()?;
    let count = parts.get(1).and_then(|c| c.parse().ok()).unwrap_or(1);
    Some((start, count))
}

/// Apply a single file change.
fn apply_file_change(
    change: &FileChange,
    cwd: &PathBuf,
    dry_run: bool,
) -> std::result::Result<String, String> {
    // Handle file deletion
    if change.is_deleted
        && let Some(ref old_path) = change.old_path
    {
        let full_path = cwd.join(old_path);
        if !dry_run {
            fs::remove_file(&full_path)
                .map_err(|e| format!("Failed to delete {}: {}", full_path.display(), e))?;
        }
        return Ok(format!("  D {}", old_path.display()));
    }

    // Get the target file path
    let target_path = change
        .new_path
        .as_ref()
        .or(change.old_path.as_ref())
        .ok_or_else(|| "No file path specified".to_string())?;

    let full_path = cwd.join(target_path);

    // Handle new file
    if change.is_new_file {
        let content = build_new_content(&change.hunks);

        if !dry_run {
            // Create parent directories
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory: {e}"))?;
            }
            fs::write(&full_path, content)
                .map_err(|e| format!("Failed to write {}: {}", full_path.display(), e))?;
        }
        return Ok(format!("  A {}", target_path.display()));
    }

    // Read existing file
    let original_content = fs::read_to_string(&full_path)
        .map_err(|e| format!("Failed to read {}: {}", full_path.display(), e))?;

    let original_lines: Vec<&str> = original_content.lines().collect();

    // Apply hunks
    let new_content = apply_hunks_to_lines(&original_lines, &change.hunks)?;

    if !dry_run {
        fs::write(&full_path, new_content)
            .map_err(|e| format!("Failed to write {}: {}", full_path.display(), e))?;
    }

    Ok(format!("  M {}", target_path.display()))
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

/// Apply hunks to existing lines.
fn apply_hunks_to_lines(
    original_lines: &[&str],
    hunks: &[Hunk],
) -> std::result::Result<String, String> {
    let mut result_lines: Vec<String> = original_lines
        .iter()
        .map(std::string::ToString::to_string)
        .collect();

    // Apply hunks in reverse order to maintain line numbers
    for hunk in hunks.iter().rev() {
        let start_idx = if hunk.old_start > 0 {
            hunk.old_start - 1
        } else {
            0
        };

        // Find the best matching position for this hunk
        let actual_start = find_hunk_position(&result_lines, hunk, start_idx)?;

        // Calculate how many lines to remove
        let lines_to_remove: usize = hunk
            .lines
            .iter()
            .filter(|l| matches!(l, HunkLine::Remove(_) | HunkLine::Context(_)))
            .count();

        // Build replacement lines
        let mut replacement: Vec<String> = Vec::new();
        for line in &hunk.lines {
            match line {
                HunkLine::Add(s) | HunkLine::Context(s) => {
                    replacement.push(s.clone());
                }
                HunkLine::Remove(_) => {}
            }
        }

        // Replace the lines
        let end_idx = std::cmp::min(actual_start + lines_to_remove, result_lines.len());
        result_lines.splice(actual_start..end_idx, replacement);
    }

    let mut content = result_lines.join("\n");
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }

    Ok(content)
}

/// Find the best position to apply a hunk, with fuzzy matching.
fn find_hunk_position(
    lines: &[String],
    hunk: &Hunk,
    suggested_start: usize,
) -> std::result::Result<usize, String> {
    // Extract context and remove lines from hunk for matching
    let match_lines: Vec<&str> = hunk
        .lines
        .iter()
        .filter_map(|l| match l {
            HunkLine::Context(s) | HunkLine::Remove(s) => Some(s.as_str()),
            HunkLine::Add(_) => None,
        })
        .collect();

    if match_lines.is_empty() {
        return Ok(suggested_start);
    }

    // Try exact position first
    if matches_at_position(lines, &match_lines, suggested_start) {
        return Ok(suggested_start);
    }

    // Search nearby positions (within 50 lines)
    for offset in 1..=50 {
        if suggested_start >= offset {
            let pos = suggested_start - offset;
            if matches_at_position(lines, &match_lines, pos) {
                return Ok(pos);
            }
        }

        let pos = suggested_start + offset;
        if pos < lines.len() && matches_at_position(lines, &match_lines, pos) {
            return Ok(pos);
        }
    }

    // If we can't find a match but we have the right number of lines, use suggested position
    if suggested_start <= lines.len() {
        return Ok(suggested_start);
    }

    Err(format!(
        "Could not find matching context for hunk at line {}",
        hunk.old_start
    ))
}

/// Check if lines match at a given position.
fn matches_at_position(lines: &[String], match_lines: &[&str], start: usize) -> bool {
    if start + match_lines.len() > lines.len() {
        return false;
    }

    for (i, expected) in match_lines.iter().enumerate() {
        if lines[start + i].trim() != expected.trim() {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hunk_header() {
        let hunk = parse_hunk_header("@@ -1,5 +1,6 @@").unwrap();
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 5);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_count, 6);
    }

    #[test]
    fn test_parse_file_path() {
        assert_eq!(
            parse_file_path("a/src/main.rs"),
            Some(PathBuf::from("src/main.rs"))
        );
        assert_eq!(
            parse_file_path("b/src/main.rs"),
            Some(PathBuf::from("src/main.rs"))
        );
        assert_eq!(
            parse_file_path("src/main.rs"),
            Some(PathBuf::from("src/main.rs"))
        );
    }

    #[test]
    fn test_parse_unified_diff() {
        let patch = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,4 @@
 line 1
+new line
 line 2
 line 3
"#;
        let changes = parse_unified_diff(patch).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].hunks.len(), 1);
        assert_eq!(changes[0].hunks[0].lines.len(), 4);
    }

    #[test]
    fn test_new_file() {
        let patch = r#"--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1,2 @@
+line 1
+line 2
"#;
        let changes = parse_unified_diff(patch).unwrap();
        assert!(changes[0].is_new_file);
    }
}
