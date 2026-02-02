//! Search result types.

use std::path::PathBuf;

/// Mode of search operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SearchMode {
    /// Match against file names only.
    #[default]
    FileName,

    /// Match against full relative paths.
    FullPath,

    /// Match against file contents.
    Content,

    /// Match against both file names and paths.
    FileNameAndPath,
}

impl SearchMode {
    /// Returns a human-readable description of the mode.
    pub fn description(&self) -> &'static str {
        match self {
            Self::FileName => "file name",
            Self::FullPath => "full path",
            Self::Content => "file content",
            Self::FileNameAndPath => "file name and path",
        }
    }
}

/// A single search match result.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// The matched file path (relative to search root).
    pub path: PathBuf,

    /// The absolute path to the file.
    pub absolute_path: PathBuf,

    /// The file name.
    pub file_name: String,

    /// The fuzzy match score (higher is better).
    pub score: u32,

    /// The mode used for this match.
    pub mode: SearchMode,

    /// Character indices that matched in the search string.
    /// For highlighting in UI.
    pub match_indices: Vec<usize>,

    /// Line number where match was found (for content search).
    pub line_number: Option<usize>,

    /// The matched line content (for content search).
    pub matched_line: Option<String>,

    /// File size in bytes.
    pub file_size: u64,

    /// File modification time (Unix timestamp).
    pub modified_time: Option<u64>,
}

impl SearchMatch {
    /// Creates a new search match for file/path matching.
    pub fn new(
        path: PathBuf,
        absolute_path: PathBuf,
        file_name: String,
        score: u32,
        mode: SearchMode,
    ) -> Self {
        Self {
            path,
            absolute_path,
            file_name,
            score,
            mode,
            match_indices: Vec::new(),
            line_number: None,
            matched_line: None,
            file_size: 0,
            modified_time: None,
        }
    }

    /// Creates a new search match for content matching.
    pub fn content_match(
        path: PathBuf,
        absolute_path: PathBuf,
        file_name: String,
        score: u32,
        line_number: usize,
        matched_line: String,
    ) -> Self {
        Self {
            path,
            absolute_path,
            file_name,
            score,
            mode: SearchMode::Content,
            match_indices: Vec::new(),
            line_number: Some(line_number),
            matched_line: Some(matched_line),
            file_size: 0,
            modified_time: None,
        }
    }

    /// Sets the match indices for highlighting.
    pub fn with_match_indices(mut self, indices: Vec<usize>) -> Self {
        self.match_indices = indices;
        self
    }

    /// Sets the file size.
    pub fn with_file_size(mut self, size: u64) -> Self {
        self.file_size = size;
        self
    }

    /// Sets the modification time.
    pub fn with_modified_time(mut self, time: u64) -> Self {
        self.modified_time = Some(time);
        self
    }

    /// Returns the display string for this match.
    pub fn display(&self) -> String {
        if let Some(line_num) = self.line_number {
            format!("{}:{}", self.path.display(), line_num)
        } else {
            self.path.display().to_string()
        }
    }

    /// Returns the text that was matched against.
    pub fn matched_text(&self) -> &str {
        match self.mode {
            SearchMode::FileName | SearchMode::FileNameAndPath => &self.file_name,
            SearchMode::FullPath => self.path.to_str().unwrap_or(&self.file_name),
            SearchMode::Content => self.matched_line.as_deref().unwrap_or(""),
        }
    }
}

impl PartialEq for SearchMatch {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.line_number == other.line_number
    }
}

impl Eq for SearchMatch {}

impl PartialOrd for SearchMatch {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SearchMatch {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Higher score first, then shorter path, then alphabetically
        other
            .score
            .cmp(&self.score)
            .then_with(|| {
                self.path
                    .as_os_str()
                    .len()
                    .cmp(&other.path.as_os_str().len())
            })
            .then_with(|| self.path.cmp(&other.path))
    }
}

/// Statistics about a search operation.
#[derive(Debug, Clone, Default)]
pub struct SearchStats {
    /// Total number of files indexed.
    pub files_indexed: usize,

    /// Total number of files searched.
    pub files_searched: usize,

    /// Number of matches found.
    pub matches_found: usize,

    /// Time taken for the search in milliseconds.
    pub search_time_ms: u64,

    /// Whether the search was performed from cache.
    pub from_cache: bool,
}

impl SearchStats {
    /// Creates new search statistics.
    pub fn new(
        files_indexed: usize,
        files_searched: usize,
        matches_found: usize,
        search_time_ms: u64,
        from_cache: bool,
    ) -> Self {
        Self {
            files_indexed,
            files_searched,
            matches_found,
            search_time_ms,
            from_cache,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_mode_description() {
        assert_eq!(SearchMode::FileName.description(), "file name");
        assert_eq!(SearchMode::FullPath.description(), "full path");
        assert_eq!(SearchMode::Content.description(), "file content");
    }

    #[test]
    fn test_search_match_ordering() {
        let match1 = SearchMatch::new(
            PathBuf::from("short.rs"),
            PathBuf::from("/root/short.rs"),
            "short.rs".to_string(),
            100,
            SearchMode::FileName,
        );

        let match2 = SearchMatch::new(
            PathBuf::from("very/long/path.rs"),
            PathBuf::from("/root/very/long/path.rs"),
            "path.rs".to_string(),
            100,
            SearchMode::FileName,
        );

        let match3 = SearchMatch::new(
            PathBuf::from("another.rs"),
            PathBuf::from("/root/another.rs"),
            "another.rs".to_string(),
            50,
            SearchMode::FileName,
        );

        let mut matches = [match3.clone(), match2.clone(), match1.clone()];
        matches.sort();

        // Higher score first
        assert_eq!(matches[0].score, 100);
        assert_eq!(matches[1].score, 100);
        assert_eq!(matches[2].score, 50);

        // Same score: shorter path first
        assert!(matches[0].path.as_os_str().len() <= matches[1].path.as_os_str().len());
    }

    #[test]
    fn test_content_match() {
        let m = SearchMatch::content_match(
            PathBuf::from("test.rs"),
            PathBuf::from("/root/test.rs"),
            "test.rs".to_string(),
            75,
            42,
            "fn main() {}".to_string(),
        );

        assert_eq!(m.mode, SearchMode::Content);
        assert_eq!(m.line_number, Some(42));
        assert_eq!(m.matched_line.as_deref(), Some("fn main() {}"));
        assert_eq!(m.display(), "test.rs:42");
    }
}
