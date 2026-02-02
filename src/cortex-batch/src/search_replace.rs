//! Search and replace across multiple files.

use crate::Result;
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info};

/// Search pattern configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPattern {
    /// Pattern to search for.
    pub pattern: String,
    /// Whether pattern is regex.
    #[serde(default)]
    pub is_regex: bool,
    /// Case sensitive search.
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    /// Whole word only.
    #[serde(default)]
    pub whole_word: bool,
}

fn default_true() -> bool {
    true
}

impl SearchPattern {
    pub fn literal(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            is_regex: false,
            case_sensitive: true,
            whole_word: false,
        }
    }

    pub fn regex(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            is_regex: true,
            case_sensitive: true,
            whole_word: false,
        }
    }

    pub fn case_insensitive(mut self) -> Self {
        self.case_sensitive = false;
        self
    }

    pub fn whole_word(mut self) -> Self {
        self.whole_word = true;
        self
    }
}

/// A match found in a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Match {
    pub file_path: PathBuf,
    pub line_number: usize,
    pub column: usize,
    pub line_content: String,
    pub match_text: String,
}

/// Result of search operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub pattern: String,
    pub files_searched: usize,
    pub files_matched: usize,
    pub total_matches: usize,
    pub matches: Vec<Match>,
}

impl SearchResult {
    pub fn new(pattern: String) -> Self {
        Self {
            pattern,
            files_searched: 0,
            files_matched: 0,
            total_matches: 0,
            matches: Vec::new(),
        }
    }
}

/// Result of replace operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceResult {
    pub pattern: String,
    pub replacement: String,
    pub files_modified: usize,
    pub total_replacements: usize,
    pub modified_files: Vec<PathBuf>,
}

impl ReplaceResult {
    pub fn new(pattern: String, replacement: String) -> Self {
        Self {
            pattern,
            replacement,
            files_modified: 0,
            total_replacements: 0,
            modified_files: Vec::new(),
        }
    }
}

/// Search and replace engine.
pub struct SearchReplace {
    /// File patterns to include.
    include_patterns: Vec<String>,
    /// File patterns to exclude.
    exclude_patterns: Vec<String>,
    /// Dry run mode.
    dry_run: bool,
}

impl SearchReplace {
    pub fn new() -> Self {
        Self {
            include_patterns: vec!["*".to_string()],
            exclude_patterns: Vec::new(),
            dry_run: false,
        }
    }

    pub fn include(mut self, pattern: impl Into<String>) -> Self {
        self.include_patterns.push(pattern.into());
        self
    }

    pub fn exclude(mut self, pattern: impl Into<String>) -> Self {
        self.exclude_patterns.push(pattern.into());
        self
    }

    pub fn dry_run(mut self, enabled: bool) -> Self {
        self.dry_run = enabled;
        self
    }

    /// Search for a pattern in files.
    pub async fn search(&self, root: &Path, pattern: &SearchPattern) -> Result<SearchResult> {
        let mut result = SearchResult::new(pattern.pattern.clone());
        let files = self.collect_files(root).await?;

        for file in files {
            result.files_searched += 1;

            let content = match fs::read_to_string(&file).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            let matches = self.find_matches(&content, pattern, &file);
            if !matches.is_empty() {
                result.files_matched += 1;
                result.total_matches += matches.len();
                result.matches.extend(matches);
            }
        }

        info!(
            "Search complete: {} files searched, {} files matched, {} total matches",
            result.files_searched, result.files_matched, result.total_matches
        );

        Ok(result)
    }

    /// Replace pattern in files.
    pub async fn replace(
        &self,
        root: &Path,
        pattern: &SearchPattern,
        replacement: &str,
    ) -> Result<ReplaceResult> {
        let mut result = ReplaceResult::new(pattern.pattern.clone(), replacement.to_string());
        let files = self.collect_files(root).await?;

        for file in files {
            let content = match fs::read_to_string(&file).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            let (new_content, count) = self.replace_in_content(&content, pattern, replacement);

            if count > 0 {
                result.files_modified += 1;
                result.total_replacements += count;
                result.modified_files.push(file.clone());

                if !self.dry_run {
                    fs::write(&file, new_content).await?;
                    debug!("Modified {}: {} replacements", file.display(), count);
                }
            }
        }

        info!(
            "Replace complete: {} files modified, {} total replacements",
            result.files_modified, result.total_replacements
        );

        Ok(result)
    }

    /// Collect files matching patterns.
    async fn collect_files(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        self.collect_files_recursive(root, &mut files).await?;
        Ok(files)
    }

    async fn collect_files_recursive(&self, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        let mut entries = fs::read_dir(dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            // Skip hidden files and common ignore patterns
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }

            if path.is_dir() {
                Box::pin(self.collect_files_recursive(&path, files)).await?;
            } else if self.matches_patterns(&name) {
                files.push(path);
            }
        }

        Ok(())
    }

    fn matches_patterns(&self, name: &str) -> bool {
        // Check exclude patterns first
        for pattern in &self.exclude_patterns {
            if Pattern::new(pattern)
                .map(|p| p.matches(name))
                .unwrap_or(false)
            {
                return false;
            }
        }

        // Check include patterns
        for pattern in &self.include_patterns {
            if Pattern::new(pattern)
                .map(|p| p.matches(name))
                .unwrap_or(false)
            {
                return true;
            }
        }

        false
    }

    fn find_matches(&self, content: &str, pattern: &SearchPattern, file_path: &Path) -> Vec<Match> {
        let mut matches = Vec::new();
        let search_pattern = if pattern.case_sensitive {
            pattern.pattern.clone()
        } else {
            pattern.pattern.to_lowercase()
        };

        for (line_num, line) in content.lines().enumerate() {
            let search_line = if pattern.case_sensitive {
                line.to_string()
            } else {
                line.to_lowercase()
            };

            let mut start = 0;
            while let Some(pos) = search_line[start..].find(&search_pattern) {
                let actual_pos = start + pos;

                // Check whole word if required
                if pattern.whole_word {
                    let before_ok = actual_pos == 0
                        || !search_line
                            .chars()
                            .nth(actual_pos - 1)
                            .map(|c| c.is_alphanumeric())
                            .unwrap_or(false);
                    let after_pos = actual_pos + search_pattern.len();
                    let after_ok = after_pos >= search_line.len()
                        || !search_line
                            .chars()
                            .nth(after_pos)
                            .map(|c| c.is_alphanumeric())
                            .unwrap_or(false);

                    if !before_ok || !after_ok {
                        start = actual_pos + 1;
                        continue;
                    }
                }

                matches.push(Match {
                    file_path: file_path.to_path_buf(),
                    line_number: line_num + 1,
                    column: actual_pos + 1,
                    line_content: line.to_string(),
                    match_text: line[actual_pos..actual_pos + pattern.pattern.len()].to_string(),
                });

                start = actual_pos + 1;
            }
        }

        matches
    }

    fn replace_in_content(
        &self,
        content: &str,
        pattern: &SearchPattern,
        replacement: &str,
    ) -> (String, usize) {
        if pattern.case_sensitive {
            let count = content.matches(&pattern.pattern).count();
            (content.replace(&pattern.pattern, replacement), count)
        } else {
            // Case insensitive replacement
            let mut result = content.to_string();
            let mut count = 0;
            let lower_content = content.to_lowercase();
            let lower_pattern = pattern.pattern.to_lowercase();

            let mut offset: i64 = 0;
            let mut search_start = 0;

            while let Some(pos) = lower_content[search_start..].find(&lower_pattern) {
                let actual_pos = search_start + pos;
                let adjusted_pos = (actual_pos as i64 + offset) as usize;

                result = format!(
                    "{}{}{}",
                    &result[..adjusted_pos],
                    replacement,
                    &result[adjusted_pos + pattern.pattern.len()..]
                );

                offset += replacement.len() as i64 - pattern.pattern.len() as i64;
                count += 1;
                search_start = actual_pos + pattern.pattern.len();
            }

            (result, count)
        }
    }
}

impl Default for SearchReplace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_search() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "Hello World\nHello Rust\nGoodbye World")
            .await
            .unwrap();

        let searcher = SearchReplace::new().include("*.txt");
        let pattern = SearchPattern::literal("Hello");
        let result = searcher.search(dir.path(), &pattern).await.unwrap();

        assert_eq!(result.files_matched, 1);
        assert_eq!(result.total_matches, 2);
    }

    #[tokio::test]
    async fn test_replace() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "foo bar foo").await.unwrap();

        let searcher = SearchReplace::new().include("*.txt");
        let pattern = SearchPattern::literal("foo");
        let result = searcher.replace(dir.path(), &pattern, "baz").await.unwrap();

        assert_eq!(result.total_replacements, 2);
        assert_eq!(fs::read_to_string(&file).await.unwrap(), "baz bar baz");
    }
}
