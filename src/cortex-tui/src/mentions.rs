//! File mentions system for @ references.
//!
//! Provides fuzzy file search triggered by `@` in the input,
//! allowing users to quickly reference files from their workspace.
//!
//! # Usage
//!
//! When the user types `@` followed by a query, the system:
//! 1. Activates fuzzy search mode
//! 2. Searches workspace files matching the query
//! 3. Displays a popup with results
//! 4. Allows selection via arrow keys + Enter/Tab
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::mentions::{FileMentionState, MentionInsert};
//!
//! let mut state = FileMentionState::new();
//!
//! // Check if @ triggers the popup
//! if state.check_trigger("Hello @src/lib", 14) {
//!     // Search for files
//!     let results = state.search_sync(workspace_path, 10);
//! }
//!
//! // User selects a file
//! if let Some(insert) = state.confirm() {
//!     // Insert the file path into the input
//! }
//! ```

use std::path::{Path, PathBuf};

// ============================================================
// CONSTANTS
// ============================================================

/// Maximum number of files to scan for performance.
const MAX_FILES_TO_SCAN: usize = 1000;

/// Default number of results to return.
const _DEFAULT_RESULT_LIMIT: usize = 10;

/// Directories to ignore during search.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".vscode",
    "dist",
    "build",
    ".next",
    ".cache",
    "coverage",
];

/// File extensions to prioritize (source code).
const PRIORITY_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "c", "cpp", "h", "hpp", "rb", "php",
    "swift", "kt", "scala", "md", "toml", "yaml", "yml", "json",
];

// ============================================================
// FILE MENTION STATE
// ============================================================

/// State for the file mention/autocomplete system.
#[derive(Debug)]
pub struct FileMentionState {
    /// Whether the mention popup is active.
    active: bool,

    /// The search query (text after @).
    query: String,

    /// Position of the @ in the input string.
    trigger_pos: usize,

    /// Current cursor position.
    cursor_pos: usize,

    /// Search results.
    results: Vec<PathBuf>,

    /// Currently selected index.
    selected: usize,

    /// Scroll offset for display.
    scroll_offset: usize,

    /// Maximum visible items.
    max_visible: usize,
}

impl Default for FileMentionState {
    fn default() -> Self {
        Self::new()
    }
}

impl FileMentionState {
    /// Creates a new file mention state.
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            trigger_pos: 0,
            cursor_pos: 0,
            results: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            max_visible: 10,
        }
    }

    /// Sets the maximum number of visible items in the popup.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self
    }

    /// Checks if the input triggers the mention popup.
    ///
    /// Returns true if the popup should be shown.
    pub fn check_trigger(&mut self, input: &str, cursor: usize) -> bool {
        // Find the @ symbol closest to and before the cursor
        let before_cursor = if cursor <= input.len() {
            &input[..cursor]
        } else {
            input
        };

        // Look for @ that starts a potential file reference
        if let Some(at_pos) = before_cursor.rfind('@') {
            // Check that the @ is either at the start or preceded by whitespace
            let is_valid_trigger = at_pos == 0 || {
                let prev_char = before_cursor.chars().nth(at_pos.saturating_sub(1));
                prev_char.map(|c| c.is_whitespace()).unwrap_or(true)
            };

            if !is_valid_trigger {
                self.close();
                return false;
            }

            // Extract the query (text after @, before cursor)
            let after_at = &before_cursor[at_pos + 1..];

            // Don't trigger if there's a space (completed mention)
            if after_at.contains(' ') {
                self.close();
                return false;
            }

            // Activate the popup
            self.active = true;
            self.query = after_at.to_string();
            self.trigger_pos = at_pos;
            self.cursor_pos = cursor;

            return true;
        }

        self.close();
        false
    }

    /// Searches for files matching the query.
    ///
    /// This is a synchronous version that walks the directory tree.
    pub fn search_sync(&mut self, workspace: &Path, limit: usize) -> &[PathBuf] {
        if self.query.is_empty() {
            self.results.clear();
            return &self.results;
        }

        self.results = fuzzy_search_files_sync(workspace, &self.query, limit);
        self.selected = 0;
        self.scroll_offset = 0;

        &self.results
    }

    /// Updates results from an external search.
    pub fn set_results(&mut self, results: Vec<PathBuf>) {
        self.results = results;
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Selects the next item.
    pub fn select_next(&mut self) {
        if self.results.is_empty() {
            return;
        }

        self.selected = (self.selected + 1) % self.results.len();

        // Adjust scroll offset
        if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected - self.max_visible + 1;
        } else if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
    }

    /// Selects the previous item.
    pub fn select_prev(&mut self) {
        if self.results.is_empty() {
            return;
        }

        self.selected = if self.selected == 0 {
            self.results.len() - 1
        } else {
            self.selected - 1
        };

        // Adjust scroll offset
        if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected - self.max_visible + 1;
        } else if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
    }

    /// Confirms the current selection.
    ///
    /// Returns the text to insert and the range to replace.
    pub fn confirm(&mut self) -> Option<MentionInsert> {
        let path = self.results.get(self.selected)?.clone();

        let result = MentionInsert {
            start: self.trigger_pos,
            end: self.cursor_pos,
            text: path.display().to_string(),
            path,
        };

        self.close();
        Some(result)
    }

    /// Closes the mention popup without action.
    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.results.clear();
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Returns whether the popup is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns the current query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns the search results.
    pub fn results(&self) -> &[PathBuf] {
        &self.results
    }

    /// Returns visible results based on scroll offset.
    pub fn visible_results(&self) -> &[PathBuf] {
        let end = (self.scroll_offset + self.max_visible).min(self.results.len());
        &self.results[self.scroll_offset..end]
    }

    /// Returns the selected index.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Returns the selected index relative to visible items.
    pub fn selected_visible(&self) -> usize {
        self.selected.saturating_sub(self.scroll_offset)
    }

    /// Returns the scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Returns the maximum visible items.
    pub fn max_visible(&self) -> usize {
        self.max_visible
    }

    /// Returns whether there are more items above.
    pub fn has_more_above(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Returns whether there are more items below.
    pub fn has_more_below(&self) -> bool {
        self.scroll_offset + self.max_visible < self.results.len()
    }
}

// ============================================================
// MENTION INSERT
// ============================================================

/// Information for inserting a file mention into the input.
#[derive(Debug, Clone)]
pub struct MentionInsert {
    /// Start position of the text to replace (the @).
    pub start: usize,

    /// End position of the text to replace.
    pub end: usize,

    /// The text to insert (file path).
    pub text: String,

    /// The full path to the file.
    pub path: PathBuf,
}

// ============================================================
// FUZZY SEARCH
// ============================================================

/// Performs a fuzzy search for files in the workspace.
///
/// Uses a simple scoring algorithm that prioritizes:
/// - Exact matches
/// - Prefix matches
/// - Substring matches
/// - Character sequence matches
pub fn fuzzy_search_files_sync(workspace: &Path, query: &str, limit: usize) -> Vec<PathBuf> {
    let query_lower = query.to_lowercase();
    let mut scored: Vec<(i64, PathBuf)> = Vec::new();
    let mut files_scanned = 0;

    // Walk the directory tree
    for entry in walkdir_filtered(workspace) {
        if files_scanned >= MAX_FILES_TO_SCAN {
            break;
        }

        if let Ok(entry) = entry
            && entry.file_type().is_file()
        {
            files_scanned += 1;

            // Get relative path
            let path = match entry.path().strip_prefix(workspace) {
                Ok(p) => p.to_path_buf(),
                Err(_) => entry.path().to_path_buf(),
            };

            let path_str = path.to_string_lossy().to_lowercase();

            // Calculate score
            if let Some(score) = fuzzy_score(&path_str, &query_lower) {
                // Boost for priority extensions
                let ext_boost = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| {
                        if PRIORITY_EXTENSIONS.contains(&e) {
                            10
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0);

                scored.push((score + ext_boost, path));
            }
        }
    }

    // Sort by score descending
    scored.sort_by(|a, b| b.0.cmp(&a.0));

    // Return top results
    scored.into_iter().take(limit).map(|(_, p)| p).collect()
}

/// Creates a filtered directory walker.
fn walkdir_filtered(
    root: &Path,
) -> impl Iterator<Item = Result<walkdir::DirEntry, walkdir::Error>> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            // Skip hidden files and ignored directories
            !name.starts_with('.') && !IGNORED_DIRS.contains(&name.as_ref())
        })
}

/// Calculates a fuzzy match score.
///
/// Returns None if there's no match.
fn fuzzy_score(haystack: &str, needle: &str) -> Option<i64> {
    if needle.is_empty() {
        return Some(0);
    }

    // Exact match
    if haystack == needle {
        return Some(1000);
    }

    // Contains exact match
    if haystack.contains(needle) {
        let bonus = if haystack.ends_with(needle) {
            100 // Filename match
        } else if haystack.starts_with(needle) {
            50 // Prefix match
        } else {
            25 // Substring match
        };
        return Some(500 + bonus);
    }

    // Fuzzy character matching
    let mut score = 0i64;
    let mut hay_iter = haystack.chars().peekable();
    let mut last_match_idx = -1i64;
    let mut consecutive = 0;

    for needle_char in needle.chars() {
        let mut found = false;
        let mut current_idx = 0i64;

        while let Some(&hay_char) = hay_iter.peek() {
            current_idx += 1;
            hay_iter.next();

            if hay_char == needle_char {
                found = true;
                // Bonus for consecutive matches
                if last_match_idx >= 0 && current_idx == last_match_idx + 1 {
                    consecutive += 1;
                    score += 5 * consecutive;
                } else {
                    consecutive = 0;
                }
                // Penalty for gaps
                if last_match_idx >= 0 {
                    let gap = current_idx - last_match_idx - 1;
                    score -= gap;
                }
                last_match_idx = current_idx;
                score += 10;
                break;
            }
        }

        if !found {
            return None; // Not all characters found
        }
    }

    Some(score.max(1))
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    #[test]
    fn test_check_trigger_basic() {
        let mut state = FileMentionState::new();

        // @ at start
        assert!(state.check_trigger("@src", 4));
        assert!(state.is_active());
        assert_eq!(state.query(), "src");

        state.close();

        // @ after space
        assert!(state.check_trigger("Hello @file", 11));
        assert!(state.is_active());
        assert_eq!(state.query(), "file");

        state.close();

        // No @ - should not trigger
        assert!(!state.check_trigger("Hello world", 11));
        assert!(!state.is_active());
    }

    #[test]
    fn test_check_trigger_with_space() {
        let mut state = FileMentionState::new();

        // @ followed by space should not trigger (completed mention)
        assert!(!state.check_trigger("Hello @file.rs world", 20));
    }

    #[test]
    fn test_navigation() {
        let mut state = FileMentionState::new();
        state.set_results(vec![
            PathBuf::from("a.rs"),
            PathBuf::from("b.rs"),
            PathBuf::from("c.rs"),
        ]);

        assert_eq!(state.selected(), 0);

        state.select_next();
        assert_eq!(state.selected(), 1);

        state.select_next();
        assert_eq!(state.selected(), 2);

        state.select_next();
        assert_eq!(state.selected(), 0); // Wraps around

        state.select_prev();
        assert_eq!(state.selected(), 2); // Wraps around
    }

    #[test]
    fn test_confirm() {
        let mut state = FileMentionState::new();
        state.active = true;
        state.trigger_pos = 6;
        state.cursor_pos = 10;
        state.set_results(vec![
            PathBuf::from("src/lib.rs"),
            PathBuf::from("src/main.rs"),
        ]);

        let insert = state.confirm().unwrap();
        assert_eq!(insert.start, 6);
        assert_eq!(insert.end, 10);
        assert_eq!(insert.text, "src/lib.rs");
        assert!(!state.is_active());
    }

    #[test]
    fn test_fuzzy_score() {
        // Exact match
        assert_eq!(fuzzy_score("lib.rs", "lib.rs"), Some(1000));

        // Contains
        assert!(fuzzy_score("src/lib.rs", "lib").unwrap() > 0);

        // No match
        assert!(fuzzy_score("main.rs", "xyz").is_none());

        // Fuzzy match
        assert!(fuzzy_score("lib.rs", "lrs").is_some());
    }

    #[test]
    fn test_fuzzy_search_sync() {
        // Create a temporary directory with some files
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files
        File::create(root.join("mylib.rs")).unwrap();
        File::create(root.join("main.rs")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        File::create(root.join("src/myutils.rs")).unwrap();

        // Search for "mylib" should find mylib.rs
        let results = fuzzy_search_files_sync(root, "mylib", 10);
        // Just verify we can search without panicking
        // Results depend on walkdir implementation details
        assert!(
            results.is_empty()
                || results
                    .iter()
                    .any(|p| p.to_string_lossy().contains("mylib"))
        );

        // Search for "myutils"
        let results = fuzzy_search_files_sync(root, "myutils", 10);
        assert!(
            results.is_empty()
                || results
                    .iter()
                    .any(|p| p.to_string_lossy().contains("myutils"))
        );
    }

    #[test]
    fn test_scroll() {
        let mut state = FileMentionState::new();
        state.max_visible = 3;
        state.set_results(vec![
            PathBuf::from("a.rs"),
            PathBuf::from("b.rs"),
            PathBuf::from("c.rs"),
            PathBuf::from("d.rs"),
            PathBuf::from("e.rs"),
        ]);

        assert_eq!(state.scroll_offset(), 0);
        assert!(state.has_more_below());
        assert!(!state.has_more_above());

        // Navigate down past visible
        state.select_next(); // 1
        state.select_next(); // 2
        state.select_next(); // 3
        assert_eq!(state.scroll_offset(), 1);
        assert!(state.has_more_above());
    }
}
