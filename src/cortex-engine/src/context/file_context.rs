//! File context management for including file contents in prompts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CortexError, Result};

/// File context for including file contents in prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContext {
    /// File path.
    pub path: PathBuf,
    /// File content.
    pub content: String,
    /// Language/type.
    pub language: Option<String>,
    /// Start line (1-indexed, inclusive).
    pub start_line: Option<u32>,
    /// End line (1-indexed, inclusive).
    pub end_line: Option<u32>,
    /// Token count estimate.
    pub token_count: u32,
    /// Relevance score (0.0 - 1.0).
    pub relevance: f32,
    /// Last modified timestamp.
    pub modified: Option<u64>,
    /// File size in bytes.
    pub size: u64,
    /// Whether content is truncated.
    pub truncated: bool,
    /// Metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

impl FileContext {
    /// Create file context from path.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        let content = std::fs::read_to_string(path).map_err(CortexError::Io)?;

        let metadata = std::fs::metadata(path).map_err(CortexError::Io)?;

        let size = metadata.len();
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        let language = detect_language(path);
        let token_count = estimate_tokens(&content);

        Ok(Self {
            path: path.to_path_buf(),
            content,
            language,
            start_line: None,
            end_line: None,
            token_count,
            relevance: 1.0,
            modified,
            size,
            truncated: false,
            metadata: HashMap::new(),
        })
    }

    /// Create file context with line range.
    pub fn from_path_with_range(
        path: impl AsRef<Path>,
        start_line: u32,
        end_line: u32,
    ) -> Result<Self> {
        let path = path.as_ref();

        let full_content = std::fs::read_to_string(path).map_err(CortexError::Io)?;

        let lines: Vec<&str> = full_content.lines().collect();
        let start = (start_line as usize).saturating_sub(1);
        let end = (end_line as usize).min(lines.len());

        let content = lines[start..end].join("\n");

        let metadata = std::fs::metadata(path).map_err(CortexError::Io)?;

        let language = detect_language(path);
        let token_count = estimate_tokens(&content);

        Ok(Self {
            path: path.to_path_buf(),
            content,
            language,
            start_line: Some(start_line),
            end_line: Some(end_line),
            token_count,
            relevance: 1.0,
            modified: metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
            size: metadata.len(),
            truncated: false,
            metadata: HashMap::new(),
        })
    }

    /// Get token count.
    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    /// Truncate content to token limit.
    pub fn truncate_to_tokens(&mut self, max_tokens: u32) {
        if self.token_count <= max_tokens {
            return;
        }

        let lines: Vec<&str> = self.content.lines().collect();
        let mut new_content = String::new();
        let mut current_tokens = 0u32;

        for line in lines {
            let line_tokens = (line.len() as u32 / 4) + 1;
            if current_tokens + line_tokens > max_tokens {
                new_content.push_str("... [truncated]\n");
                break;
            }
            new_content.push_str(line);
            new_content.push('\n');
            current_tokens += line_tokens;
        }

        self.content = new_content;
        self.token_count = current_tokens;
        self.truncated = true;
    }

    /// Format for inclusion in prompt.
    pub fn format(&self) -> String {
        let mut output = String::new();

        // Header
        output.push_str(&format!("=== File: {} ===\n", self.path.display()));

        if let Some(lang) = &self.language {
            output.push_str(&format!("Language: {lang}\n"));
        }

        if let (Some(start), Some(end)) = (self.start_line, self.end_line) {
            output.push_str(&format!("Lines: {start}-{end}\n"));
        }

        output.push_str("\n```");
        if let Some(lang) = &self.language {
            output.push_str(lang);
        }
        output.push('\n');
        output.push_str(&self.content);
        if !self.content.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("```\n");

        output
    }

    /// Format as minimal context.
    pub fn format_minimal(&self) -> String {
        format!("[{}: {} tokens]", self.path.display(), self.token_count)
    }

    /// Set relevance score.
    pub fn with_relevance(mut self, relevance: f32) -> Self {
        self.relevance = relevance.clamp(0.0, 1.0);
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// Builder for file context.
#[derive(Debug, Default)]
pub struct FileContextBuilder {
    path: Option<PathBuf>,
    content: Option<String>,
    language: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
    max_tokens: Option<u32>,
    relevance: f32,
    metadata: HashMap<String, serde_json::Value>,
}

impl FileContextBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            relevance: 1.0,
            ..Self::default()
        }
    }

    /// Set file path.
    pub fn path(mut self, path: impl AsRef<Path>) -> Self {
        self.path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set content directly.
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// Set language.
    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Set line range.
    pub fn line_range(mut self, start: u32, end: u32) -> Self {
        self.start_line = Some(start);
        self.end_line = Some(end);
        self
    }

    /// Set max tokens.
    pub fn max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    /// Set relevance.
    pub fn relevance(mut self, relevance: f32) -> Self {
        self.relevance = relevance;
        self
    }

    /// Add metadata.
    pub fn metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Build the file context.
    pub fn build(self) -> Result<FileContext> {
        let path = self
            .path
            .ok_or_else(|| CortexError::Provider("FileContext requires a path".to_string()))?;

        let mut ctx = if let Some(content) = self.content {
            let token_count = estimate_tokens(&content);
            FileContext {
                path,
                content,
                language: self.language.clone(),
                start_line: self.start_line,
                end_line: self.end_line,
                token_count,
                relevance: self.relevance,
                modified: None,
                size: 0,
                truncated: false,
                metadata: self.metadata.clone(),
            }
        } else if let (Some(start), Some(end)) = (self.start_line, self.end_line) {
            let mut ctx = FileContext::from_path_with_range(&path, start, end)?;
            ctx.relevance = self.relevance;
            ctx.metadata = self.metadata;
            if let Some(lang) = self.language {
                ctx.language = Some(lang);
            }
            ctx
        } else {
            let mut ctx = FileContext::from_path(&path)?;
            ctx.relevance = self.relevance;
            ctx.metadata = self.metadata;
            if let Some(lang) = self.language {
                ctx.language = Some(lang);
            }
            ctx
        };

        if let Some(max) = self.max_tokens {
            ctx.truncate_to_tokens(max);
        }

        Ok(ctx)
    }
}

/// File context collection for managing multiple files.
#[derive(Debug, Default)]
pub struct FileContextCollection {
    /// Contexts by path.
    contexts: HashMap<PathBuf, FileContext>,
    /// Maximum total tokens.
    max_tokens: u32,
}

impl FileContextCollection {
    /// Create a new collection.
    pub fn new(max_tokens: u32) -> Self {
        Self {
            contexts: HashMap::new(),
            max_tokens,
        }
    }

    /// Add a file context.
    pub fn add(&mut self, context: FileContext) -> Result<()> {
        let current_tokens: u32 = self.contexts.values().map(|c| c.token_count).sum();

        if current_tokens + context.token_count > self.max_tokens {
            return Err(CortexError::Provider(format!(
                "Adding file would exceed token limit: {} + {} > {}",
                current_tokens, context.token_count, self.max_tokens
            )));
        }

        self.contexts.insert(context.path.clone(), context);
        Ok(())
    }

    /// Remove a file context.
    pub fn remove(&mut self, path: &Path) -> Option<FileContext> {
        self.contexts.remove(path)
    }

    /// Get a file context.
    pub fn get(&self, path: &Path) -> Option<&FileContext> {
        self.contexts.get(path)
    }

    /// Get all contexts.
    pub fn all(&self) -> impl Iterator<Item = &FileContext> {
        self.contexts.values()
    }

    /// Get total token count.
    pub fn token_count(&self) -> u32 {
        self.contexts.values().map(|c| c.token_count).sum()
    }

    /// Get file count.
    pub fn len(&self) -> usize {
        self.contexts.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.contexts.is_empty()
    }

    /// Clear all contexts.
    pub fn clear(&mut self) {
        self.contexts.clear();
    }

    /// Sort by relevance (descending).
    pub fn sorted_by_relevance(&self) -> Vec<&FileContext> {
        let mut sorted: Vec<_> = self.contexts.values().collect();
        sorted.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        sorted
    }

    /// Trim to fit within max tokens.
    pub fn trim_to_fit(&mut self) {
        while self.token_count() > self.max_tokens && !self.contexts.is_empty() {
            // Remove lowest relevance file
            let lowest = self
                .contexts
                .iter()
                .min_by(|(_, a), (_, b)| {
                    a.relevance
                        .partial_cmp(&b.relevance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(k, _)| k.clone());

            if let Some(path) = lowest {
                self.contexts.remove(&path);
            }
        }
    }

    /// Format all contexts for prompt.
    pub fn format(&self) -> String {
        let mut output = String::new();

        for ctx in self.sorted_by_relevance() {
            output.push_str(&ctx.format());
            output.push('\n');
        }

        output
    }
}

/// Detect language from file extension.
fn detect_language(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    let lang = match ext.to_lowercase().as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "go" => "go",
        "java" => "java",
        "c" => "c",
        "cpp" | "cc" | "cxx" => "cpp",
        "h" | "hpp" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" => "kotlin",
        "scala" => "scala",
        "sh" | "bash" => "bash",
        "zsh" => "zsh",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "scss",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" => "xml",
        "md" => "markdown",
        _ => return None,
    };
    Some(lang.to_string())
}

/// Estimate token count for content.
fn estimate_tokens(content: &str) -> u32 {
    // Rough estimate: ~4 characters per token
    (content.len() as u32 / 4) + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_file_context_builder() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "fn main() {{\n    println!(\"Hello\");\n}}").unwrap();

        let ctx = FileContextBuilder::new()
            .path(file.path())
            .relevance(0.8)
            .build()
            .unwrap();

        assert!(ctx.content.contains("main"));
        assert_eq!(ctx.relevance, 0.8);
    }

    #[test]
    fn test_file_context_truncate() {
        let mut ctx = FileContext {
            path: PathBuf::from("test.rs"),
            content: "a".repeat(1000),
            language: Some("rust".to_string()),
            start_line: None,
            end_line: None,
            token_count: 250,
            relevance: 1.0,
            modified: None,
            size: 1000,
            truncated: false,
            metadata: HashMap::new(),
        };

        ctx.truncate_to_tokens(100);
        assert!(ctx.truncated);
        assert!(ctx.token_count <= 100);
    }

    #[test]
    fn test_file_context_collection() {
        let mut collection = FileContextCollection::new(1000);

        let ctx1 = FileContext {
            path: PathBuf::from("file1.rs"),
            content: "content1".to_string(),
            language: Some("rust".to_string()),
            start_line: None,
            end_line: None,
            token_count: 100,
            relevance: 0.5,
            modified: None,
            size: 8,
            truncated: false,
            metadata: HashMap::new(),
        };

        let ctx2 = FileContext {
            path: PathBuf::from("file2.rs"),
            content: "content2".to_string(),
            language: Some("rust".to_string()),
            start_line: None,
            end_line: None,
            token_count: 100,
            relevance: 0.8,
            modified: None,
            size: 8,
            truncated: false,
            metadata: HashMap::new(),
        };

        collection.add(ctx1).unwrap();
        collection.add(ctx2).unwrap();

        assert_eq!(collection.len(), 2);
        assert_eq!(collection.token_count(), 200);

        let sorted = collection.sorted_by_relevance();
        assert_eq!(sorted[0].relevance, 0.8);
    }
}
