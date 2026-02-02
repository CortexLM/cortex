//! Patch parsing for multiple formats.

use crate::error::PatchResult;
use crate::hunk::{FileChange, Hunk, HunkLine, SearchReplace};
use std::path::PathBuf;

/// Supported patch formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchFormat {
    /// Standard unified diff format.
    UnifiedDiff,
    /// Git extended diff format.
    GitDiff,
    /// Simple search/replace format.
    SearchReplace,
    /// Unknown format.
    Unknown,
}

impl PatchFormat {
    /// Detect the format of a patch string.
    pub fn detect(patch: &str) -> Self {
        let lines: Vec<&str> = patch.lines().collect();

        if lines.is_empty() {
            return Self::Unknown;
        }

        // Check for git diff format
        if lines.iter().any(|l| l.starts_with("diff --git")) {
            return Self::GitDiff;
        }

        // Check for unified diff format
        if lines
            .iter()
            .any(|l| l.starts_with("--- ") || l.starts_with("+++ "))
        {
            return Self::UnifiedDiff;
        }

        // Check for search/replace format (<<<<<<< SEARCH / ======= / >>>>>>> REPLACE)
        if lines
            .iter()
            .any(|l| l.contains("<<<<<<< SEARCH") || l.contains(">>>>>>> REPLACE"))
        {
            return Self::SearchReplace;
        }

        Self::Unknown
    }
}

/// Parse a patch string into file changes.
pub fn parse_patch(patch: &str) -> PatchResult<Vec<FileChange>> {
    let patch = patch.trim();

    if patch.is_empty() {
        return Ok(vec![]);
    }

    let format = PatchFormat::detect(patch);

    match format {
        PatchFormat::GitDiff => parse_git_diff(patch),
        PatchFormat::UnifiedDiff => parse_unified_diff(patch),
        PatchFormat::SearchReplace => parse_search_replace(patch),
        PatchFormat::Unknown => {
            // Try unified diff as fallback
            parse_unified_diff(patch)
        }
    }
}

/// Parse a standard unified diff.
pub fn parse_unified_diff(patch: &str) -> PatchResult<Vec<FileChange>> {
    let mut file_changes = Vec::new();
    let mut current_change: Option<FileChange> = None;
    let mut current_hunk: Option<Hunk> = None;
    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Detect file header: --- path
        if let Some(old_str) = line.strip_prefix("--- ") {
            // Save previous file change if exists
            if let Some(mut change) = current_change.take() {
                if let Some(hunk) = current_hunk.take() {
                    change.hunks.push(hunk);
                }
                if !change.hunks.is_empty() || change.is_new_file || change.is_deleted {
                    file_changes.push(change);
                }
            }

            let old_path = parse_file_path(old_str);

            // Look for +++ line
            if i + 1 < lines.len()
                && let Some(new_str) = lines[i + 1].strip_prefix("+++ ")
            {
                let new_path = parse_file_path(new_str);
                current_change = Some(FileChange::new(old_path, new_path));
                i += 2;
                continue;
            }
            // Malformed patch, but try to continue
            current_change = Some(FileChange::new(old_path, None));
            i += 1;
            continue;
        }

        // Detect hunk header: @@ -start,count +start,count @@
        if line.starts_with("@@ ") {
            // Save previous hunk
            if let Some(ref mut change) = current_change
                && let Some(hunk) = current_hunk.take()
            {
                change.hunks.push(hunk);
            }

            current_hunk = parse_hunk_header(line);
            i += 1;
            continue;
        }

        // Parse hunk lines
        if let Some(ref mut hunk) = current_hunk {
            if let Some(content) = line.strip_prefix('+') {
                // Addition line - but not the +++ header
                if !content.starts_with("++") {
                    hunk.lines.push(HunkLine::Add(content.to_string()));
                }
            } else if let Some(content) = line.strip_prefix('-') {
                // Removal line - but not the --- header
                if !content.starts_with("--") {
                    hunk.lines.push(HunkLine::Remove(content.to_string()));
                }
            } else if let Some(content) = line.strip_prefix(' ') {
                // Context line
                hunk.lines.push(HunkLine::Context(content.to_string()));
            } else if line.is_empty() {
                // Empty context line
                hunk.lines.push(HunkLine::Context(String::new()));
            } else if line.starts_with('\\') {
                // "\ No newline at end of file" - ignore but don't add as context
            }
            // Skip other lines (like git metadata)
        }

        i += 1;
    }

    // Save final file change and hunk
    if let Some(mut change) = current_change.take() {
        if let Some(hunk) = current_hunk.take() {
            change.hunks.push(hunk);
        }
        if !change.hunks.is_empty() || change.is_new_file || change.is_deleted {
            file_changes.push(change);
        }
    }

    Ok(file_changes)
}

/// Parse a git extended diff format.
pub fn parse_git_diff(patch: &str) -> PatchResult<Vec<FileChange>> {
    let mut file_changes = Vec::new();
    let mut current_change: Option<FileChange> = None;
    let mut current_hunk: Option<Hunk> = None;
    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Git diff header: diff --git a/path b/path
        if line.starts_with("diff --git ") {
            // Save previous file change
            if let Some(mut change) = current_change.take() {
                if let Some(hunk) = current_hunk.take() {
                    change.hunks.push(hunk);
                }
                if !change.hunks.is_empty()
                    || change.is_new_file
                    || change.is_deleted
                    || change.is_binary
                {
                    file_changes.push(change);
                }
            }

            // Parse the git diff header
            let (old_path, new_path) = parse_git_diff_header(line);
            current_change = Some(FileChange::new(old_path, new_path));
            i += 1;
            continue;
        }

        // Handle git-specific metadata
        if let Some(ref mut change) = current_change {
            if let Some(mode) = line.strip_prefix("old mode ") {
                change.old_mode = Some(mode.to_string());
                i += 1;
                continue;
            }
            if let Some(mode) = line.strip_prefix("new mode ") {
                change.new_mode = Some(mode.to_string());
                i += 1;
                continue;
            }
            if let Some(mode) = line.strip_prefix("new file mode ") {
                change.is_new_file = true;
                change.new_mode = Some(mode.to_string());
                i += 1;
                continue;
            }
            if let Some(mode) = line.strip_prefix("deleted file mode ") {
                change.is_deleted = true;
                change.old_mode = Some(mode.to_string());
                i += 1;
                continue;
            }
            if let Some(path) = line.strip_prefix("rename from ") {
                change.is_rename = true;
                change.old_path = Some(PathBuf::from(path));
                i += 1;
                continue;
            }
            if let Some(path) = line.strip_prefix("rename to ") {
                change.is_rename = true;
                change.new_path = Some(PathBuf::from(path));
                i += 1;
                continue;
            }
            if line.starts_with("similarity index ") || line.starts_with("dissimilarity index ") {
                i += 1;
                continue;
            }
            if line.starts_with("index ") {
                i += 1;
                continue;
            }
            if line == "GIT binary patch" || line.starts_with("Binary files ") {
                change.is_binary = true;
                i += 1;
                continue;
            }
        }

        // Standard unified diff parts
        if let Some(path_str) = line.strip_prefix("--- ") {
            if let Some(ref mut change) = current_change {
                let old_path = parse_file_path(path_str);
                if old_path
                    .as_ref()
                    .is_some_and(|p| p.as_os_str() == "/dev/null")
                {
                    change.is_new_file = true;
                    change.old_path = None;
                } else if change.old_path.is_none() || !change.is_rename {
                    change.old_path = old_path;
                }
            }
            i += 1;
            continue;
        }

        if let Some(path_str) = line.strip_prefix("+++ ") {
            if let Some(ref mut change) = current_change {
                let new_path = parse_file_path(path_str);
                if new_path
                    .as_ref()
                    .is_some_and(|p| p.as_os_str() == "/dev/null")
                {
                    change.is_deleted = true;
                    change.new_path = None;
                } else if change.new_path.is_none() || !change.is_rename {
                    change.new_path = new_path;
                }
            }
            i += 1;
            continue;
        }

        // Hunk header
        if line.starts_with("@@ ") {
            if let Some(ref mut change) = current_change
                && let Some(hunk) = current_hunk.take()
            {
                change.hunks.push(hunk);
            }
            current_hunk = parse_hunk_header(line);
            i += 1;
            continue;
        }

        // Hunk content
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
            // Ignore "\ No newline at end of file" and other non-standard lines
        }

        i += 1;
    }

    // Save final file change
    if let Some(mut change) = current_change.take() {
        if let Some(hunk) = current_hunk.take() {
            change.hunks.push(hunk);
        }
        if !change.hunks.is_empty() || change.is_new_file || change.is_deleted || change.is_binary {
            file_changes.push(change);
        }
    }

    Ok(file_changes)
}

/// Parse a search/replace format patch.
///
/// Format:
/// ```text
/// path/to/file.txt
/// <<<<<<< SEARCH
/// old content
/// =======
/// new content
/// >>>>>>> REPLACE
/// ```
pub fn parse_search_replace(patch: &str) -> PatchResult<Vec<FileChange>> {
    let mut file_changes = Vec::new();
    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        // Look for file path followed by SEARCH marker
        if i + 1 < lines.len() && lines[i + 1].contains("<<<<<<< SEARCH") {
            let file_path = PathBuf::from(lines[i].trim());
            i += 2; // Skip file path and SEARCH marker

            let mut search_content = String::new();
            let mut replace_content = String::new();
            let mut in_replace = false;

            while i < lines.len() {
                let line = lines[i];

                if line.contains("=======") {
                    in_replace = true;
                    i += 1;
                    continue;
                }

                if line.contains(">>>>>>> REPLACE") {
                    break;
                }

                if in_replace {
                    if !replace_content.is_empty() {
                        replace_content.push('\n');
                    }
                    replace_content.push_str(line);
                } else {
                    if !search_content.is_empty() {
                        search_content.push('\n');
                    }
                    search_content.push_str(line);
                }
                i += 1;
            }

            // Convert search/replace to a FileChange with hunks
            let search_replace = SearchReplace::new(
                file_path.clone(),
                search_content.clone(),
                replace_content.clone(),
            );
            let change = search_replace_to_file_change(&search_replace)?;
            file_changes.push(change);
        }

        i += 1;
    }

    Ok(file_changes)
}

/// Convert a SearchReplace to a FileChange.
fn search_replace_to_file_change(sr: &SearchReplace) -> PatchResult<FileChange> {
    let mut change = FileChange::new(Some(sr.path.clone()), Some(sr.path.clone()));

    // Create a hunk that removes the search content and adds the replace content
    let search_lines: Vec<&str> = sr.search.lines().collect();
    let replace_lines: Vec<&str> = sr.replace.lines().collect();

    let mut hunk = Hunk::new(
        1, // Will be updated when applied
        search_lines.len(),
        1,
        replace_lines.len(),
    );

    for line in &search_lines {
        hunk.lines.push(HunkLine::Remove((*line).to_string()));
    }

    for line in &replace_lines {
        hunk.lines.push(HunkLine::Add((*line).to_string()));
    }

    change.hunks.push(hunk);

    Ok(change)
}

/// Parse a file path from a diff header line.
fn parse_file_path(path_str: &str) -> Option<PathBuf> {
    let path = path_str.trim();

    // Handle various prefixes: a/, b/, or no prefix
    let path = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path);

    // Remove timestamp if present (e.g., "file.txt\t2024-01-01 00:00:00")
    let path = path.split('\t').next().unwrap_or(path).trim();

    // Remove trailing whitespace and quotes
    let path = path.trim_matches('"');

    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

/// Parse a git diff header line.
fn parse_git_diff_header(line: &str) -> (Option<PathBuf>, Option<PathBuf>) {
    // Format: diff --git a/path b/path
    let rest = line.strip_prefix("diff --git ").unwrap_or(line);

    // Find the split point between a/path and b/path
    // This is tricky because paths can contain spaces
    if let Some(b_idx) = rest.find(" b/") {
        let a_part = &rest[..b_idx];
        let b_part = &rest[b_idx + 1..];

        let old_path = a_part.strip_prefix("a/").map(PathBuf::from);
        let new_path = b_part.strip_prefix("b/").map(PathBuf::from);

        (old_path, new_path)
    } else {
        // Fallback: split on space
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        let old_path = parts
            .first()
            .and_then(|p| p.strip_prefix("a/"))
            .map(PathBuf::from);
        let new_path = parts
            .get(1)
            .and_then(|p| p.strip_prefix("b/"))
            .map(PathBuf::from);
        (old_path, new_path)
    }
}

/// Parse a hunk header line.
fn parse_hunk_header(line: &str) -> Option<Hunk> {
    // Format: @@ -start,count +start,count @@ [section]
    let line = line.trim();

    if !line.starts_with("@@") {
        return None;
    }

    // Find the closing @@
    let end_marker = line[2..].find("@@")?;
    let range_part = line[2..2 + end_marker].trim();
    let section = if 2 + end_marker + 2 < line.len() {
        Some(line[2 + end_marker + 2..].trim().to_string())
    } else {
        None
    };

    let parts: Vec<&str> = range_part.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let (old_start, old_count) = parse_range(parts[0].trim_start_matches('-'))?;
    let (new_start, new_count) = parse_range(parts[1].trim_start_matches('+'))?;

    let mut hunk = Hunk::new(old_start, old_count, new_start, new_count);
    hunk.section_header = section.filter(|s| !s.is_empty());

    Some(hunk)
}

/// Parse a range like "1,5" or "1".
fn parse_range(s: &str) -> Option<(usize, usize)> {
    let parts: Vec<&str> = s.split(',').collect();
    let start: usize = parts.first()?.parse().ok()?;
    let count: usize = parts.get(1).and_then(|c| c.parse().ok()).unwrap_or(1);
    Some((start, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_unified_diff() {
        let patch = "--- a/file.txt\n+++ b/file.txt\n@@ -1,1 +1,1 @@\n";
        assert_eq!(PatchFormat::detect(patch), PatchFormat::UnifiedDiff);
    }

    #[test]
    fn test_detect_git_diff() {
        let patch = "diff --git a/file.txt b/file.txt\nindex abc..def 100644\n";
        assert_eq!(PatchFormat::detect(patch), PatchFormat::GitDiff);
    }

    #[test]
    fn test_detect_search_replace() {
        let patch = "file.txt\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n";
        assert_eq!(PatchFormat::detect(patch), PatchFormat::SearchReplace);
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

        let hunk = &changes[0].hunks[0];
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 3);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_count, 4);
    }

    #[test]
    fn test_parse_git_diff() {
        let patch = r#"diff --git a/test.txt b/test.txt
index abc123..def456 100644
--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,4 @@
 line 1
+new line
 line 2
 line 3
"#;
        let changes = parse_git_diff(patch).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].hunks.len(), 1);
    }

    #[test]
    fn test_parse_new_file() {
        let patch = r#"--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1,2 @@
+line 1
+line 2
"#;
        let changes = parse_unified_diff(patch).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].is_new_file);
        assert_eq!(changes[0].new_path, Some(PathBuf::from("new_file.txt")));
    }

    #[test]
    fn test_parse_deleted_file() {
        let patch = r#"--- a/old_file.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-line 1
-line 2
"#;
        let changes = parse_unified_diff(patch).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].is_deleted);
        assert_eq!(changes[0].old_path, Some(PathBuf::from("old_file.txt")));
    }

    #[test]
    fn test_parse_hunk_header() {
        let hunk = parse_hunk_header("@@ -1,5 +1,6 @@").unwrap();
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 5);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_count, 6);
    }

    #[test]
    fn test_parse_hunk_header_with_section() {
        let hunk = parse_hunk_header("@@ -10,5 +10,6 @@ fn main()").unwrap();
        assert_eq!(hunk.old_start, 10);
        assert_eq!(hunk.section_header, Some("fn main()".to_string()));
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
        assert_eq!(
            parse_file_path("/dev/null"),
            Some(PathBuf::from("/dev/null"))
        );
    }

    #[test]
    fn test_parse_search_replace() {
        let patch = r#"src/file.txt
<<<<<<< SEARCH
old content
more old
=======
new content
more new
even more new
>>>>>>> REPLACE
"#;
        let changes = parse_search_replace(patch).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].effective_path(),
            Some(&PathBuf::from("src/file.txt"))
        );

        let hunk = &changes[0].hunks[0];
        assert_eq!(hunk.lines_removed(), 2);
        assert_eq!(hunk.lines_added(), 3);
    }

    #[test]
    fn test_multiple_files() {
        let patch = r#"--- a/file1.txt
+++ b/file1.txt
@@ -1,1 +1,2 @@
 line 1
+new line
--- a/file2.txt
+++ b/file2.txt
@@ -1,1 +1,1 @@
-old
+new
"#;
        let changes = parse_unified_diff(patch).unwrap();
        assert_eq!(changes.len(), 2);
    }
}
