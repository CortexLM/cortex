//! Main file search implementation.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ignore::WalkBuilder;
use tokio::sync::RwLock;

use crate::cache::{FileCache, get_mtime};
use crate::config::SearchConfig;
use crate::error::{SearchError, SearchResult};
use crate::index::{FileIndex, IndexedFile};
use crate::matcher::{FuzzyMatcher, glob_match};
use crate::result::{SearchMatch, SearchMode, SearchStats};

/// File search engine with fuzzy matching, caching, and incremental updates.
///
/// # Example
///
/// ```no_run
/// use cortex_file_search::{FileSearch, SearchMode};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let mut search = FileSearch::new("/path/to/project");
///     search.build_index().await?;
///     
///     let results = search.search("main.rs", SearchMode::FileName, 10).await?;
///     for result in results {
///         println!("{}: {}", result.score, result.path.display());
///     }
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct FileSearch {
    /// Configuration for the search.
    config: SearchConfig,

    /// File index.
    index: Arc<RwLock<FileIndex>>,

    /// File cache.
    cache: Arc<RwLock<FileCache>>,

    /// Fuzzy matcher.
    matcher: Arc<RwLock<FuzzyMatcher>>,
}

impl FileSearch {
    /// Creates a new file search with default configuration.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let config = SearchConfig::new(root);
        Self::with_config(config)
    }

    /// Creates a new file search with the specified configuration.
    pub fn with_config(config: SearchConfig) -> Self {
        let cache_config = config.cache_config.clone();
        let root = config.root.clone();

        Self {
            config,
            index: Arc::new(RwLock::new(FileIndex::new(root))),
            cache: Arc::new(RwLock::new(FileCache::new(cache_config))),
            matcher: Arc::new(RwLock::new(FuzzyMatcher::new())),
        }
    }

    /// Returns the root directory being searched.
    pub fn root(&self) -> &Path {
        &self.config.root
    }

    /// Returns the current configuration.
    pub fn config(&self) -> &SearchConfig {
        &self.config
    }

    /// Updates the configuration.
    ///
    /// Note: This does not automatically rebuild the index.
    pub fn set_config(&mut self, config: SearchConfig) {
        self.config = config;
    }

    /// Builds or rebuilds the file index.
    ///
    /// This walks the file system and indexes all files matching the
    /// configuration criteria.
    pub async fn build_index(&self) -> SearchResult<()> {
        // Validate root directory
        let root = &self.config.root;
        if !root.exists() {
            return Err(SearchError::root_not_found(root));
        }
        if !root.is_dir() {
            return Err(SearchError::not_a_directory(root));
        }

        // Mark as building
        {
            let mut index = self.index.write().await;
            if index.is_building() {
                return Err(SearchError::IndexBuilding);
            }
            index.set_building(true);
            index.clear();
        }

        // Build the walker
        let mut builder = WalkBuilder::new(root);

        // Configure walker
        builder
            .hidden(!self.config.include_hidden)
            .follow_links(self.config.follow_symlinks)
            .git_ignore(self.config.respect_gitignore)
            .git_global(self.config.respect_gitignore)
            .git_exclude(self.config.respect_gitignore);

        if let Some(depth) = self.config.max_depth {
            builder.max_depth(Some(depth));
        }

        // Add custom ignore patterns
        for pattern in &self.config.ignore_patterns {
            let mut override_builder = ignore::overrides::OverrideBuilder::new(root);
            if let Err(e) = override_builder.add(&format!("!{pattern}")) {
                // Log but continue - pattern might be invalid
                tracing::warn!("Invalid ignore pattern '{}': {}", pattern, e);
            }
        }

        // Walk the file system
        let walker = builder.build();
        let mut files_to_add = Vec::new();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!("Error walking directory: {}", e);
                    continue;
                }
            };

            // Skip directories
            let file_type = match entry.file_type() {
                Some(ft) => ft,
                None => continue,
            };

            if file_type.is_dir() {
                // Check if directory should be excluded
                if let Some(name) = entry.file_name().to_str()
                    && self.config.should_exclude_dir(name)
                {
                    continue;
                }
                continue;
            }

            // Skip non-files (symlinks if not following, etc.)
            if !file_type.is_file() {
                continue;
            }

            let path = entry.path();

            // Check extension filter
            if let Some(ext) = path.extension().and_then(|e| e.to_str())
                && !self.config.should_include_extension(ext)
            {
                continue;
            }

            // Get file metadata
            let (size, mtime) = match path.metadata() {
                Ok(meta) => {
                    let size = meta.len();
                    let mtime = get_mtime(path);
                    (size, mtime)
                }
                Err(_) => (0, None),
            };

            // Create relative path
            let relative_path = match path.strip_prefix(root) {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };

            let indexed_file = IndexedFile::new(relative_path, path.to_path_buf(), size, mtime);

            files_to_add.push(indexed_file);
        }

        // Add all files to index
        {
            let mut index = self.index.write().await;
            for file in files_to_add {
                index.add_file(file);
            }
            index.mark_built();
        }

        Ok(())
    }

    /// Performs incremental update of the index.
    ///
    /// This only re-scans directories that have been marked as dirty.
    pub async fn incremental_update(&self) -> SearchResult<()> {
        let dirty_dirs: Vec<PathBuf>;

        {
            let index = self.index.read().await;
            if !index.is_built() {
                return Err(SearchError::IndexNotBuilt);
            }
            dirty_dirs = index.dirty_dirs().iter().cloned().collect();
        }

        if dirty_dirs.is_empty() {
            return Ok(());
        }

        for dir in dirty_dirs {
            self.update_directory(&dir).await?;
        }

        Ok(())
    }

    /// Updates a single directory in the index.
    async fn update_directory(&self, dir: &Path) -> SearchResult<()> {
        let full_path = self.config.root.join(dir);

        if !full_path.exists() {
            // Directory was deleted - remove from index
            let mut index = self.index.write().await;
            index.remove_files_in_directory(dir);
            return Ok(());
        }

        // Remove old entries for this directory
        {
            let mut index = self.index.write().await;
            index.remove_files_in_directory(dir);
        }

        // Re-scan directory
        let mut builder = WalkBuilder::new(&full_path);
        builder
            .max_depth(Some(1)) // Only this directory
            .hidden(!self.config.include_hidden)
            .git_ignore(self.config.respect_gitignore);

        let walker = builder.build();
        let mut files_to_add = Vec::new();

        for entry in walker.skip(1) {
            // Skip the directory itself
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            match entry.file_type() {
                Some(ft) if ft.is_file() => {}
                _ => continue,
            }

            let path = entry.path();

            // Check extension filter
            if let Some(ext) = path.extension().and_then(|e| e.to_str())
                && !self.config.should_include_extension(ext)
            {
                continue;
            }

            let (size, mtime) = match path.metadata() {
                Ok(meta) => (meta.len(), get_mtime(path)),
                Err(_) => (0, None),
            };

            let relative_path = match path.strip_prefix(&self.config.root) {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };

            files_to_add.push(IndexedFile::new(
                relative_path,
                path.to_path_buf(),
                size,
                mtime,
            ));
        }

        // Add files to index
        {
            let mut index = self.index.write().await;
            for file in files_to_add {
                index.add_file(file);
            }
        }

        Ok(())
    }

    /// Marks a directory as needing re-indexing.
    pub async fn mark_dirty(&self, path: &Path) {
        let mut index = self.index.write().await;
        index.mark_dirty(path);
    }

    /// Searches for files matching the query.
    ///
    /// # Arguments
    ///
    /// * `query` - The search query (fuzzy match)
    /// * `mode` - What to match against (file name, path, content)
    /// * `limit` - Maximum number of results to return
    ///
    /// # Returns
    ///
    /// A vector of search matches, sorted by score (highest first).
    pub async fn search(
        &self,
        query: &str,
        mode: SearchMode,
        limit: usize,
    ) -> SearchResult<Vec<SearchMatch>> {
        if query.is_empty() {
            return Err(SearchError::EmptyQuery);
        }

        let _start = Instant::now();

        // Check if index is built
        {
            let index = self.index.read().await;
            if !index.is_built() {
                return Err(SearchError::IndexNotBuilt);
            }
        }

        let results = match mode {
            SearchMode::FileName => self.search_file_names(query, limit).await?,
            SearchMode::FullPath => self.search_paths(query, limit).await?,
            SearchMode::Content => self.search_content(query, limit).await?,
            SearchMode::FileNameAndPath => {
                // Combine file name and path results
                let mut results = self.search_file_names(query, limit * 2).await?;
                let path_results = self.search_paths(query, limit * 2).await?;

                // Merge and deduplicate
                for pr in path_results {
                    if !results.iter().any(|r| r.path == pr.path) {
                        results.push(pr);
                    }
                }

                results.sort();
                results.truncate(limit);
                results
            }
        };

        Ok(results)
    }

    /// Searches by file name using fuzzy matching.
    async fn search_file_names(&self, query: &str, limit: usize) -> SearchResult<Vec<SearchMatch>> {
        let index = self.index.read().await;
        let files = index.files();
        let file_names = index.file_names();

        let mut matcher = self.matcher.write().await;

        // Score all file names
        let haystacks: Vec<&str> = file_names.iter().map(String::as_str).collect();
        let scored = matcher.batch_score(query, haystacks);

        // Convert to SearchMatch
        let mut results: Vec<SearchMatch> = scored
            .into_iter()
            .take(limit)
            .filter_map(|(idx, score)| {
                files.get(idx).map(|file| {
                    SearchMatch::new(
                        file.relative_path.clone(),
                        file.absolute_path.clone(),
                        file.file_name.clone(),
                        score,
                        SearchMode::FileName,
                    )
                    .with_file_size(file.size)
                })
            })
            .collect();

        // Get match indices for highlighting
        for result in &mut results {
            if let Some((_, indices)) = matcher.score_with_indices(query, &result.file_name) {
                result.match_indices = indices;
            }
        }

        Ok(results)
    }

    /// Searches by full path using fuzzy matching.
    async fn search_paths(&self, query: &str, limit: usize) -> SearchResult<Vec<SearchMatch>> {
        let index = self.index.read().await;
        let files = index.files();
        let paths = index.relative_paths();

        let mut matcher = self.matcher.write().await;

        // Score all paths
        let haystacks: Vec<&str> = paths.iter().map(String::as_str).collect();
        let scored = matcher.batch_score(query, haystacks);

        // Convert to SearchMatch
        let results: Vec<SearchMatch> = scored
            .into_iter()
            .take(limit)
            .filter_map(|(idx, score)| {
                files.get(idx).map(|file| {
                    SearchMatch::new(
                        file.relative_path.clone(),
                        file.absolute_path.clone(),
                        file.file_name.clone(),
                        score,
                        SearchMode::FullPath,
                    )
                    .with_file_size(file.size)
                })
            })
            .collect();

        Ok(results)
    }

    /// Searches file contents (basic implementation).
    async fn search_content(&self, query: &str, limit: usize) -> SearchResult<Vec<SearchMatch>> {
        if !self.config.index_contents {
            return Ok(Vec::new());
        }

        let index = self.index.read().await;
        let files = index.files();

        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        for file in files {
            // Skip files that are too large
            if file.size > self.config.max_file_size {
                continue;
            }

            // Try to read and search file contents
            if let Ok(content_matches) = self.search_file_content(file, &query_lower).await {
                results.extend(content_matches);

                if results.len() >= limit * 2 {
                    break;
                }
            }
        }

        // Sort by score and truncate
        results.sort();
        results.truncate(limit);

        Ok(results)
    }

    /// Searches content of a single file.
    async fn search_file_content(
        &self,
        file: &IndexedFile,
        query: &str,
    ) -> SearchResult<Vec<SearchMatch>> {
        let path = &file.absolute_path;

        // Open file and search line by line
        let file_handle = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => return Err(SearchError::read_file(path, e)),
        };

        let reader = BufReader::new(file_handle);
        let mut results = Vec::new();
        let mut matcher = self.matcher.write().await;

        for (line_num, line_result) in reader.lines().enumerate() {
            let line = match line_result {
                Ok(l) => l,
                Err(_) => continue, // Skip lines that can't be read (binary files, etc.)
            };

            // Quick check: does line contain query as substring?
            if let Some(score) = matcher.score(query, &line) {
                results.push(SearchMatch::content_match(
                    file.relative_path.clone(),
                    file.absolute_path.clone(),
                    file.file_name.clone(),
                    score,
                    line_num + 1,
                    line.trim().to_string(),
                ));

                // Limit matches per file
                if results.len() >= 10 {
                    break;
                }
            }
        }

        Ok(results)
    }

    /// Searches using a glob pattern.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cortex_file_search::FileSearch;
    /// # async fn example() -> anyhow::Result<()> {
    /// let search = FileSearch::new("/project");
    /// let matches = search.glob("**/*.rs", 100).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn glob(&self, pattern: &str, limit: usize) -> SearchResult<Vec<SearchMatch>> {
        let index = self.index.read().await;

        if !index.is_built() {
            return Err(SearchError::IndexNotBuilt);
        }

        let files = index.files();
        let mut results = Vec::new();

        for file in files {
            let path_str = file.relative_path.to_string_lossy();

            // Convert Windows paths to forward slashes for consistent matching
            let normalized_path = path_str.replace('\\', "/");

            if glob_match(pattern, &normalized_path) {
                results.push(SearchMatch::new(
                    file.relative_path.clone(),
                    file.absolute_path.clone(),
                    file.file_name.clone(),
                    100, // Glob matches get full score
                    SearchMode::FullPath,
                ));

                if results.len() >= limit {
                    break;
                }
            }
        }

        Ok(results)
    }

    /// Returns statistics about the current index.
    pub async fn stats(&self) -> SearchStats {
        let index = self.index.read().await;
        let cache = self.cache.read().await;

        SearchStats {
            files_indexed: index.len(),
            files_searched: 0,
            matches_found: 0,
            search_time_ms: 0,
            from_cache: cache.is_enabled(),
        }
    }

    /// Clears the file cache.
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Returns whether the index has been built.
    pub async fn is_indexed(&self) -> bool {
        let index = self.index.read().await;
        index.is_built()
    }

    /// Returns the number of indexed files.
    pub async fn file_count(&self) -> usize {
        let index = self.index.read().await;
        index.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    async fn setup_test_dir() -> (TempDir, FileSearch) {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test file structure
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();

        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/lib.rs"), "pub mod utils;").unwrap();
        fs::write(root.join("src/utils.rs"), "pub fn helper() {}").unwrap();
        fs::write(root.join("tests/test_main.rs"), "#[test] fn test() {}").unwrap();
        fs::write(root.join("docs/README.md"), "# Documentation").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();

        let search = FileSearch::new(root);

        (temp_dir, search)
    }

    #[tokio::test]
    async fn test_build_index() {
        let (_temp_dir, search) = setup_test_dir().await;

        assert!(!search.is_indexed().await);

        search.build_index().await.unwrap();

        assert!(search.is_indexed().await);
        assert!(search.file_count().await >= 6);
    }

    #[tokio::test]
    async fn test_search_file_name() {
        let (_temp_dir, search) = setup_test_dir().await;
        search.build_index().await.unwrap();

        let results = search
            .search("main", SearchMode::FileName, 10)
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.file_name.contains("main")));
    }

    #[tokio::test]
    async fn test_search_path() {
        let (_temp_dir, search) = setup_test_dir().await;
        search.build_index().await.unwrap();

        let results = search
            .search("src/lib", SearchMode::FullPath, 10)
            .await
            .unwrap();

        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_glob_search() {
        let (_temp_dir, search) = setup_test_dir().await;
        search.build_index().await.unwrap();

        let results = search.glob("**/*.rs", 100).await.unwrap();

        assert!(results.len() >= 4); // main.rs, lib.rs, utils.rs, test_main.rs
        assert!(results.iter().all(|r| r.file_name.ends_with(".rs")));
    }

    #[tokio::test]
    async fn test_glob_src_only() {
        let (_temp_dir, search) = setup_test_dir().await;
        search.build_index().await.unwrap();

        let results = search.glob("src/*.rs", 100).await.unwrap();

        assert!(results.len() >= 3);
        for result in &results {
            let path_str = result.path.to_string_lossy();
            assert!(path_str.starts_with("src"));
        }
    }

    #[tokio::test]
    async fn test_empty_query() {
        let (_temp_dir, search) = setup_test_dir().await;
        search.build_index().await.unwrap();

        let result = search.search("", SearchMode::FileName, 10).await;
        assert!(matches!(result, Err(SearchError::EmptyQuery)));
    }

    #[tokio::test]
    async fn test_search_before_index() {
        let (_temp_dir, search) = setup_test_dir().await;

        let result = search.search("main", SearchMode::FileName, 10).await;
        assert!(matches!(result, Err(SearchError::IndexNotBuilt)));
    }

    #[tokio::test]
    async fn test_fuzzy_match() {
        let (_temp_dir, search) = setup_test_dir().await;
        search.build_index().await.unwrap();

        // "mn" should fuzzy match "main"
        let results = search.search("mn", SearchMode::FileName, 10).await.unwrap();

        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_results_sorted_by_score() {
        let (_temp_dir, search) = setup_test_dir().await;
        search.build_index().await.unwrap();

        let results = search.search("rs", SearchMode::FileName, 10).await.unwrap();

        // Results should be sorted by score (descending)
        for i in 1..results.len() {
            assert!(results[i - 1].score >= results[i].score);
        }
    }
}
