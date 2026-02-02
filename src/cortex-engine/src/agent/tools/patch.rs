use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::error::Result;
use crate::tools::{ToolContext, ToolHandler, ToolResult};

/// PatchTool applies unified diffs to the workspace with robust error handling for failed hunks.
pub struct PatchTool;

#[derive(Debug, Deserialize)]
struct PatchArgs {
    patch: String,
    #[serde(default)]
    dry_run: bool,
}

impl PatchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PatchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for PatchTool {
    fn name(&self) -> &str {
        "ApplyPatch"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: PatchArgs = match serde_json::from_value(arguments) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Invalid arguments: {e}"))),
        };

        if args.patch.trim().is_empty() {
            return Ok(ToolResult::error("Empty patch provided"));
        }

        match self
            .apply_patch(&args.patch, &context.cwd, args.dry_run)
            .await
        {
            Ok(report) => Ok(ToolResult::success(report)),
            Err(e) => Ok(ToolResult::error(format!("Failed to apply patch: {e}"))),
        }
    }
}

impl PatchTool {
    async fn apply_patch(
        &self,
        patch: &str,
        cwd: &Path,
        dry_run: bool,
    ) -> std::result::Result<String, String> {
        let file_changes = parse_unified_diff(patch)?;

        if file_changes.is_empty() {
            return Ok("No changes to apply".to_string());
        }

        let mut report = Vec::new();
        let mut modified_files = Vec::new();
        let mut failed_files = Vec::new();

        for change in file_changes {
            match self.apply_file_change(&change, cwd, dry_run).await {
                Ok(res) => {
                    report.push(res);
                    if let Some(ref path) = change.new_path {
                        modified_files.push(path.display().to_string());
                    } else if let Some(ref path) = change.old_path {
                        modified_files.push(path.display().to_string());
                    }
                }
                Err(e) => {
                    failed_files.push(format!(
                        "{}: {}",
                        change
                            .new_path
                            .as_ref()
                            .or(change.old_path.as_ref())
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                        e
                    ));
                }
            }
        }

        let action = if dry_run { "Would apply" } else { "Applied" };
        let mut summary = format!("{} changes to {} file(s).", action, modified_files.len());

        if !report.is_empty() {
            summary.push_str("\n\nDetails:\n");
            summary.push_str(&report.join("\n"));
        }

        if !failed_files.is_empty() {
            summary.push_str("\n\nFailed to apply to some files:\n");
            summary.push_str(&failed_files.join("\n"));
            return Err(summary);
        }

        Ok(summary)
    }

    async fn apply_file_change(
        &self,
        change: &FileChange,
        cwd: &Path,
        dry_run: bool,
    ) -> std::result::Result<String, String> {
        // Handle file deletion
        if change.is_deleted
            && let Some(ref old_path) = change.old_path
        {
            let full_path = cwd.join(old_path);
            if !dry_run {
                if full_path.exists() {
                    fs::remove_file(&full_path)
                        .await
                        .map_err(|e| format!("Failed to delete {}: {}", old_path.display(), e))?;
                }
            }
            return Ok(format!("  D {}", old_path.display()));
        }

        // Get the target file path
        let target_path = change
            .new_path
            .as_ref()
            .or(change.old_path.as_ref())
            .ok_or_else(|| "No file path specified in diff".to_string())?;

        let full_path = cwd.join(target_path);

        // Handle new file
        if change.is_new_file {
            let content = build_new_content(&change.hunks);
            if !dry_run {
                if let Some(parent) = full_path.parent() {
                    fs::create_dir_all(parent)
                        .await
                        .map_err(|e| format!("Failed to create directory: {e}"))?;
                }
                fs::write(&full_path, content)
                    .await
                    .map_err(|e| format!("Failed to write {}: {}", target_path.display(), e))?;
            }
            return Ok(format!("  A {}", target_path.display()));
        }

        // Read existing file
        if !full_path.exists() {
            return Err(format!("File does not exist: {}", target_path.display()));
        }

        let original_content = fs::read_to_string(&full_path)
            .await
            .map_err(|e| format!("Failed to read {}: {}", target_path.display(), e))?;

        let original_lines: Vec<&str> = original_content.lines().collect();

        // Apply hunks
        let (new_content, applied_count, failed_hunks) =
            apply_hunks_robustly(&original_lines, &change.hunks)?;

        if !failed_hunks.is_empty() {
            let mut msg = format!(
                "Failed to apply {}/{} hunks to {}",
                failed_hunks.len(),
                change.hunks.len(),
                target_path.display()
            );
            for hunk_idx in failed_hunks {
                msg.push_str(&format!(
                    "\n    - Hunk #{} failed (context mismatch)",
                    hunk_idx + 1
                ));
            }
            return Err(msg);
        }

        if !dry_run {
            fs::write(&full_path, new_content)
                .await
                .map_err(|e| format!("Failed to write {}: {}", target_path.display(), e))?;
        }

        Ok(format!(
            "  M {} ({} hunks applied)",
            target_path.display(),
            applied_count
        ))
    }
}

/// A parsed hunk from a unified diff.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<HunkLine>,
}

#[derive(Debug, Clone)]
enum HunkLine {
    Context(String),
    Add(String),
    Remove(String),
}

/// A file change from a unified diff.
#[derive(Debug, Clone)]
struct FileChange {
    old_path: Option<PathBuf>,
    new_path: Option<PathBuf>,
    hunks: Vec<Hunk>,
    is_new_file: bool,
    is_deleted: bool,
}

/// Parse a unified diff into file changes.
fn parse_unified_diff(patch: &str) -> std::result::Result<Vec<FileChange>, String> {
    let mut file_changes = Vec::new();
    let mut current_change: Option<FileChange> = None;
    let mut current_hunk: Option<Hunk> = None;

    for line in patch.lines() {
        if line.starts_with("--- ") {
            if let Some(mut change) = current_change.take() {
                if let Some(hunk) = current_hunk.take() {
                    change.hunks.push(hunk);
                }
                file_changes.push(change);
            }

            let path_str = &line[4..];
            let old_path = parse_diff_path(path_str);

            current_change = Some(FileChange {
                old_path,
                new_path: None,
                hunks: Vec::new(),
                is_new_file: false,
                is_deleted: false,
            });
            continue;
        }

        if line.starts_with("+++ ") {
            if let Some(ref mut change) = current_change {
                let path_str = &line[4..];
                let new_path = parse_diff_path(path_str);

                change.new_path = new_path;

                if change.old_path.as_ref().map(|p| p.to_string_lossy()) == Some("/dev/null".into())
                {
                    change.is_new_file = true;
                    change.old_path = None;
                }
                if change.new_path.as_ref().map(|p| p.to_string_lossy()) == Some("/dev/null".into())
                {
                    change.is_deleted = true;
                    change.new_path = None;
                }
            }
            continue;
        }

        if line.starts_with("@@ ") {
            if let Some(ref mut change) = current_change {
                if let Some(hunk) = current_hunk.take() {
                    change.hunks.push(hunk);
                }
                if let Some(hunk) = parse_hunk_header(line) {
                    current_hunk = Some(hunk);
                }
            }
            continue;
        }

        if let Some(ref mut hunk) = current_hunk {
            if let Some(content) = line.strip_prefix('+') {
                hunk.lines.push(HunkLine::Add(content.to_string()));
            } else if let Some(content) = line.strip_prefix('-') {
                hunk.lines.push(HunkLine::Remove(content.to_string()));
            } else if let Some(content) = line.strip_prefix(' ') {
                hunk.lines.push(HunkLine::Context(content.to_string()));
            } else if line.is_empty() {
                hunk.lines.push(HunkLine::Context(String::new()));
            }
        }
    }

    if let Some(mut change) = current_change {
        if let Some(hunk) = current_hunk {
            change.hunks.push(hunk);
        }
        file_changes.push(change);
    }

    Ok(file_changes)
}

fn parse_diff_path(path_str: &str) -> Option<PathBuf> {
    let path = path_str.trim();
    let path = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path);
    let path = path.split('\t').next().unwrap_or(path).trim();

    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

fn parse_hunk_header(line: &str) -> Option<Hunk> {
    // @@ -1,5 +1,6 @@
    let line = line.trim_start_matches("@@").trim_end_matches("@@").trim();
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let old_range = parse_range(parts[0].trim_start_matches('-'))?;
    let new_range = parse_range(parts[1].trim_start_matches('+'))?;

    Some(Hunk {
        old_start: old_range.0,
        old_count: old_range.1,
        new_start: new_range.0,
        new_count: new_range.1,
        lines: Vec::new(),
    })
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
    let parts: Vec<&str> = s.split(',').collect();
    let start = parts[0].parse().ok()?;
    let count = parts.get(1).and_then(|c| c.parse().ok()).unwrap_or(1);
    Some((start, count))
}

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

fn apply_hunks_robustly(
    original_lines: &[&str],
    hunks: &[Hunk],
) -> std::result::Result<(String, usize, Vec<usize>), String> {
    let mut result_lines: Vec<String> = original_lines.iter().map(|s| s.to_string()).collect();
    let mut applied_count = 0;
    let mut failed_hunks = Vec::new();

    // Apply hunks in reverse order to maintain line stability for previous hunks
    // (though with fuzzy matching we might want to be more careful)
    for (idx, hunk) in hunks.iter().enumerate().rev() {
        let suggested_start = if hunk.old_start > 0 {
            hunk.old_start - 1
        } else {
            0
        };

        match find_hunk_position(&result_lines, hunk, suggested_start) {
            Ok(actual_start) => {
                let lines_to_remove = hunk
                    .lines
                    .iter()
                    .filter(|l| matches!(l, HunkLine::Remove(_) | HunkLine::Context(_)))
                    .count();
                let mut replacement = Vec::new();
                for line in &hunk.lines {
                    match line {
                        HunkLine::Add(s) | HunkLine::Context(s) => replacement.push(s.clone()),
                        HunkLine::Remove(_) => {}
                    }
                }

                let end_idx = std::cmp::min(actual_start + lines_to_remove, result_lines.len());
                result_lines.splice(actual_start..end_idx, replacement);
                applied_count += 1;
            }
            Err(_) => {
                failed_hunks.push(idx);
            }
        }
    }

    let mut content = result_lines.join("\n");
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }

    Ok((content, applied_count, failed_hunks))
}

fn find_hunk_position(
    lines: &[String],
    hunk: &Hunk,
    suggested_start: usize,
) -> std::result::Result<usize, ()> {
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

    // 1. Try exact position
    if matches_at_position(lines, &match_lines, suggested_start) {
        return Ok(suggested_start);
    }

    // 2. Search nearby
    let max_offset = 100;
    for offset in 1..=max_offset {
        if suggested_start >= offset {
            let pos = suggested_start - offset;
            if matches_at_position(lines, &match_lines, pos) {
                return Ok(pos);
            }
        }
        let pos = suggested_start + offset;
        if pos < lines.len() {
            if matches_at_position(lines, &match_lines, pos) {
                return Ok(pos);
            }
        }
    }

    Err(())
}

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
