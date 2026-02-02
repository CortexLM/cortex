//! Diff utilities for comparing file states.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A diff for a single file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    /// File path.
    pub path: PathBuf,
    /// Content before change.
    pub before: String,
    /// Content after change.
    pub after: String,
    /// Number of additions.
    pub additions: usize,
    /// Number of deletions.
    pub deletions: usize,
    /// Diff hunks.
    pub hunks: Vec<DiffHunk>,
}

impl FileDiff {
    pub fn new(path: PathBuf, before: String, after: String) -> Self {
        let (additions, deletions, hunks) = Self::compute_diff(&before, &after);
        Self {
            path,
            before,
            after,
            additions,
            deletions,
            hunks,
        }
    }

    fn compute_diff(before: &str, after: &str) -> (usize, usize, Vec<DiffHunk>) {
        let before_lines: Vec<&str> = before.lines().collect();
        let after_lines: Vec<&str> = after.lines().collect();

        let mut additions = 0;
        let mut deletions = 0;
        let mut hunks = Vec::new();

        // Simple line-by-line diff
        let mut current_hunk: Option<DiffHunk> = None;
        let mut i = 0;
        let mut j = 0;

        while i < before_lines.len() || j < after_lines.len() {
            let before_line = before_lines.get(i);
            let after_line = after_lines.get(j);

            match (before_line, after_line) {
                (Some(b), Some(a)) if b == a => {
                    // Lines match, finalize current hunk if any
                    if let Some(h) = current_hunk.take() {
                        hunks.push(h);
                    }
                    i += 1;
                    j += 1;
                }
                (Some(b), Some(a)) => {
                    // Lines differ
                    let hunk = current_hunk.get_or_insert_with(|| DiffHunk::new(i + 1, j + 1));
                    hunk.add_deletion(b.to_string());
                    hunk.add_addition(a.to_string());
                    deletions += 1;
                    additions += 1;
                    i += 1;
                    j += 1;
                }
                (Some(b), None) => {
                    // Line deleted
                    let hunk = current_hunk.get_or_insert_with(|| DiffHunk::new(i + 1, j + 1));
                    hunk.add_deletion(b.to_string());
                    deletions += 1;
                    i += 1;
                }
                (None, Some(a)) => {
                    // Line added
                    let hunk = current_hunk.get_or_insert_with(|| DiffHunk::new(i + 1, j + 1));
                    hunk.add_addition(a.to_string());
                    additions += 1;
                    j += 1;
                }
                (None, None) => break,
            }
        }

        if let Some(h) = current_hunk {
            hunks.push(h);
        }

        (additions, deletions, hunks)
    }

    /// Format as unified diff.
    pub fn format_unified(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("--- a/{}\n", self.path.display()));
        output.push_str(&format!("+++ b/{}\n", self.path.display()));

        for hunk in &self.hunks {
            output.push_str(&hunk.format_header());
            for line in &hunk.lines {
                output.push_str(&format!("{}\n", line));
            }
        }

        output
    }

    /// Get a summary of changes.
    pub fn summary(&self) -> String {
        format!(
            "{}: +{} -{} lines",
            self.path.display(),
            self.additions,
            self.deletions
        )
    }

    /// Check if there are any changes.
    pub fn has_changes(&self) -> bool {
        self.additions > 0 || self.deletions > 0
    }
}

/// A hunk in a diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    /// Starting line in old file.
    pub old_start: usize,
    /// Starting line in new file.
    pub new_start: usize,
    /// Number of lines in old file.
    pub old_lines: usize,
    /// Number of lines in new file.
    pub new_lines: usize,
    /// Diff lines (prefixed with +, -, or space).
    pub lines: Vec<String>,
}

impl DiffHunk {
    pub fn new(old_start: usize, new_start: usize) -> Self {
        Self {
            old_start,
            new_start,
            old_lines: 0,
            new_lines: 0,
            lines: Vec::new(),
        }
    }

    pub fn add_addition(&mut self, line: String) {
        self.lines.push(format!("+{}", line));
        self.new_lines += 1;
    }

    pub fn add_deletion(&mut self, line: String) {
        self.lines.push(format!("-{}", line));
        self.old_lines += 1;
    }

    pub fn add_context(&mut self, line: String) {
        self.lines.push(format!(" {}", line));
        self.old_lines += 1;
        self.new_lines += 1;
    }

    pub fn format_header(&self) -> String {
        format!(
            "@@ -{},{} +{},{} @@\n",
            self.old_start, self.old_lines, self.new_start, self.new_lines
        )
    }
}

/// Parse a unified diff string into FileDiffs.
pub fn parse_unified_diff(diff: &str) -> Vec<FileDiff> {
    let mut diffs = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut before = String::new();
    let mut after = String::new();

    for line in diff.lines() {
        if line.starts_with("--- a/") {
            // Start of new file diff
            if let Some(path) = current_path.take() {
                diffs.push(FileDiff::new(path, before.clone(), after.clone()));
            }
            before.clear();
            after.clear();
            // Path will be set from +++ line
        } else if line.starts_with("+++ b/") {
            current_path = Some(PathBuf::from(&line[6..]));
        } else if line.starts_with('+') && !line.starts_with("+++") {
            after.push_str(&line[1..]);
            after.push('\n');
        } else if line.starts_with('-') && !line.starts_with("---") {
            before.push_str(&line[1..]);
            before.push('\n');
        } else if line.starts_with(' ') {
            before.push_str(&line[1..]);
            before.push('\n');
            after.push_str(&line[1..]);
            after.push('\n');
        }
    }

    if let Some(path) = current_path {
        diffs.push(FileDiff::new(path, before, after));
    }

    diffs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_diff() {
        let before = "line 1\nline 2\nline 3";
        let after = "line 1\nmodified line 2\nline 3\nline 4";

        let diff = FileDiff::new(
            PathBuf::from("test.txt"),
            before.to_string(),
            after.to_string(),
        );

        assert_eq!(diff.additions, 2); // modified + new line
        assert_eq!(diff.deletions, 1); // original line 2
        assert!(diff.has_changes());
    }

    #[test]
    fn test_no_changes() {
        let content = "line 1\nline 2";
        let diff = FileDiff::new(
            PathBuf::from("test.txt"),
            content.to_string(),
            content.to_string(),
        );

        assert_eq!(diff.additions, 0);
        assert_eq!(diff.deletions, 0);
        assert!(!diff.has_changes());
    }
}
