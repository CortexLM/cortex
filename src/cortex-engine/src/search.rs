//! Search utilities.
//!
//! Provides utilities for searching files, code,
//! and content with various matching strategies.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Search options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOptions {
    /// Case sensitive.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Whole word matching.
    #[serde(default)]
    pub whole_word: bool,
    /// Use regex.
    #[serde(default)]
    pub regex: bool,
    /// Include hidden files.
    #[serde(default)]
    pub include_hidden: bool,
    /// File patterns to include.
    #[serde(default)]
    pub include_patterns: Vec<String>,
    /// File patterns to exclude.
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
    /// Max results.
    pub max_results: Option<usize>,
    /// Context lines.
    #[serde(default)]
    pub context_lines: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            whole_word: false,
            regex: false,
            include_hidden: false,
            include_patterns: Vec::new(),
            exclude_patterns: default_exclude_patterns(),
            max_results: None,
            context_lines: 0,
        }
    }
}

/// Default exclude patterns.
fn default_exclude_patterns() -> Vec<String> {
    vec![
        "node_modules".to_string(),
        ".git".to_string(),
        "target".to_string(),
        "dist".to_string(),
        "build".to_string(),
        "__pycache__".to_string(),
        "*.pyc".to_string(),
        ".DS_Store".to_string(),
    ]
}

/// Search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// File path.
    pub path: PathBuf,
    /// Line number (1-based).
    pub line_number: usize,
    /// Column number (1-based).
    pub column: usize,
    /// Matching line content.
    pub line: String,
    /// Context before.
    pub context_before: Vec<String>,
    /// Context after.
    pub context_after: Vec<String>,
    /// Match start index in line.
    pub match_start: usize,
    /// Match end index in line.
    pub match_end: usize,
}

impl SearchResult {
    /// Get highlighted line.
    pub fn highlight(&self, start_marker: &str, end_marker: &str) -> String {
        format!(
            "{}{}{}{}{}",
            &self.line[..self.match_start],
            start_marker,
            &self.line[self.match_start..self.match_end],
            end_marker,
            &self.line[self.match_end..]
        )
    }

    /// Get match text.
    pub fn match_text(&self) -> &str {
        &self.line[self.match_start..self.match_end]
    }
}

/// Search results collection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResults {
    /// Results.
    pub results: Vec<SearchResult>,
    /// Total matches.
    pub total_matches: usize,
    /// Files searched.
    pub files_searched: usize,
    /// Search duration (ms).
    pub duration_ms: u64,
    /// Truncated (more results available).
    pub truncated: bool,
}

impl SearchResults {
    /// Create new results.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a result.
    pub fn add(&mut self, result: SearchResult) {
        self.results.push(result);
        self.total_matches += 1;
    }

    /// Get results grouped by file.
    pub fn by_file(&self) -> HashMap<PathBuf, Vec<&SearchResult>> {
        let mut grouped: HashMap<PathBuf, Vec<&SearchResult>> = HashMap::new();

        for result in &self.results {
            grouped.entry(result.path.clone()).or_default().push(result);
        }

        grouped
    }

    /// Get file count.
    pub fn file_count(&self) -> usize {
        self.results
            .iter()
            .map(|r| &r.path)
            .collect::<std::collections::HashSet<_>>()
            .len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
    }

    /// Get count.
    pub fn len(&self) -> usize {
        self.results.len()
    }
}

/// Text searcher.
pub struct TextSearcher {
    options: SearchOptions,
}

impl TextSearcher {
    /// Create a new searcher.
    pub fn new(options: SearchOptions) -> Self {
        Self { options }
    }

    /// Search in text.
    pub fn search_text(&self, text: &str, pattern: &str) -> Vec<TextMatch> {
        let lines: Vec<&str> = text.lines().collect();
        let mut matches = Vec::new();

        let pattern_lower = if self.options.case_sensitive {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };

        for (line_idx, line) in lines.iter().enumerate() {
            let search_line = if self.options.case_sensitive {
                line.to_string()
            } else {
                line.to_lowercase()
            };

            let mut start = 0;
            while let Some(pos) = search_line[start..].find(&pattern_lower) {
                let match_start = start + pos;
                let match_end = match_start + pattern.len();

                // Check whole word
                if self.options.whole_word {
                    let before_ok = match_start == 0
                        || !search_line
                            .chars()
                            .nth(match_start - 1)
                            .map(char::is_alphanumeric)
                            .unwrap_or(false);
                    let after_ok = match_end >= search_line.len()
                        || !search_line
                            .chars()
                            .nth(match_end)
                            .map(char::is_alphanumeric)
                            .unwrap_or(false);

                    if !before_ok || !after_ok {
                        start = match_start + 1;
                        continue;
                    }
                }

                // Get context
                let context_before: Vec<String> = (0..self.options.context_lines)
                    .filter_map(|i| {
                        if line_idx > i {
                            Some(lines[line_idx - i - 1].to_string())
                        } else {
                            None
                        }
                    })
                    .rev()
                    .collect();

                let context_after: Vec<String> = (0..self.options.context_lines)
                    .filter_map(|i| {
                        lines
                            .get(line_idx + i + 1)
                            .map(std::string::ToString::to_string)
                    })
                    .collect();

                matches.push(TextMatch {
                    line_number: line_idx + 1,
                    column: match_start + 1,
                    line: line.to_string(),
                    match_start,
                    match_end,
                    context_before,
                    context_after,
                });

                start = match_start + 1;
            }

            if let Some(max) = self.options.max_results
                && matches.len() >= max
            {
                break;
            }
        }

        matches
    }
}

impl Default for TextSearcher {
    fn default() -> Self {
        Self::new(SearchOptions::default())
    }
}

/// Text match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMatch {
    /// Line number (1-based).
    pub line_number: usize,
    /// Column number (1-based).
    pub column: usize,
    /// Line content.
    pub line: String,
    /// Match start in line.
    pub match_start: usize,
    /// Match end in line.
    pub match_end: usize,
    /// Context before.
    pub context_before: Vec<String>,
    /// Context after.
    pub context_after: Vec<String>,
}

impl TextMatch {
    /// Get match text.
    pub fn match_text(&self) -> &str {
        &self.line[self.match_start..self.match_end]
    }
}

/// File search.
pub struct FileSearcher {
    options: SearchOptions,
}

impl FileSearcher {
    /// Create a new searcher.
    pub fn new(options: SearchOptions) -> Self {
        Self { options }
    }

    /// Check if path should be excluded.
    fn should_exclude(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        // Check hidden files
        if !self.options.include_hidden
            && let Some(name) = path.file_name()
            && name.to_string_lossy().starts_with('.')
        {
            return true;
        }

        // Check exclude patterns
        for pattern in &self.options.exclude_patterns {
            if path_str.contains(pattern) {
                return true;
            }
        }

        false
    }

    /// Check if path matches include patterns.
    fn matches_include(&self, path: &Path) -> bool {
        if self.options.include_patterns.is_empty() {
            return true;
        }

        let path_str = path.to_string_lossy();

        for pattern in &self.options.include_patterns {
            if path_str.contains(pattern) {
                return true;
            }
            // Check extension
            if let Some(ext) = pattern.strip_prefix("*.")
                && path
                    .extension()
                    .map(|e| e.to_str() == Some(ext))
                    .unwrap_or(false)
            {
                return true;
            }
        }

        false
    }

    /// Find files matching pattern.
    pub async fn find_files(&self, dir: &Path, name_pattern: &str) -> Result<Vec<PathBuf>> {
        let mut results = Vec::new();
        self.find_files_recursive(dir, name_pattern, &mut results)
            .await?;
        Ok(results)
    }

    /// Recursive file finder.
    async fn find_files_recursive(
        &self,
        dir: &Path,
        pattern: &str,
        results: &mut Vec<PathBuf>,
    ) -> Result<()> {
        let mut entries = tokio::fs::read_dir(dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if self.should_exclude(&path) {
                continue;
            }

            if path.is_dir() {
                Box::pin(self.find_files_recursive(&path, pattern, results)).await?;
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let name_lower = name.to_lowercase();
                let pattern_lower = pattern.to_lowercase();

                if name_lower.contains(&pattern_lower) && self.matches_include(&path) {
                    results.push(path);

                    if let Some(max) = self.options.max_results
                        && results.len() >= max
                    {
                        return Ok(());
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for FileSearcher {
    fn default() -> Self {
        Self::new(SearchOptions::default())
    }
}

/// Symbol kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    /// Function.
    Function,
    /// Method.
    Method,
    /// Class.
    Class,
    /// Struct.
    Struct,
    /// Enum.
    Enum,
    /// Interface.
    Interface,
    /// Trait.
    Trait,
    /// Variable.
    Variable,
    /// Constant.
    Constant,
    /// Module.
    Module,
    /// Type.
    Type,
}

impl SymbolKind {
    /// Get icon.
    pub fn icon(&self) -> char {
        match self {
            Self::Function | Self::Method => 'ƒ',
            Self::Class => 'C',
            Self::Struct => 'S',
            Self::Enum => 'E',
            Self::Interface | Self::Trait => 'I',
            Self::Variable => 'v',
            Self::Constant => 'c',
            Self::Module => 'M',
            Self::Type => 'T',
        }
    }
}

/// Symbol definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: SymbolKind,
    /// File path.
    pub path: PathBuf,
    /// Line number.
    pub line: usize,
    /// Column.
    pub column: usize,
    /// Container (parent symbol).
    pub container: Option<String>,
    /// Documentation.
    pub doc: Option<String>,
}

impl Symbol {
    /// Get full name.
    pub fn full_name(&self) -> String {
        if let Some(ref container) = self.container {
            format!("{}::{}", container, self.name)
        } else {
            self.name.clone()
        }
    }
}

/// Symbol index.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SymbolIndex {
    /// Symbols.
    pub symbols: Vec<Symbol>,
    /// By name.
    by_name: HashMap<String, Vec<usize>>,
    /// By kind.
    by_kind: HashMap<SymbolKind, Vec<usize>>,
}

impl SymbolIndex {
    /// Create a new index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a symbol.
    pub fn add(&mut self, symbol: Symbol) {
        let idx = self.symbols.len();

        self.by_name
            .entry(symbol.name.clone())
            .or_default()
            .push(idx);

        self.by_kind.entry(symbol.kind).or_default().push(idx);

        self.symbols.push(symbol);
    }

    /// Find by name.
    pub fn find_by_name(&self, name: &str) -> Vec<&Symbol> {
        self.by_name
            .get(name)
            .map(|indices| indices.iter().map(|&i| &self.symbols[i]).collect())
            .unwrap_or_default()
    }

    /// Find by kind.
    pub fn find_by_kind(&self, kind: SymbolKind) -> Vec<&Symbol> {
        self.by_kind
            .get(&kind)
            .map(|indices| indices.iter().map(|&i| &self.symbols[i]).collect())
            .unwrap_or_default()
    }

    /// Search symbols.
    pub fn search(&self, query: &str) -> Vec<&Symbol> {
        let query_lower = query.to_lowercase();

        self.symbols
            .iter()
            .filter(|s| s.name.to_lowercase().contains(&query_lower))
            .collect()
    }

    /// Get count.
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_options_default() {
        let opts = SearchOptions::default();
        assert!(!opts.case_sensitive);
        assert!(!opts.exclude_patterns.is_empty());
    }

    #[test]
    fn test_text_search() {
        let searcher = TextSearcher::default();
        let text = "Hello world\nHello there\nGoodbye world";

        let matches = searcher.search_text(text, "hello");
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_text_search_case_sensitive() {
        let searcher = TextSearcher::new(SearchOptions {
            case_sensitive: true,
            ..Default::default()
        });
        let text = "Hello world\nhello there";

        let matches = searcher.search_text(text, "Hello");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_text_search_whole_word() {
        let searcher = TextSearcher::new(SearchOptions {
            whole_word: true,
            ..Default::default()
        });
        let text = "hello helloworld worldhello";

        let matches = searcher.search_text(text, "hello");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_search_result_highlight() {
        let result = SearchResult {
            path: PathBuf::from("test.txt"),
            line_number: 1,
            column: 7,
            line: "Hello world!".to_string(),
            context_before: vec![],
            context_after: vec![],
            match_start: 6,
            match_end: 11,
        };

        let highlighted = result.highlight("[", "]");
        assert_eq!(highlighted, "Hello [world]!");
    }

    #[test]
    fn test_search_results_by_file() {
        let mut results = SearchResults::new();

        results.add(SearchResult {
            path: PathBuf::from("a.txt"),
            line_number: 1,
            column: 1,
            line: "line".to_string(),
            context_before: vec![],
            context_after: vec![],
            match_start: 0,
            match_end: 4,
        });

        results.add(SearchResult {
            path: PathBuf::from("b.txt"),
            line_number: 1,
            column: 1,
            line: "line".to_string(),
            context_before: vec![],
            context_after: vec![],
            match_start: 0,
            match_end: 4,
        });

        let by_file = results.by_file();
        assert_eq!(by_file.len(), 2);
    }

    #[test]
    fn test_symbol_index() {
        let mut index = SymbolIndex::new();

        index.add(Symbol {
            name: "main".to_string(),
            kind: SymbolKind::Function,
            path: PathBuf::from("main.rs"),
            line: 1,
            column: 1,
            container: None,
            doc: None,
        });

        index.add(Symbol {
            name: "MyClass".to_string(),
            kind: SymbolKind::Class,
            path: PathBuf::from("lib.rs"),
            line: 10,
            column: 1,
            container: None,
            doc: None,
        });

        assert_eq!(index.len(), 2);
        assert_eq!(index.find_by_name("main").len(), 1);
        assert_eq!(index.find_by_kind(SymbolKind::Function).len(), 1);
    }

    #[test]
    fn test_symbol_full_name() {
        let symbol = Symbol {
            name: "method".to_string(),
            kind: SymbolKind::Method,
            path: PathBuf::from("lib.rs"),
            line: 10,
            column: 1,
            container: Some("MyClass".to_string()),
            doc: None,
        };

        assert_eq!(symbol.full_name(), "MyClass::method");
    }
}
