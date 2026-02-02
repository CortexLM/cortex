//! Configuration types for file search.

use std::path::PathBuf;

/// Configuration for the file search system.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Root directory to search in.
    pub root: PathBuf,

    /// Maximum depth to traverse into subdirectories.
    /// `None` means unlimited depth.
    pub max_depth: Option<usize>,

    /// Whether to follow symbolic links.
    pub follow_symlinks: bool,

    /// Whether to respect .gitignore files.
    pub respect_gitignore: bool,

    /// Whether to include hidden files (starting with .).
    pub include_hidden: bool,

    /// Custom ignore patterns (gitignore-style).
    pub ignore_patterns: Vec<String>,

    /// File extensions to include (empty means all).
    pub include_extensions: Vec<String>,

    /// File extensions to exclude.
    pub exclude_extensions: Vec<String>,

    /// Directories to exclude by name.
    pub exclude_dirs: Vec<String>,

    /// Maximum file size to index (in bytes).
    /// Files larger than this are skipped for content search.
    pub max_file_size: u64,

    /// Whether to index file contents for content search.
    pub index_contents: bool,

    /// Cache configuration.
    pub cache_config: CacheConfig,
}

/// Configuration for the file cache.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Whether caching is enabled.
    pub enabled: bool,

    /// Maximum number of files to cache.
    pub max_entries: usize,

    /// Time-to-live for cache entries in seconds.
    /// `None` means entries never expire.
    pub ttl_seconds: Option<u64>,

    /// Whether to persist cache to disk.
    pub persist_to_disk: bool,

    /// Path to cache file (if persisting).
    pub cache_path: Option<PathBuf>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: 100_000,
            ttl_seconds: Some(300), // 5 minutes
            persist_to_disk: false,
            cache_path: None,
        }
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            max_depth: None,
            follow_symlinks: false,
            respect_gitignore: true,
            include_hidden: false,
            ignore_patterns: Vec::new(),
            include_extensions: Vec::new(),
            exclude_extensions: vec![
                "exe".to_string(),
                "dll".to_string(),
                "so".to_string(),
                "dylib".to_string(),
                "o".to_string(),
                "a".to_string(),
                "lib".to_string(),
                "pyc".to_string(),
                "pyo".to_string(),
                "class".to_string(),
                "jar".to_string(),
                "war".to_string(),
                "zip".to_string(),
                "tar".to_string(),
                "gz".to_string(),
                "bz2".to_string(),
                "xz".to_string(),
                "7z".to_string(),
                "rar".to_string(),
                "png".to_string(),
                "jpg".to_string(),
                "jpeg".to_string(),
                "gif".to_string(),
                "bmp".to_string(),
                "ico".to_string(),
                "svg".to_string(),
                "webp".to_string(),
                "mp3".to_string(),
                "mp4".to_string(),
                "avi".to_string(),
                "mkv".to_string(),
                "mov".to_string(),
                "wav".to_string(),
                "flac".to_string(),
                "pdf".to_string(),
                "doc".to_string(),
                "docx".to_string(),
                "xls".to_string(),
                "xlsx".to_string(),
                "ppt".to_string(),
                "pptx".to_string(),
                "wasm".to_string(),
            ],
            exclude_dirs: vec![
                ".git".to_string(),
                ".hg".to_string(),
                ".svn".to_string(),
                "node_modules".to_string(),
                "target".to_string(),
                "build".to_string(),
                "dist".to_string(),
                ".cache".to_string(),
                "__pycache__".to_string(),
                ".venv".to_string(),
                "venv".to_string(),
                ".idea".to_string(),
                ".vscode".to_string(),
                "vendor".to_string(),
            ],
            max_file_size: 10 * 1024 * 1024, // 10 MB
            index_contents: false,
            cache_config: CacheConfig::default(),
        }
    }
}

impl SearchConfig {
    /// Creates a new configuration with the specified root directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ..Default::default()
        }
    }

    /// Creates a builder for constructing a configuration.
    pub fn builder(root: impl Into<PathBuf>) -> SearchConfigBuilder {
        SearchConfigBuilder::new(root)
    }

    /// Checks if a file extension should be included.
    pub fn should_include_extension(&self, ext: &str) -> bool {
        let ext_lower = ext.to_lowercase();

        // If include list is specified, only include those
        if !self.include_extensions.is_empty() {
            return self
                .include_extensions
                .iter()
                .any(|e| e.eq_ignore_ascii_case(ext));
        }

        // Otherwise, exclude based on exclude list
        !self
            .exclude_extensions
            .iter()
            .any(|e| e.eq_ignore_ascii_case(&ext_lower))
    }

    /// Checks if a directory should be excluded.
    pub fn should_exclude_dir(&self, name: &str) -> bool {
        self.exclude_dirs.iter().any(|d| d == name)
    }
}

/// Builder for creating `SearchConfig` instances.
#[derive(Debug)]
pub struct SearchConfigBuilder {
    config: SearchConfig,
}

impl SearchConfigBuilder {
    /// Creates a new builder with the specified root directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            config: SearchConfig::new(root),
        }
    }

    /// Sets the maximum traversal depth.
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.config.max_depth = Some(depth);
        self
    }

    /// Sets whether to follow symbolic links.
    pub fn follow_symlinks(mut self, follow: bool) -> Self {
        self.config.follow_symlinks = follow;
        self
    }

    /// Sets whether to respect .gitignore files.
    pub fn respect_gitignore(mut self, respect: bool) -> Self {
        self.config.respect_gitignore = respect;
        self
    }

    /// Sets whether to include hidden files.
    pub fn include_hidden(mut self, include: bool) -> Self {
        self.config.include_hidden = include;
        self
    }

    /// Adds ignore patterns.
    pub fn ignore_patterns(
        mut self,
        patterns: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.config.ignore_patterns = patterns.into_iter().map(Into::into).collect();
        self
    }

    /// Adds a single ignore pattern.
    pub fn add_ignore_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.config.ignore_patterns.push(pattern.into());
        self
    }

    /// Sets file extensions to include.
    pub fn include_extensions(
        mut self,
        extensions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.config.include_extensions = extensions.into_iter().map(Into::into).collect();
        self
    }

    /// Sets file extensions to exclude.
    pub fn exclude_extensions(
        mut self,
        extensions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.config.exclude_extensions = extensions.into_iter().map(Into::into).collect();
        self
    }

    /// Sets directories to exclude.
    pub fn exclude_dirs(mut self, dirs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.config.exclude_dirs = dirs.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the maximum file size for content indexing.
    pub fn max_file_size(mut self, size: u64) -> Self {
        self.config.max_file_size = size;
        self
    }

    /// Sets whether to index file contents.
    pub fn index_contents(mut self, index: bool) -> Self {
        self.config.index_contents = index;
        self
    }

    /// Enables or disables caching.
    pub fn enable_cache(mut self, enable: bool) -> Self {
        self.config.cache_config.enabled = enable;
        self
    }

    /// Sets the maximum number of cache entries.
    pub fn cache_max_entries(mut self, max: usize) -> Self {
        self.config.cache_config.max_entries = max;
        self
    }

    /// Sets the cache TTL in seconds.
    pub fn cache_ttl(mut self, ttl: Option<u64>) -> Self {
        self.config.cache_config.ttl_seconds = ttl;
        self
    }

    /// Builds the configuration.
    pub fn build(self) -> SearchConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SearchConfig::default();
        assert!(config.respect_gitignore);
        assert!(!config.include_hidden);
        assert!(!config.exclude_extensions.is_empty());
    }

    #[test]
    fn test_config_builder() {
        let config = SearchConfig::builder("/test/path")
            .max_depth(5)
            .include_hidden(true)
            .respect_gitignore(false)
            .add_ignore_pattern("*.log")
            .build();

        assert_eq!(config.max_depth, Some(5));
        assert!(config.include_hidden);
        assert!(!config.respect_gitignore);
        assert!(config.ignore_patterns.contains(&"*.log".to_string()));
    }

    #[test]
    fn test_should_include_extension() {
        let config = SearchConfig::default();
        assert!(config.should_include_extension("rs"));
        assert!(config.should_include_extension("js"));
        assert!(!config.should_include_extension("exe"));
        assert!(!config.should_include_extension("png"));
    }

    #[test]
    fn test_should_exclude_dir() {
        let config = SearchConfig::default();
        assert!(config.should_exclude_dir("node_modules"));
        assert!(config.should_exclude_dir(".git"));
        assert!(!config.should_exclude_dir("src"));
    }

    #[test]
    fn test_include_extensions_filter() {
        let config = SearchConfig::builder("/test")
            .include_extensions(["rs", "toml"])
            .build();

        assert!(config.should_include_extension("rs"));
        assert!(config.should_include_extension("toml"));
        assert!(!config.should_include_extension("js"));
    }
}
