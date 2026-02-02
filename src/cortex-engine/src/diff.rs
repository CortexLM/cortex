//! Diff and patch utilities.
//!
//! Provides functionality for computing, parsing, and applying diffs
//! to files and text content.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CortexError, Result};
use crate::tasks::snapshot::{FileState, Snapshot};

/// A unified diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedDiff {
    /// File changes.
    pub files: Vec<FileDiff>,
    /// Original text (if available).
    pub original_text: Option<String>,
}

impl UnifiedDiff {
    /// Parse a unified diff string.
    pub fn parse(diff_text: &str) -> Result<Self> {
        let parser = DiffParser::new();
        parser.parse(diff_text)
    }

    /// Create an empty diff.
    pub fn empty() -> Self {
        Self {
            files: Vec::new(),
            original_text: None,
        }
    }

    /// Check if diff is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get number of files changed.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Get total lines added.
    pub fn lines_added(&self) -> usize {
        self.files.iter().map(FileDiff::lines_added).sum()
    }

    /// Get total lines removed.
    pub fn lines_removed(&self) -> usize {
        self.files.iter().map(FileDiff::lines_removed).sum()
    }

    /// Reverse the diff.
    pub fn reverse(&self) -> Self {
        Self {
            files: self.files.iter().map(FileDiff::reverse).collect(),
            original_text: None,
        }
    }

    /// Apply the diff to a directory.
    pub async fn apply(&self, root: &Path) -> Result<ApplyResult> {
        let mut result = ApplyResult::default();

        for file_diff in &self.files {
            match file_diff.apply(root).await {
                Ok(stats) => {
                    result.files_modified += 1;
                    result.lines_added += stats.lines_added;
                    result.lines_removed += stats.lines_removed;
                }
                Err(e) => {
                    result.errors.push(FileError {
                        path: file_diff.path.clone(),
                        error: e.to_string(),
                    });
                }
            }
        }

        Ok(result)
    }
}

impl fmt::Display for UnifiedDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let result: Vec<String> = self.files.iter().map(|f| f.to_string()).collect();
        write!(f, "{}", result.join("\n"))
    }
}

/// A single file's diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    /// File path (new path for renames).
    pub path: PathBuf,
    /// Original path (for renames).
    pub original_path: Option<PathBuf>,
    /// File operation type.
    pub operation: FileOperation,
    /// Hunks (change blocks).
    pub hunks: Vec<Hunk>,
    /// File mode (for new files).
    pub mode: Option<u32>,
    /// Is binary file.
    pub is_binary: bool,
}

impl FileDiff {
    /// Create a new file addition diff.
    pub fn new_file(path: impl Into<PathBuf>, content: &str) -> Self {
        let lines: Vec<_> = content.lines().collect();
        let hunk = Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: lines.len() as u32,
            lines: lines.iter().map(|l| DiffLine::Add(l.to_string())).collect(),
            context_before: String::new(),
            context_after: String::new(),
        };

        Self {
            path: path.into(),
            original_path: None,
            operation: FileOperation::Create,
            hunks: vec![hunk],
            mode: Some(0o644),
            is_binary: false,
        }
    }

    /// Create a file deletion diff.
    pub fn delete_file(path: impl Into<PathBuf>, content: &str) -> Self {
        let lines: Vec<_> = content.lines().collect();
        let hunk = Hunk {
            old_start: 1,
            old_count: lines.len() as u32,
            new_start: 0,
            new_count: 0,
            lines: lines
                .iter()
                .map(|l| DiffLine::Remove(l.to_string()))
                .collect(),
            context_before: String::new(),
            context_after: String::new(),
        };

        Self {
            path: path.into(),
            original_path: None,
            operation: FileOperation::Delete,
            hunks: vec![hunk],
            mode: None,
            is_binary: false,
        }
    }

    /// Get number of lines added.
    pub fn lines_added(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| matches!(l, DiffLine::Add(_)))
            .count()
    }

    /// Get number of lines removed.
    pub fn lines_removed(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| matches!(l, DiffLine::Remove(_)))
            .count()
    }

    /// Reverse this file diff.
    pub fn reverse(&self) -> Self {
        Self {
            path: self
                .original_path
                .clone()
                .unwrap_or_else(|| self.path.clone()),
            original_path: Some(self.path.clone()),
            operation: match self.operation {
                FileOperation::Create => FileOperation::Delete,
                FileOperation::Delete => FileOperation::Create,
                FileOperation::Modify => FileOperation::Modify,
                FileOperation::Rename => FileOperation::Rename,
            },
            hunks: self.hunks.iter().map(Hunk::reverse).collect(),
            mode: self.mode,
            is_binary: self.is_binary,
        }
    }

    /// Apply this file diff.
    pub async fn apply(&self, root: &Path) -> Result<ApplyStats> {
        let file_path = root.join(&self.path);

        match self.operation {
            FileOperation::Create => {
                // Ensure parent exists
                if let Some(parent) = file_path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(CortexError::Io)?;
                }

                let content = self.reconstruct_new_content();
                tokio::fs::write(&file_path, &content)
                    .await
                    .map_err(CortexError::Io)?;

                Ok(ApplyStats {
                    lines_added: self.lines_added(),
                    lines_removed: 0,
                })
            }
            FileOperation::Delete => {
                if file_path.exists() {
                    tokio::fs::remove_file(&file_path)
                        .await
                        .map_err(CortexError::Io)?;
                }
                Ok(ApplyStats {
                    lines_added: 0,
                    lines_removed: self.lines_removed(),
                })
            }
            FileOperation::Modify => {
                let content = tokio::fs::read_to_string(&file_path)
                    .await
                    .map_err(CortexError::Io)?;

                let new_content = self.apply_to_content(&content)?;
                tokio::fs::write(&file_path, &new_content)
                    .await
                    .map_err(CortexError::Io)?;

                Ok(ApplyStats {
                    lines_added: self.lines_added(),
                    lines_removed: self.lines_removed(),
                })
            }
            FileOperation::Rename => {
                if let Some(ref orig) = self.original_path {
                    let orig_path = root.join(orig);
                    if orig_path.exists() {
                        tokio::fs::rename(&orig_path, &file_path)
                            .await
                            .map_err(CortexError::Io)?;
                    }
                }
                Ok(ApplyStats::default())
            }
        }
    }

    /// Reconstruct new file content from additions.
    fn reconstruct_new_content(&self) -> String {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter_map(|l| match l {
                DiffLine::Add(s) | DiffLine::Context(s) => Some(s.as_str()),
                DiffLine::Remove(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Apply hunks to existing content.
    fn apply_to_content(&self, content: &str) -> Result<String> {
        let mut lines: Vec<String> = content
            .lines()
            .map(std::string::ToString::to_string)
            .collect();
        let mut offset: i64 = 0;

        for hunk in &self.hunks {
            let start = (hunk.old_start as i64 - 1 + offset) as usize;
            let end = start + hunk.old_count as usize;

            // Verify context matches
            // For now, just apply the changes
            let mut new_lines = Vec::new();
            for line in &hunk.lines {
                match line {
                    DiffLine::Context(s) | DiffLine::Add(s) => {
                        new_lines.push(s.clone());
                    }
                    DiffLine::Remove(_) => {}
                }
            }

            // Replace range
            if end <= lines.len() {
                lines.splice(start..end, new_lines.clone());
            } else {
                // Append if past end
                lines.extend(new_lines);
            }

            // Update offset
            offset += hunk.new_count as i64 - hunk.old_count as i64;
        }

        Ok(lines.join("\n"))
    }
}

impl fmt::Display for FileDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Header
        let old_path = self.original_path.as_ref().unwrap_or(&self.path);
        writeln!(f, "--- a/{}", old_path.display())?;
        writeln!(f, "+++ b/{}", self.path.display())?;

        // Hunks
        for hunk in &self.hunks {
            write!(f, "{hunk}")?;
        }

        Ok(())
    }
}

/// File operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOperation {
    /// Create new file.
    Create,
    /// Delete file.
    Delete,
    /// Modify file.
    Modify,
    /// Rename file.
    Rename,
}

/// A hunk (contiguous change block).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    /// Old file start line.
    pub old_start: u32,
    /// Old file line count.
    pub old_count: u32,
    /// New file start line.
    pub new_start: u32,
    /// New file line count.
    pub new_count: u32,
    /// Lines in the hunk.
    pub lines: Vec<DiffLine>,
    /// Context before (for display).
    pub context_before: String,
    /// Context after (for display).
    pub context_after: String,
}

impl Hunk {
    /// Create a new empty hunk.
    pub fn new(old_start: u32, new_start: u32) -> Self {
        Self {
            old_start,
            old_count: 0,
            new_start,
            new_count: 0,
            lines: Vec::new(),
            context_before: String::new(),
            context_after: String::new(),
        }
    }

    /// Add a context line.
    pub fn add_context(&mut self, line: impl Into<String>) {
        self.lines.push(DiffLine::Context(line.into()));
        self.old_count += 1;
        self.new_count += 1;
    }

    /// Add an addition line.
    pub fn add_addition(&mut self, line: impl Into<String>) {
        self.lines.push(DiffLine::Add(line.into()));
        self.new_count += 1;
    }

    /// Add a removal line.
    pub fn add_removal(&mut self, line: impl Into<String>) {
        self.lines.push(DiffLine::Remove(line.into()));
        self.old_count += 1;
    }

    /// Reverse this hunk.
    pub fn reverse(&self) -> Self {
        Self {
            old_start: self.new_start,
            old_count: self.new_count,
            new_start: self.old_start,
            new_count: self.old_count,
            lines: self.lines.iter().map(DiffLine::reverse).collect(),
            context_before: self.context_after.clone(),
            context_after: self.context_before.clone(),
        }
    }
}

impl fmt::Display for Hunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_count, self.new_start, self.new_count
        )?;

        for line in &self.lines {
            writeln!(f, "{line}")?;
        }

        Ok(())
    }
}

/// A single diff line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLine {
    /// Context line (unchanged).
    Context(String),
    /// Added line.
    Add(String),
    /// Removed line.
    Remove(String),
}

impl DiffLine {
    /// Get the line content.
    pub fn content(&self) -> &str {
        match self {
            Self::Context(s) | Self::Add(s) | Self::Remove(s) => s,
        }
    }

    /// Check if this is an addition.
    pub fn is_add(&self) -> bool {
        matches!(self, Self::Add(_))
    }

    /// Check if this is a removal.
    pub fn is_remove(&self) -> bool {
        matches!(self, Self::Remove(_))
    }

    /// Check if this is context.
    pub fn is_context(&self) -> bool {
        matches!(self, Self::Context(_))
    }

    /// Reverse this diff line.
    pub fn reverse(&self) -> Self {
        match self {
            Self::Context(s) => Self::Context(s.clone()),
            Self::Add(s) => Self::Remove(s.clone()),
            Self::Remove(s) => Self::Add(s.clone()),
        }
    }
}

impl fmt::Display for DiffLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(s) => write!(f, " {s}"),
            Self::Add(s) => write!(f, "+{s}"),
            Self::Remove(s) => write!(f, "-{s}"),
        }
    }
}

/// Diff parser.
#[allow(dead_code)]
pub struct DiffParser {
    /// Context lines to include.
    context_lines: usize,
}

impl DiffParser {
    /// Create a new parser.
    pub fn new() -> Self {
        Self { context_lines: 3 }
    }

    /// Parse a unified diff.
    pub fn parse(&self, diff_text: &str) -> Result<UnifiedDiff> {
        let mut files = Vec::new();
        let mut current_file: Option<FileDiff> = None;
        let mut current_hunk: Option<Hunk> = None;

        for line in diff_text.lines() {
            if line.starts_with("---") {
                // Start of new file
                if let Some(mut file) = current_file.take() {
                    if let Some(hunk) = current_hunk.take() {
                        file.hunks.push(hunk);
                    }
                    files.push(file);
                }

                let path = line
                    .strip_prefix("--- ")
                    .and_then(|s| s.strip_prefix("a/"))
                    .unwrap_or(&line[4..]);

                current_file = Some(FileDiff {
                    path: PathBuf::from(path),
                    original_path: Some(PathBuf::from(path)),
                    operation: FileOperation::Modify,
                    hunks: Vec::new(),
                    mode: None,
                    is_binary: false,
                });
            } else if line.starts_with("+++") {
                if let Some(ref mut file) = current_file {
                    let path = line
                        .strip_prefix("+++ ")
                        .and_then(|s| s.strip_prefix("b/"))
                        .unwrap_or(&line[4..]);
                    file.path = PathBuf::from(path);

                    // Detect operation
                    if path == "/dev/null" {
                        file.operation = FileOperation::Delete;
                    } else if file.original_path.as_ref().map(|p| p.to_string_lossy())
                        == Some("/dev/null".into())
                    {
                        file.operation = FileOperation::Create;
                    }
                }
            } else if line.starts_with("@@") {
                // New hunk
                if let Some(ref mut file) = current_file {
                    if let Some(hunk) = current_hunk.take() {
                        file.hunks.push(hunk);
                    }

                    if let Some(hunk) = self.parse_hunk_header(line) {
                        current_hunk = Some(hunk);
                    }
                }
            } else if let Some(ref mut hunk) = current_hunk {
                // Hunk content
                if let Some(rest) = line.strip_prefix('+') {
                    hunk.add_addition(rest);
                } else if let Some(rest) = line.strip_prefix('-') {
                    hunk.add_removal(rest);
                } else if let Some(rest) = line.strip_prefix(' ') {
                    hunk.add_context(rest);
                } else if line.is_empty() {
                    hunk.add_context("");
                }
            }
        }

        // Don't forget the last file
        if let Some(mut file) = current_file {
            if let Some(hunk) = current_hunk {
                file.hunks.push(hunk);
            }
            files.push(file);
        }

        Ok(UnifiedDiff {
            files,
            original_text: Some(diff_text.to_string()),
        })
    }

    /// Parse a hunk header line.
    fn parse_hunk_header(&self, line: &str) -> Option<Hunk> {
        // Format: @@ -old_start,old_count +new_start,new_count @@
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }

        let old_range = parts.get(1)?;
        let new_range = parts.get(2)?;

        let (old_start, old_count) = self.parse_range(old_range.strip_prefix('-')?)?;
        let (new_start, new_count) = self.parse_range(new_range.strip_prefix('+')?)?;

        Some(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            lines: Vec::new(),
            context_before: String::new(),
            context_after: String::new(),
        })
    }

    /// Parse a range like "1,5" or "1".
    fn parse_range(&self, s: &str) -> Option<(u32, u32)> {
        if let Some((start, count)) = s.split_once(',') {
            Some((start.parse().ok()?, count.parse().ok()?))
        } else {
            let start: u32 = s.parse().ok()?;
            Some((start, 1))
        }
    }
}

impl Default for DiffParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute diff between two strings.
pub fn diff(old: &str, new: &str) -> UnifiedDiff {
    use similar::{ChangeTag, TextDiff};

    let text_diff = TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();
    let mut current_hunk: Option<Hunk> = None;
    let mut old_line = 1u32;
    let mut new_line = 1u32;

    for change in text_diff.iter_all_changes() {
        let tag = change.tag();
        let content = change.value().trim_end_matches('\n');

        match tag {
            ChangeTag::Equal => {
                if let Some(ref mut hunk) = current_hunk {
                    hunk.add_context(content);
                }
                old_line += 1;
                new_line += 1;
            }
            ChangeTag::Delete => {
                if current_hunk.is_none() {
                    current_hunk = Some(Hunk::new(old_line, new_line));
                }
                if let Some(ref mut hunk) = current_hunk {
                    hunk.add_removal(content);
                }
                old_line += 1;
            }
            ChangeTag::Insert => {
                if current_hunk.is_none() {
                    current_hunk = Some(Hunk::new(old_line, new_line));
                }
                if let Some(ref mut hunk) = current_hunk {
                    hunk.add_addition(content);
                }
                new_line += 1;
            }
        }

        // Finalize hunk if we have enough trailing context
        if tag == ChangeTag::Equal
            && let Some(ref hunk) = current_hunk
        {
            let context_count = hunk
                .lines
                .iter()
                .rev()
                .take_while(|l| l.is_context())
                .count();

            if context_count >= 3 {
                hunks.push(current_hunk.take().unwrap());
            }
        }
    }

    // Push remaining hunk
    if let Some(hunk) = current_hunk {
        hunks.push(hunk);
    }

    let file = FileDiff {
        path: PathBuf::from("file"),
        original_path: None,
        operation: FileOperation::Modify,
        hunks,
        mode: None,
        is_binary: false,
    };

    UnifiedDiff {
        files: if file.hunks.is_empty() {
            Vec::new()
        } else {
            vec![file]
        },
        original_text: None,
    }
}

/// Compute diff between two files.
pub async fn diff_files(old_path: &Path, new_path: &Path) -> Result<UnifiedDiff> {
    let old_content = tokio::fs::read_to_string(old_path)
        .await
        .map_err(CortexError::Io)?;
    let new_content = tokio::fs::read_to_string(new_path)
        .await
        .map_err(CortexError::Io)?;

    let mut result = diff(&old_content, &new_content);

    if let Some(ref mut file) = result.files.first_mut() {
        file.path = new_path.to_path_buf();
        file.original_path = Some(old_path.to_path_buf());
    }

    Ok(result)
}

/// Compute diff between two snapshots.
pub fn diff_snapshots(old: &Snapshot, new: &Snapshot) -> UnifiedDiff {
    let mut files = Vec::new();

    // Find all paths in both snapshots
    let mut all_paths: std::collections::HashSet<&PathBuf> =
        old.files.keys().chain(new.files.keys()).collect();

    let mut sorted_paths: Vec<_> = all_paths.drain().collect();
    sorted_paths.sort();

    for path in sorted_paths {
        let old_state = old.files.get(path);
        let new_state = new.files.get(path);

        match (old_state, new_state) {
            (
                Some(FileState::Exists {
                    content: old_content,
                    ..
                }),
                Some(FileState::Exists {
                    content: new_content,
                    ..
                }),
            ) => {
                if old_content != new_content {
                    let old_str = String::from_utf8_lossy(old_content);
                    let new_str = String::from_utf8_lossy(new_content);
                    let mut d = diff(&old_str, &new_str);
                    if let Some(mut file_diff) = d.files.pop() {
                        file_diff.path = path.clone();
                        file_diff.original_path = Some(path.clone());
                        file_diff.operation = FileOperation::Modify;
                        files.push(file_diff);
                    }
                }
            }
            (None | Some(FileState::NotExists), Some(FileState::Exists { content, .. })) => {
                // Created
                let content_str = String::from_utf8_lossy(content);
                files.push(FileDiff::new_file(path, &content_str));
            }
            (Some(FileState::Exists { content, .. }), None | Some(FileState::NotExists)) => {
                // Deleted
                let content_str = String::from_utf8_lossy(content);
                files.push(FileDiff::delete_file(path, &content_str));
            }
            _ => {}
        }
    }

    UnifiedDiff {
        files,
        original_text: None,
    }
}

/// Apply result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyResult {
    /// Files modified.
    pub files_modified: usize,
    /// Lines added.
    pub lines_added: usize,
    /// Lines removed.
    pub lines_removed: usize,
    /// Errors encountered.
    pub errors: Vec<FileError>,
}

impl ApplyResult {
    /// Check if apply was successful.
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Apply stats for a single file.
#[derive(Debug, Clone, Default)]
pub struct ApplyStats {
    /// Lines added.
    pub lines_added: usize,
    /// Lines removed.
    pub lines_removed: usize,
}

/// File error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileError {
    /// File path.
    pub path: PathBuf,
    /// Error message.
    pub error: String,
}

/// Diff builder for creating diffs programmatically.
#[derive(Debug, Default)]
pub struct DiffBuilder {
    files: Vec<FileDiff>,
}

impl DiffBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a file creation.
    pub fn create_file(mut self, path: impl Into<PathBuf>, content: &str) -> Self {
        self.files.push(FileDiff::new_file(path, content));
        self
    }

    /// Add a file deletion.
    pub fn delete_file(mut self, path: impl Into<PathBuf>, content: &str) -> Self {
        self.files.push(FileDiff::delete_file(path, content));
        self
    }

    /// Add a file modification.
    pub fn modify_file(mut self, path: impl Into<PathBuf>, old: &str, new: &str) -> Self {
        let mut file_diff = diff(old, new);
        if let Some(mut file) = file_diff.files.pop() {
            file.path = path.into();
            self.files.push(file);
        }
        self
    }

    /// Build the unified diff.
    pub fn build(self) -> UnifiedDiff {
        UnifiedDiff {
            files: self.files,
            original_text: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_computation() {
        let old = "line 1\nline 2\nline 3";
        let new = "line 1\nline 2 modified\nline 3\nline 4";

        let diff = diff(old, new);
        assert!(!diff.is_empty());
        assert!(diff.lines_added() > 0);
    }

    #[test]
    fn test_diff_parse() {
        let diff_text = r#"--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,4 @@
 line 1
-line 2
+line 2 modified
 line 3
+line 4
"#;

        let diff = UnifiedDiff::parse(diff_text).unwrap();
        assert_eq!(diff.file_count(), 1);
        assert_eq!(diff.files[0].hunks.len(), 1);
    }

    #[test]
    fn test_hunk_display() {
        let mut hunk = Hunk::new(1, 1);
        hunk.add_context("context");
        hunk.add_removal("removed");
        hunk.add_addition("added");

        let s = hunk.to_string();
        assert!(s.contains("@@"));
        assert!(s.contains("-removed"));
        assert!(s.contains("+added"));
    }

    #[test]
    fn test_diff_builder() {
        let diff = DiffBuilder::new()
            .create_file("new.txt", "hello\nworld")
            .build();

        assert_eq!(diff.file_count(), 1);
        assert_eq!(diff.files[0].operation, FileOperation::Create);
    }

    #[test]
    fn test_diff_line() {
        let add = DiffLine::Add("test".to_string());
        assert!(add.is_add());
        assert_eq!(add.content(), "test");
        assert_eq!(format!("{}", add), "+test");
    }

    #[test]
    fn test_diff_snapshots() {
        let mut old = Snapshot::new("old", "Old state");
        old.add_file(
            "modified.txt",
            FileState::Exists {
                content: b"original content".to_vec(),
                permissions: None,
                modified: None,
            },
        );
        old.add_file(
            "deleted.txt",
            FileState::Exists {
                content: b"bye".to_vec(),
                permissions: None,
                modified: None,
            },
        );

        let mut new = Snapshot::new("new", "New state");
        new.add_file(
            "modified.txt",
            FileState::Exists {
                content: b"new content".to_vec(),
                permissions: None,
                modified: None,
            },
        );
        new.add_file(
            "created.txt",
            FileState::Exists {
                content: b"hello".to_vec(),
                permissions: None,
                modified: None,
            },
        );

        let diff = diff_snapshots(&old, &new);
        assert_eq!(diff.file_count(), 3);

        let paths: Vec<_> = diff
            .files
            .iter()
            .map(|f| f.path.to_str().unwrap())
            .collect();
        assert!(paths.contains(&"modified.txt"));
        assert!(paths.contains(&"created.txt"));
        assert!(paths.contains(&"deleted.txt"));
    }
}
