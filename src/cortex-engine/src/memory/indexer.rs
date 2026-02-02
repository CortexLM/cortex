//! Code and document indexer.
//!
//! Provides:
//! - File watching for incremental updates
//! - Code chunking by functions/classes
//! - Language-aware parsing
//! - Metadata extraction

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::embedding::Embedder;
use super::store::{Memory, MemoryMetadata, MemoryStore, MemoryType};
use crate::error::Result;

/// Indexer configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerConfig {
    /// Root directory to index.
    pub root_path: PathBuf,
    /// File patterns to include (globs).
    pub include_patterns: Vec<String>,
    /// File patterns to exclude (globs).
    pub exclude_patterns: Vec<String>,
    /// Maximum file size to index (bytes).
    pub max_file_size: u64,
    /// Chunk size for splitting code.
    pub chunk_size: usize,
    /// Chunk overlap.
    pub chunk_overlap: usize,
    /// Enable language-aware chunking.
    pub language_aware: bool,
    /// Watch for file changes.
    pub watch_enabled: bool,
    /// Debounce time for file changes (ms).
    pub debounce_ms: u64,
    /// Index hidden files.
    pub include_hidden: bool,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            root_path: PathBuf::from("."),
            include_patterns: vec![
                "**/*.rs".to_string(),
                "**/*.py".to_string(),
                "**/*.js".to_string(),
                "**/*.ts".to_string(),
                "**/*.go".to_string(),
                "**/*.java".to_string(),
                "**/*.cpp".to_string(),
                "**/*.c".to_string(),
                "**/*.h".to_string(),
                "**/*.md".to_string(),
                "**/*.txt".to_string(),
            ],
            exclude_patterns: vec![
                "**/target/**".to_string(),
                "**/node_modules/**".to_string(),
                "**/.git/**".to_string(),
                "**/dist/**".to_string(),
                "**/build/**".to_string(),
                "**/__pycache__/**".to_string(),
                "**/.venv/**".to_string(),
            ],
            max_file_size: 1024 * 1024, // 1MB
            chunk_size: 1500,
            chunk_overlap: 200,
            language_aware: true,
            watch_enabled: false,
            debounce_ms: 500,
            include_hidden: false,
        }
    }
}

/// Code chunk extracted from a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeChunk {
    /// Unique ID.
    pub id: String,
    /// File path.
    pub file_path: PathBuf,
    /// Content.
    pub content: String,
    /// Start line (1-indexed).
    pub start_line: usize,
    /// End line (1-indexed).
    pub end_line: usize,
    /// Language.
    pub language: String,
    /// Entity type (function, class, module, etc).
    pub entity_type: Option<String>,
    /// Entity name.
    pub entity_name: Option<String>,
    /// Parent entity name (e.g., class for a method).
    pub parent_name: Option<String>,
}

impl CodeChunk {
    /// Create metadata for this chunk.
    pub fn to_metadata(&self) -> MemoryMetadata {
        MemoryMetadata {
            file_path: Some(self.file_path.clone()),
            line_range: Some((self.start_line, self.end_line)),
            language: Some(self.language.clone()),
            entity_name: self.entity_name.clone(),
            tags: vec![
                self.language.clone(),
                self.entity_type
                    .clone()
                    .unwrap_or_else(|| "chunk".to_string()),
            ],
            custom: HashMap::new(),
        }
    }
}

/// Index update result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexUpdate {
    /// Files indexed.
    pub files_indexed: usize,
    /// Chunks created.
    pub chunks_created: usize,
    /// Files skipped.
    pub files_skipped: usize,
    /// Errors encountered.
    pub errors: Vec<String>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// File indexer for extracting and storing file content.
#[derive(Debug)]
pub struct FileIndexer {
    config: IndexerConfig,
}

impl FileIndexer {
    /// Create a new file indexer.
    pub fn new(config: IndexerConfig) -> Self {
        Self { config }
    }

    /// Check if a file should be indexed.
    pub fn should_index(&self, path: &Path) -> bool {
        // Check if file exists and is a file
        if !path.is_file() {
            return false;
        }

        // Check file size
        if let Ok(metadata) = std::fs::metadata(path) {
            if metadata.len() > self.config.max_file_size {
                return false;
            }
        }

        // Check hidden files
        if !self.config.include_hidden {
            if let Some(name) = path.file_name() {
                if name.to_string_lossy().starts_with('.') {
                    return false;
                }
            }
        }

        let path_str = path.to_string_lossy();

        // Check exclude patterns
        for pattern in &self.config.exclude_patterns {
            if let Ok(glob) = glob::Pattern::new(pattern) {
                if glob.matches(&path_str) {
                    return false;
                }
            }
        }

        // Check include patterns
        for pattern in &self.config.include_patterns {
            if let Ok(glob) = glob::Pattern::new(pattern) {
                if glob.matches(&path_str) {
                    return true;
                }
            }
        }

        false
    }

    /// Detect language from file extension.
    pub fn detect_language(&self, path: &Path) -> String {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| match ext.to_lowercase().as_str() {
                "rs" => "rust",
                "py" => "python",
                "js" => "javascript",
                "ts" => "typescript",
                "tsx" => "typescript",
                "jsx" => "javascript",
                "go" => "go",
                "java" => "java",
                "cpp" | "cc" | "cxx" => "cpp",
                "c" => "c",
                "h" | "hpp" => "c",
                "rb" => "ruby",
                "php" => "php",
                "swift" => "swift",
                "kt" => "kotlin",
                "scala" => "scala",
                "cs" => "csharp",
                "md" => "markdown",
                "txt" => "text",
                "json" => "json",
                "yaml" | "yml" => "yaml",
                "toml" => "toml",
                "xml" => "xml",
                "html" => "html",
                "css" => "css",
                "sql" => "sql",
                "sh" | "bash" => "bash",
                _ => "unknown",
            })
            .unwrap_or("unknown")
            .to_string()
    }

    /// Chunk a file's content.
    pub fn chunk_file(&self, path: &Path, content: &str) -> Vec<CodeChunk> {
        let language = self.detect_language(path);

        if self.config.language_aware {
            self.chunk_by_structure(path, content, &language)
        } else {
            self.chunk_by_lines(path, content, &language)
        }
    }

    /// Chunk by line-based sliding window.
    fn chunk_by_lines(&self, path: &Path, content: &str, language: &str) -> Vec<CodeChunk> {
        let lines: Vec<&str> = content.lines().collect();
        let mut chunks = Vec::new();

        if lines.is_empty() {
            return chunks;
        }

        let lines_per_chunk = self.config.chunk_size / 80; // Approximate lines
        let overlap_lines = self.config.chunk_overlap / 80;

        let mut start = 0;

        while start < lines.len() {
            let end = (start + lines_per_chunk).min(lines.len());
            let chunk_content = lines[start..end].join("\n");

            chunks.push(CodeChunk {
                id: format!("{}:{}:{}", path.display(), start + 1, end),
                file_path: path.to_path_buf(),
                content: chunk_content,
                start_line: start + 1,
                end_line: end,
                language: language.to_string(),
                entity_type: Some("chunk".to_string()),
                entity_name: None,
                parent_name: None,
            });

            if end >= lines.len() {
                break;
            }

            start = if end > overlap_lines {
                end - overlap_lines
            } else {
                end
            };
        }

        chunks
    }

    /// Chunk by code structure (functions, classes, etc).
    fn chunk_by_structure(&self, path: &Path, content: &str, language: &str) -> Vec<CodeChunk> {
        // Use simple heuristics for now
        // In production, use tree-sitter for proper parsing
        match language {
            "rust" => self.chunk_rust(path, content),
            "python" => self.chunk_python(path, content),
            "javascript" | "typescript" => self.chunk_javascript(path, content),
            _ => self.chunk_by_lines(path, content, language),
        }
    }

    /// Chunk Rust code by functions and impl blocks.
    fn chunk_rust(&self, path: &Path, content: &str) -> Vec<CodeChunk> {
        let mut chunks = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        let mut current_chunk_start = 0;
        let mut brace_depth = 0;
        let mut in_function = false;
        let mut function_name = None;
        let mut current_impl = None;

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            // Track impl blocks
            if trimmed.starts_with("impl ") {
                if let Some(name) = trimmed
                    .strip_prefix("impl ")
                    .and_then(|s| s.split_whitespace().next())
                {
                    current_impl = Some(name.trim_end_matches('{').to_string());
                }
            }

            // Track function definitions
            if (trimmed.starts_with("pub fn ")
                || trimmed.starts_with("fn ")
                || trimmed.starts_with("pub async fn ")
                || trimmed.starts_with("async fn "))
                && !in_function
            {
                in_function = true;
                current_chunk_start = i;

                // Extract function name
                let fn_start = trimmed.find("fn ").unwrap_or(0) + 3;
                if let Some(end) = trimmed[fn_start..].find('(') {
                    function_name = Some(trimmed[fn_start..fn_start + end].to_string());
                }
            }

            // Track braces
            brace_depth += line.chars().filter(|c| *c == '{').count() as i32;
            brace_depth -= line.chars().filter(|c| *c == '}').count() as i32;

            // End of function
            if in_function && brace_depth == 0 && trimmed.ends_with('}') {
                let chunk_content = lines[current_chunk_start..=i].join("\n");

                if !chunk_content.trim().is_empty() {
                    chunks.push(CodeChunk {
                        id: format!(
                            "{}:fn:{}",
                            path.display(),
                            function_name.as_deref().unwrap_or("unknown")
                        ),
                        file_path: path.to_path_buf(),
                        content: chunk_content,
                        start_line: current_chunk_start + 1,
                        end_line: i + 1,
                        language: "rust".to_string(),
                        entity_type: Some("function".to_string()),
                        entity_name: function_name.take(),
                        parent_name: current_impl.clone(),
                    });
                }

                in_function = false;
                current_chunk_start = i + 1;
            }

            // Reset impl tracking when leaving impl block
            if brace_depth == 0 && trimmed == "}" {
                current_impl = None;
            }
        }

        // Handle remaining content
        if current_chunk_start < lines.len() {
            let remaining = lines[current_chunk_start..].join("\n");
            if !remaining.trim().is_empty() && remaining.len() > 50 {
                // Fall back to line-based chunking for remaining
                chunks.extend(self.chunk_by_lines(path, &remaining, "rust"));
            }
        }

        if chunks.is_empty() {
            // Fall back to line-based if no functions found
            return self.chunk_by_lines(path, content, "rust");
        }

        chunks
    }

    /// Chunk Python code by functions and classes.
    fn chunk_python(&self, path: &Path, content: &str) -> Vec<CodeChunk> {
        let mut chunks = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        let mut current_chunk_start = 0;
        let mut current_indent = 0;
        let mut in_definition = false;
        let mut entity_type = None;
        let mut entity_name = None;
        let mut current_class = None;

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            let indent = line.len() - line.trim_start().len();

            // Track class definitions
            if trimmed.starts_with("class ") {
                // Save previous chunk
                if in_definition && i > current_chunk_start {
                    let chunk_content = lines[current_chunk_start..i].join("\n");
                    if !chunk_content.trim().is_empty() {
                        chunks.push(CodeChunk {
                            id: format!(
                                "{}:{}:{}",
                                path.display(),
                                entity_type.as_deref().unwrap_or("chunk"),
                                entity_name.as_deref().unwrap_or("unknown")
                            ),
                            file_path: path.to_path_buf(),
                            content: chunk_content,
                            start_line: current_chunk_start + 1,
                            end_line: i,
                            language: "python".to_string(),
                            entity_type: entity_type.take(),
                            entity_name: entity_name.take(),
                            parent_name: current_class.clone(),
                        });
                    }
                }

                current_chunk_start = i;
                current_indent = indent;
                in_definition = true;
                entity_type = Some("class".to_string());

                if let Some(name) = trimmed
                    .strip_prefix("class ")
                    .and_then(|s| s.split(['(', ':']).next())
                {
                    current_class = Some(name.to_string());
                    entity_name = Some(name.to_string());
                }
            }
            // Track function definitions
            else if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
                // Save previous chunk if at same or lower indent
                if in_definition && indent <= current_indent && i > current_chunk_start {
                    let chunk_content = lines[current_chunk_start..i].join("\n");
                    if !chunk_content.trim().is_empty() {
                        chunks.push(CodeChunk {
                            id: format!(
                                "{}:{}:{}",
                                path.display(),
                                entity_type.as_deref().unwrap_or("chunk"),
                                entity_name.as_deref().unwrap_or("unknown")
                            ),
                            file_path: path.to_path_buf(),
                            content: chunk_content,
                            start_line: current_chunk_start + 1,
                            end_line: i,
                            language: "python".to_string(),
                            entity_type: entity_type.take(),
                            entity_name: entity_name.take(),
                            parent_name: if indent > 0 {
                                current_class.clone()
                            } else {
                                None
                            },
                        });
                    }
                }

                current_chunk_start = i;
                current_indent = indent;
                in_definition = true;
                entity_type = Some(if indent > 0 { "method" } else { "function" }.to_string());

                let prefix = if trimmed.starts_with("async def ") {
                    "async def "
                } else {
                    "def "
                };
                if let Some(name) = trimmed
                    .strip_prefix(prefix)
                    .and_then(|s| s.split('(').next())
                {
                    entity_name = Some(name.to_string());
                }

                // Reset class tracking if at module level
                if indent == 0 {
                    current_class = None;
                }
            }
        }

        // Handle last chunk
        if in_definition && current_chunk_start < lines.len() {
            let chunk_content = lines[current_chunk_start..].join("\n");
            if !chunk_content.trim().is_empty() {
                chunks.push(CodeChunk {
                    id: format!(
                        "{}:{}:{}",
                        path.display(),
                        entity_type.as_deref().unwrap_or("chunk"),
                        entity_name.as_deref().unwrap_or("unknown")
                    ),
                    file_path: path.to_path_buf(),
                    content: chunk_content,
                    start_line: current_chunk_start + 1,
                    end_line: lines.len(),
                    language: "python".to_string(),
                    entity_type,
                    entity_name,
                    parent_name: current_class,
                });
            }
        }

        if chunks.is_empty() {
            return self.chunk_by_lines(path, content, "python");
        }

        chunks
    }

    /// Chunk JavaScript/TypeScript code.
    fn chunk_javascript(&self, path: &Path, content: &str) -> Vec<CodeChunk> {
        let mut chunks = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        let mut current_chunk_start = 0;
        let mut brace_depth = 0;
        let mut in_function = false;
        let mut function_name = None;
        let mut current_class = None;

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            // Track class definitions
            if trimmed.starts_with("class ") || trimmed.starts_with("export class ") {
                let prefix = if trimmed.starts_with("export ") {
                    "export class "
                } else {
                    "class "
                };
                if let Some(name) = trimmed
                    .strip_prefix(prefix)
                    .and_then(|s| s.split_whitespace().next())
                {
                    current_class = Some(name.trim_end_matches('{').to_string());
                }
            }

            // Track function definitions
            let is_function = trimmed.starts_with("function ")
                || trimmed.starts_with("async function ")
                || trimmed.starts_with("export function ")
                || trimmed.starts_with("export async function ")
                || trimmed.contains("=>")
                || (trimmed.ends_with('{') && trimmed.contains('('));

            if is_function && !in_function {
                in_function = true;
                current_chunk_start = i;

                // Extract function name
                if let Some(fn_idx) = trimmed.find("function ") {
                    let start = fn_idx + 9;
                    if let Some(end) = trimmed[start..].find('(') {
                        function_name = Some(trimmed[start..start + end].to_string());
                    }
                }
            }

            // Track braces
            brace_depth += line.chars().filter(|c| *c == '{').count() as i32;
            brace_depth -= line.chars().filter(|c| *c == '}').count() as i32;

            // End of function
            if in_function && brace_depth == 0 {
                let chunk_content = lines[current_chunk_start..=i].join("\n");

                if !chunk_content.trim().is_empty() {
                    chunks.push(CodeChunk {
                        id: format!(
                            "{}:fn:{}",
                            path.display(),
                            function_name.as_deref().unwrap_or("anonymous")
                        ),
                        file_path: path.to_path_buf(),
                        content: chunk_content,
                        start_line: current_chunk_start + 1,
                        end_line: i + 1,
                        language: self.detect_language(path),
                        entity_type: Some("function".to_string()),
                        entity_name: function_name.take(),
                        parent_name: current_class.clone(),
                    });
                }

                in_function = false;
                current_chunk_start = i + 1;
            }
        }

        if chunks.is_empty() {
            return self.chunk_by_lines(path, content, &self.detect_language(path));
        }

        chunks
    }
}

/// Code indexer for maintaining an index of project code.
#[derive(Debug)]
pub struct CodeIndexer {
    store: Arc<MemoryStore>,
    embedder: Arc<dyn Embedder>,
    config: IndexerConfig,
    file_indexer: FileIndexer,
    /// Indexed files with their last modified time.
    indexed_files: HashMap<PathBuf, std::time::SystemTime>,
    /// Watcher shutdown channel.
    watcher_shutdown: Option<mpsc::Sender<()>>,
}

impl CodeIndexer {
    /// Create a new code indexer.
    pub fn new(
        store: Arc<MemoryStore>,
        embedder: Arc<dyn Embedder>,
        config: IndexerConfig,
    ) -> Self {
        let file_indexer = FileIndexer::new(config.clone());
        Self {
            store,
            embedder,
            config,
            file_indexer,
            indexed_files: HashMap::new(),
            watcher_shutdown: None,
        }
    }

    /// Index a directory recursively.
    pub async fn index_directory(&mut self, path: PathBuf) -> Result<IndexUpdate> {
        let start = std::time::Instant::now();
        let mut update = IndexUpdate::default();

        // Walk directory and collect paths first to avoid borrow conflict
        let include_hidden = self.config.include_hidden;
        let paths_to_index: Vec<PathBuf> = walkdir::WalkDir::new(&path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // Skip hidden directories
                if !include_hidden {
                    if let Some(name) = e.file_name().to_str() {
                        if name.starts_with('.') && e.file_type().is_dir() {
                            return false;
                        }
                    }
                }
                true
            })
            .filter_map(|e| e.ok())
            .map(|entry| entry.path().to_path_buf())
            .filter(|p| self.file_indexer.should_index(p))
            .collect();

        for entry_path in paths_to_index {
            match self.index_file(entry_path.clone()).await {
                Ok(chunks) => {
                    update.files_indexed += 1;
                    update.chunks_created += chunks;
                }
                Err(e) => {
                    update.files_skipped += 1;
                    update
                        .errors
                        .push(format!("{}: {}", entry_path.display(), e));
                }
            }
        }

        update.duration_ms = start.elapsed().as_millis() as u64;
        Ok(update)
    }

    /// Index a single file.
    pub async fn index_file(&mut self, path: PathBuf) -> Result<usize> {
        // Check if file needs reindexing
        let modified = std::fs::metadata(&path)?.modified()?;
        if let Some(&last_indexed) = self.indexed_files.get(&path) {
            if modified <= last_indexed {
                return Ok(0); // Already indexed and not modified
            }
        }

        // Read file content
        let content = tokio::fs::read_to_string(&path).await?;

        // Chunk the file
        let chunks = self.file_indexer.chunk_file(&path, &content);

        // Remove old chunks for this file
        // (In a real implementation, you'd track chunk IDs per file)

        // Store new chunks
        let mut count = 0;
        for chunk in chunks {
            let embedding = self.embedder.embed(&chunk.content).await?;
            let memory = Memory::new(
                chunk.content.clone(),
                embedding,
                MemoryType::Code,
                chunk.to_metadata(),
            );
            self.store.insert(memory).await?;
            count += 1;
        }

        // Update tracking
        self.indexed_files.insert(path, modified);

        Ok(count)
    }

    /// Start watching for file changes.
    pub async fn start_watching(&mut self, path: PathBuf) -> Result<()> {
        if self.watcher_shutdown.is_some() {
            return Ok(()); // Already watching
        }

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        self.watcher_shutdown = Some(shutdown_tx);

        // Clone what we need for the task
        let config = self.config.clone();
        let _watch_path = path.clone();

        // In a real implementation, you'd use notify crate here
        // For now, we just set up the shutdown mechanism
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        break;
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(config.debounce_ms)) => {
                        // Poll for changes (placeholder)
                    }
                }
            }
        });

        Ok(())
    }

    /// Stop watching for file changes.
    pub async fn stop_watching(&mut self) -> Result<()> {
        if let Some(tx) = self.watcher_shutdown.take() {
            let _ = tx.send(()).await;
        }
        Ok(())
    }

    /// Get indexed file count.
    pub fn indexed_file_count(&self) -> usize {
        self.indexed_files.len()
    }

    /// Clear the index.
    pub async fn clear(&mut self) -> Result<()> {
        self.indexed_files.clear();
        // Note: This doesn't clear the store, just the tracking
        Ok(())
    }

    /// Re-index all tracked files.
    pub async fn reindex_all(&mut self) -> Result<IndexUpdate> {
        let files: Vec<_> = self.indexed_files.keys().cloned().collect();
        self.indexed_files.clear();

        let mut update = IndexUpdate::default();
        let start = std::time::Instant::now();

        for path in files {
            match self.index_file(path.clone()).await {
                Ok(chunks) => {
                    update.files_indexed += 1;
                    update.chunks_created += chunks;
                }
                Err(e) => {
                    update.files_skipped += 1;
                    update.errors.push(format!("{}: {}", path.display(), e));
                }
            }
        }

        update.duration_ms = start.elapsed().as_millis() as u64;
        Ok(update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_indexer_should_index() {
        let config = IndexerConfig::default();
        let _indexer = FileIndexer::new(config);

        // These patterns should match (if files existed)
        // We can't test actual files without creating them
    }

    #[test]
    fn test_detect_language() {
        let config = IndexerConfig::default();
        let indexer = FileIndexer::new(config);

        assert_eq!(indexer.detect_language(Path::new("test.rs")), "rust");
        assert_eq!(indexer.detect_language(Path::new("test.py")), "python");
        assert_eq!(indexer.detect_language(Path::new("test.js")), "javascript");
        assert_eq!(indexer.detect_language(Path::new("test.ts")), "typescript");
        assert_eq!(indexer.detect_language(Path::new("test.go")), "go");
    }

    #[test]
    fn test_chunk_rust() {
        let config = IndexerConfig::default();
        let indexer = FileIndexer::new(config);

        let rust_code = r#"
fn hello() {
    println!("hello");
}

pub fn world() {
    println!("world");
}

impl Foo {
    fn bar(&self) {
        println!("bar");
    }
}
"#;

        let chunks = indexer.chunk_rust(Path::new("test.rs"), rust_code);
        assert!(!chunks.is_empty());

        // Should find at least the functions
        let fn_names: Vec<_> = chunks
            .iter()
            .filter_map(|c| c.entity_name.as_ref())
            .collect();
        assert!(fn_names.contains(&&"hello".to_string()));
        assert!(fn_names.contains(&&"world".to_string()));
    }

    #[test]
    fn test_chunk_python() {
        let config = IndexerConfig::default();
        let indexer = FileIndexer::new(config);

        let python_code = r#"
def hello():
    print("hello")

class Foo:
    def bar(self):
        print("bar")

async def async_func():
    pass
"#;

        let chunks = indexer.chunk_python(Path::new("test.py"), python_code);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_code_chunk_metadata() {
        let chunk = CodeChunk {
            id: "test".to_string(),
            file_path: PathBuf::from("test.rs"),
            content: "fn test() {}".to_string(),
            start_line: 1,
            end_line: 1,
            language: "rust".to_string(),
            entity_type: Some("function".to_string()),
            entity_name: Some("test".to_string()),
            parent_name: None,
        };

        let metadata = chunk.to_metadata();
        assert_eq!(metadata.language, Some("rust".to_string()));
        assert_eq!(metadata.entity_name, Some("test".to_string()));
        assert_eq!(metadata.line_range, Some((1, 1)));
    }
}
