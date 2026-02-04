//! Hunk and file change data structures.

use std::path::PathBuf;

/// A line within a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    /// A context line (unchanged).
    Context(String),
    /// A line to be added.
    Add(String),
    /// A line to be removed.
    Remove(String),
}

impl HunkLine {
    /// Get the content of this line.
    pub fn content(&self) -> &str {
        match self {
            Self::Context(s) | Self::Add(s) | Self::Remove(s) => s,
        }
    }

    /// Check if this is a context line.
    pub fn is_context(&self) -> bool {
        matches!(self, Self::Context(_))
    }

    /// Check if this is an add line.
    pub fn is_add(&self) -> bool {
        matches!(self, Self::Add(_))
    }

    /// Check if this is a remove line.
    pub fn is_remove(&self) -> bool {
        matches!(self, Self::Remove(_))
    }

    /// Get the line for matching purposes (context or remove lines).
    pub fn match_content(&self) -> Option<&str> {
        match self {
            Self::Context(s) | Self::Remove(s) => Some(s),
            Self::Add(_) => None,
        }
    }
}

/// A hunk represents a contiguous block of changes.
#[derive(Debug, Clone)]
pub struct Hunk {
    /// Starting line number in the original file (1-indexed).
    pub old_start: usize,
    /// Number of lines in the original file this hunk spans.
    pub old_count: usize,
    /// Starting line number in the new file (1-indexed).
    pub new_start: usize,
    /// Number of lines in the new file this hunk spans.
    pub new_count: usize,
    /// Optional section header (function name, etc.).
    pub section_header: Option<String>,
    /// The lines in this hunk.
    pub lines: Vec<HunkLine>,
}

impl Hunk {
    /// Create a new hunk.
    pub fn new(old_start: usize, old_count: usize, new_start: usize, new_count: usize) -> Self {
        Self {
            old_start,
            old_count,
            new_start,
            new_count,
            section_header: None,
            lines: Vec::new(),
        }
    }

    /// Add a line to this hunk.
    pub fn add_line(&mut self, line: HunkLine) {
        self.lines.push(line);
    }

    /// Get the lines that should be matched against the original file.
    pub fn match_lines(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|l| l.match_content())
            .collect()
    }

    /// Get the lines that will appear in the new file.
    pub fn result_lines(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|l| match l {
                HunkLine::Context(s) | HunkLine::Add(s) => Some(s.as_str()),
                HunkLine::Remove(_) => None,
            })
            .collect()
    }

    /// Calculate the number of lines added by this hunk.
    pub fn lines_added(&self) -> usize {
        self.lines.iter().filter(|l| l.is_add()).count()
    }

    /// Calculate the number of lines removed by this hunk.
    pub fn lines_removed(&self) -> usize {
        self.lines.iter().filter(|l| l.is_remove()).count()
    }

    /// Calculate the net change in line count.
    pub fn line_delta(&self) -> isize {
        self.lines_added() as isize - self.lines_removed() as isize
    }

    /// Get the number of context lines at the start of this hunk.
    pub fn leading_context_count(&self) -> usize {
        self.lines.iter().take_while(|l| l.is_context()).count()
    }

    /// Get the number of context lines at the end of this hunk.
    pub fn trailing_context_count(&self) -> usize {
        self.lines
            .iter()
            .rev()
            .take_while(|l| l.is_context())
            .count()
    }

    /// Check if this hunk is empty (no actual changes).
    pub fn is_empty(&self) -> bool {
        !self.lines.iter().any(|l| l.is_add() || l.is_remove())
    }

    /// Validate that the hunk line counts match the header.
    pub fn validate(&self) -> bool {
        let context_and_remove = self
            .lines
            .iter()
            .filter(|l| l.is_context() || l.is_remove())
            .count();
        let context_and_add = self
            .lines
            .iter()
            .filter(|l| l.is_context() || l.is_add())
            .count();

        context_and_remove == self.old_count && context_and_add == self.new_count
    }
}

/// Represents changes to a single file.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// The original file path (None for new files).
    pub old_path: Option<PathBuf>,
    /// The new file path (None for deleted files).
    pub new_path: Option<PathBuf>,
    /// The hunks (changes) for this file.
    pub hunks: Vec<Hunk>,
    /// Whether this is a new file.
    pub is_new_file: bool,
    /// Whether this file is being deleted.
    pub is_deleted: bool,
    /// Whether this is a file rename.
    pub is_rename: bool,
    /// The old file mode (git diff).
    pub old_mode: Option<String>,
    /// The new file mode (git diff).
    pub new_mode: Option<String>,
    /// Binary file indicator.
    pub is_binary: bool,
}

impl FileChange {
    /// Create a new file change.
    pub fn new(old_path: Option<PathBuf>, new_path: Option<PathBuf>) -> Self {
        let is_new_file = old_path
            .as_ref()
            .is_some_and(|p| p.as_os_str() == "/dev/null")
            || old_path.is_none();
        let is_deleted = new_path
            .as_ref()
            .is_some_and(|p| p.as_os_str() == "/dev/null")
            || (new_path.is_none() && old_path.is_some());

        Self {
            old_path: if is_new_file { None } else { old_path },
            new_path: if is_deleted { None } else { new_path },
            hunks: Vec::new(),
            is_new_file,
            is_deleted,
            is_rename: false,
            old_mode: None,
            new_mode: None,
            is_binary: false,
        }
    }

    /// Get the effective file path (new path for modifications, old path for deletions).
    pub fn effective_path(&self) -> Option<&PathBuf> {
        self.new_path.as_ref().or(self.old_path.as_ref())
    }

    /// Add a hunk to this file change.
    pub fn add_hunk(&mut self, hunk: Hunk) {
        self.hunks.push(hunk);
    }

    /// Calculate the total number of lines added.
    pub fn total_lines_added(&self) -> usize {
        self.hunks.iter().map(Hunk::lines_added).sum()
    }

    /// Calculate the total number of lines removed.
    pub fn total_lines_removed(&self) -> usize {
        self.hunks.iter().map(Hunk::lines_removed).sum()
    }

    /// Check if the hunks overlap with each other.
    pub fn has_overlapping_hunks(&self) -> bool {
        if self.hunks.len() < 2 {
            return false;
        }

        let mut sorted_hunks: Vec<&Hunk> = self.hunks.iter().collect();
        sorted_hunks.sort_by_key(|h| h.old_start);

        for i in 1..sorted_hunks.len() {
            let prev = sorted_hunks[i - 1];
            let curr = sorted_hunks[i];

            let prev_end = prev.old_start + prev.old_count;
            if prev_end > curr.old_start {
                return true;
            }
        }

        false
    }
}

/// Represents a simple search/replace operation.
#[derive(Debug, Clone)]
pub struct SearchReplace {
    /// The file path.
    pub path: PathBuf,
    /// The text to search for.
    pub search: String,
    /// The text to replace with.
    pub replace: String,
}

impl SearchReplace {
    /// Create a new search/replace operation.
    pub fn new(
        path: impl Into<PathBuf>,
        search: impl Into<String>,
        replace: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            search: search.into(),
            replace: replace.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hunk_line_content() {
        let ctx = HunkLine::Context("hello".to_string());
        let add = HunkLine::Add("world".to_string());
        let rem = HunkLine::Remove("foo".to_string());

        assert_eq!(ctx.content(), "hello");
        assert_eq!(add.content(), "world");
        assert_eq!(rem.content(), "foo");
    }

    #[test]
    fn test_hunk_match_lines() {
        let mut hunk = Hunk::new(1, 3, 1, 4);
        hunk.add_line(HunkLine::Context("line 1".to_string()));
        hunk.add_line(HunkLine::Remove("old line".to_string()));
        hunk.add_line(HunkLine::Add("new line".to_string()));
        hunk.add_line(HunkLine::Context("line 3".to_string()));

        let match_lines = hunk.match_lines();
        assert_eq!(match_lines, vec!["line 1", "old line", "line 3"]);
    }

    #[test]
    fn test_hunk_result_lines() {
        let mut hunk = Hunk::new(1, 3, 1, 4);
        hunk.add_line(HunkLine::Context("line 1".to_string()));
        hunk.add_line(HunkLine::Remove("old line".to_string()));
        hunk.add_line(HunkLine::Add("new line".to_string()));
        hunk.add_line(HunkLine::Context("line 3".to_string()));

        let result_lines = hunk.result_lines();
        assert_eq!(result_lines, vec!["line 1", "new line", "line 3"]);
    }

    #[test]
    fn test_hunk_line_delta() {
        let mut hunk = Hunk::new(1, 3, 1, 4);
        hunk.add_line(HunkLine::Context("line 1".to_string()));
        hunk.add_line(HunkLine::Add("new 1".to_string()));
        hunk.add_line(HunkLine::Add("new 2".to_string()));
        hunk.add_line(HunkLine::Remove("old".to_string()));
        hunk.add_line(HunkLine::Context("line 3".to_string()));

        assert_eq!(hunk.lines_added(), 2);
        assert_eq!(hunk.lines_removed(), 1);
        assert_eq!(hunk.line_delta(), 1);
    }

    #[test]
    fn test_file_change_new_file() {
        let change = FileChange::new(
            Some(PathBuf::from("/dev/null")),
            Some(PathBuf::from("new_file.txt")),
        );
        assert!(change.is_new_file);
        assert!(!change.is_deleted);
        assert_eq!(change.new_path, Some(PathBuf::from("new_file.txt")));
    }

    #[test]
    fn test_file_change_deleted_file() {
        let change = FileChange::new(
            Some(PathBuf::from("old_file.txt")),
            Some(PathBuf::from("/dev/null")),
        );
        assert!(!change.is_new_file);
        assert!(change.is_deleted);
        assert_eq!(change.old_path, Some(PathBuf::from("old_file.txt")));
    }

    #[test]
    fn test_overlapping_hunks() {
        let mut change = FileChange::new(
            Some(PathBuf::from("file.txt")),
            Some(PathBuf::from("file.txt")),
        );

        change.add_hunk(Hunk::new(1, 5, 1, 5));
        change.add_hunk(Hunk::new(3, 5, 3, 5)); // Overlaps with first

        assert!(change.has_overlapping_hunks());
    }

    #[test]
    fn test_non_overlapping_hunks() {
        let mut change = FileChange::new(
            Some(PathBuf::from("file.txt")),
            Some(PathBuf::from("file.txt")),
        );

        change.add_hunk(Hunk::new(1, 5, 1, 5));
        change.add_hunk(Hunk::new(10, 5, 10, 5));

        assert!(!change.has_overlapping_hunks());
    }
}
