//! Code analysis utilities.
//!
//! Provides utilities for analyzing code including language detection,
//! dependency analysis, and code metrics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Programming language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Language {
    /// Rust.
    Rust,
    /// Python.
    Python,
    /// JavaScript.
    JavaScript,
    /// TypeScript.
    TypeScript,
    /// Go.
    Go,
    /// Java.
    Java,
    /// C.
    C,
    /// C++.
    Cpp,
    /// C#.
    CSharp,
    /// Ruby.
    Ruby,
    /// PHP.
    Php,
    /// Swift.
    Swift,
    /// Kotlin.
    Kotlin,
    /// Scala.
    Scala,
    /// Shell.
    Shell,
    /// SQL.
    Sql,
    /// HTML.
    Html,
    /// CSS.
    Css,
    /// JSON.
    Json,
    /// YAML.
    Yaml,
    /// TOML.
    Toml,
    /// Markdown.
    Markdown,
    /// Unknown.
    #[default]
    Unknown,
}

impl Language {
    /// Detect from file extension.
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "rs" => Self::Rust,
            "py" => Self::Python,
            "js" | "mjs" | "cjs" => Self::JavaScript,
            "ts" | "tsx" | "mts" => Self::TypeScript,
            "go" => Self::Go,
            "java" => Self::Java,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" => Self::Cpp,
            "cs" => Self::CSharp,
            "rb" => Self::Ruby,
            "php" => Self::Php,
            "swift" => Self::Swift,
            "kt" | "kts" => Self::Kotlin,
            "scala" => Self::Scala,
            "sh" | "bash" | "zsh" => Self::Shell,
            "sql" => Self::Sql,
            "html" | "htm" => Self::Html,
            "css" | "scss" | "sass" | "less" => Self::Css,
            "json" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "md" | "markdown" => Self::Markdown,
            _ => Self::Unknown,
        }
    }

    /// Get language name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Go => "Go",
            Self::Java => "Java",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::CSharp => "C#",
            Self::Ruby => "Ruby",
            Self::Php => "PHP",
            Self::Swift => "Swift",
            Self::Kotlin => "Kotlin",
            Self::Scala => "Scala",
            Self::Shell => "Shell",
            Self::Sql => "SQL",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Json => "JSON",
            Self::Yaml => "YAML",
            Self::Toml => "TOML",
            Self::Markdown => "Markdown",
            Self::Unknown => "Unknown",
        }
    }

    /// Get file extensions.
    pub fn extensions(&self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["rs"],
            Self::Python => &["py"],
            Self::JavaScript => &["js", "mjs", "cjs"],
            Self::TypeScript => &["ts", "tsx", "mts"],
            Self::Go => &["go"],
            Self::Java => &["java"],
            Self::C => &["c", "h"],
            Self::Cpp => &["cpp", "cc", "cxx", "hpp"],
            Self::CSharp => &["cs"],
            Self::Ruby => &["rb"],
            Self::Php => &["php"],
            Self::Swift => &["swift"],
            Self::Kotlin => &["kt", "kts"],
            Self::Scala => &["scala"],
            Self::Shell => &["sh", "bash", "zsh"],
            Self::Sql => &["sql"],
            Self::Html => &["html", "htm"],
            Self::Css => &["css", "scss", "sass", "less"],
            Self::Json => &["json"],
            Self::Yaml => &["yaml", "yml"],
            Self::Toml => &["toml"],
            Self::Markdown => &["md", "markdown"],
            Self::Unknown => &[],
        }
    }

    /// Get comment syntax.
    pub fn comment_syntax(&self) -> Option<CommentSyntax> {
        match self {
            Self::Rust
            | Self::Go
            | Self::Java
            | Self::JavaScript
            | Self::TypeScript
            | Self::C
            | Self::Cpp
            | Self::CSharp
            | Self::Swift
            | Self::Kotlin
            | Self::Scala
            | Self::Php => Some(CommentSyntax {
                line: "//",
                block_start: Some("/*"),
                block_end: Some("*/"),
            }),
            Self::Python | Self::Ruby | Self::Shell | Self::Yaml | Self::Toml => {
                Some(CommentSyntax {
                    line: "#",
                    block_start: None,
                    block_end: None,
                })
            }
            Self::Html => Some(CommentSyntax {
                line: "<!--",
                block_start: Some("<!--"),
                block_end: Some("-->"),
            }),
            Self::Css => Some(CommentSyntax {
                line: "/*",
                block_start: Some("/*"),
                block_end: Some("*/"),
            }),
            Self::Sql => Some(CommentSyntax {
                line: "--",
                block_start: Some("/*"),
                block_end: Some("*/"),
            }),
            _ => None,
        }
    }
}

/// Comment syntax.
#[derive(Debug, Clone)]
pub struct CommentSyntax {
    /// Line comment prefix.
    pub line: &'static str,
    /// Block comment start.
    pub block_start: Option<&'static str>,
    /// Block comment end.
    pub block_end: Option<&'static str>,
}

/// Code metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodeMetrics {
    /// Total lines.
    pub total_lines: u32,
    /// Code lines.
    pub code_lines: u32,
    /// Comment lines.
    pub comment_lines: u32,
    /// Blank lines.
    pub blank_lines: u32,
    /// Functions/methods.
    pub functions: u32,
    /// Classes/structs.
    pub classes: u32,
    /// Imports.
    pub imports: u32,
}

impl CodeMetrics {
    /// Get comment ratio.
    pub fn comment_ratio(&self) -> f32 {
        if self.code_lines == 0 {
            0.0
        } else {
            self.comment_lines as f32 / self.code_lines as f32
        }
    }

    /// Get code density.
    pub fn code_density(&self) -> f32 {
        if self.total_lines == 0 {
            0.0
        } else {
            self.code_lines as f32 / self.total_lines as f32
        }
    }

    /// Merge with another metrics.
    pub fn merge(&mut self, other: &CodeMetrics) {
        self.total_lines += other.total_lines;
        self.code_lines += other.code_lines;
        self.comment_lines += other.comment_lines;
        self.blank_lines += other.blank_lines;
        self.functions += other.functions;
        self.classes += other.classes;
        self.imports += other.imports;
    }
}

/// Code analyzer.
pub struct CodeAnalyzer;

impl CodeAnalyzer {
    /// Analyze a file.
    pub fn analyze_file(path: &Path) -> Result<FileAnalysis> {
        let content = std::fs::read_to_string(path)?;
        let language = path
            .extension()
            .and_then(|e| e.to_str())
            .map(Language::from_extension)
            .unwrap_or(Language::Unknown);

        let metrics = Self::compute_metrics(&content, language);
        let imports = Self::extract_imports(&content, language);

        Ok(FileAnalysis {
            path: path.to_path_buf(),
            language,
            metrics,
            imports,
        })
    }

    /// Compute metrics for content.
    pub fn compute_metrics(content: &str, language: Language) -> CodeMetrics {
        let mut metrics = CodeMetrics::default();
        let comment_syntax = language.comment_syntax();
        let mut in_block_comment = false;

        for line in content.lines() {
            metrics.total_lines += 1;
            let trimmed = line.trim();

            if trimmed.is_empty() {
                metrics.blank_lines += 1;
                continue;
            }

            // Check for block comments
            if let Some(ref syntax) = comment_syntax {
                if let (Some(start), Some(end)) = (syntax.block_start, syntax.block_end) {
                    if trimmed.contains(start) {
                        in_block_comment = true;
                    }
                    if in_block_comment {
                        metrics.comment_lines += 1;
                        if trimmed.contains(end) {
                            in_block_comment = false;
                        }
                        continue;
                    }
                }

                // Line comment
                if trimmed.starts_with(syntax.line) {
                    metrics.comment_lines += 1;
                    continue;
                }
            }

            // Count as code
            metrics.code_lines += 1;

            // Count functions/classes (simplified)
            Self::count_definitions(trimmed, language, &mut metrics);
        }

        metrics
    }

    /// Count function/class definitions.
    fn count_definitions(line: &str, language: Language, metrics: &mut CodeMetrics) {
        match language {
            Language::Rust => {
                if line.starts_with("fn ") || line.starts_with("pub fn ") {
                    metrics.functions += 1;
                } else if line.starts_with("struct ")
                    || line.starts_with("pub struct ")
                    || line.starts_with("enum ")
                    || line.starts_with("pub enum ")
                {
                    metrics.classes += 1;
                } else if line.starts_with("use ") {
                    metrics.imports += 1;
                }
            }
            Language::Python => {
                if line.starts_with("def ") {
                    metrics.functions += 1;
                } else if line.starts_with("class ") {
                    metrics.classes += 1;
                } else if line.starts_with("import ") || line.starts_with("from ") {
                    metrics.imports += 1;
                }
            }
            Language::JavaScript | Language::TypeScript => {
                if line.contains("function ") || line.contains("=> {") {
                    metrics.functions += 1;
                } else if line.starts_with("class ") {
                    metrics.classes += 1;
                } else if line.starts_with("import ") || line.contains("require(") {
                    metrics.imports += 1;
                }
            }
            Language::Go => {
                if line.starts_with("func ") {
                    metrics.functions += 1;
                } else if line.starts_with("type ") && line.contains("struct") {
                    metrics.classes += 1;
                } else if line.starts_with("import ") {
                    metrics.imports += 1;
                }
            }
            Language::Java => {
                if line.contains("void ") || line.contains("public ") && line.contains("(") {
                    metrics.functions += 1;
                } else if line.contains("class ") || line.contains("interface ") {
                    metrics.classes += 1;
                } else if line.starts_with("import ") {
                    metrics.imports += 1;
                }
            }
            _ => {}
        }
    }

    /// Extract imports.
    fn extract_imports(content: &str, language: Language) -> Vec<String> {
        let mut imports = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();

            match language {
                Language::Rust => {
                    if trimmed.starts_with("use ") {
                        imports.push(trimmed.to_string());
                    }
                }
                Language::Python => {
                    if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
                        imports.push(trimmed.to_string());
                    }
                }
                Language::JavaScript | Language::TypeScript => {
                    if trimmed.starts_with("import ") || trimmed.contains("require(") {
                        imports.push(trimmed.to_string());
                    }
                }
                Language::Go => {
                    if trimmed.starts_with("import ") {
                        imports.push(trimmed.to_string());
                    }
                }
                Language::Java => {
                    if trimmed.starts_with("import ") {
                        imports.push(trimmed.to_string());
                    }
                }
                _ => {}
            }
        }

        imports
    }

    /// Analyze a directory.
    pub fn analyze_directory(path: &Path) -> Result<DirectoryAnalysis> {
        let mut files = Vec::new();
        let mut by_language: HashMap<Language, LanguageStats> = HashMap::new();
        let mut total_metrics = CodeMetrics::default();

        Self::walk_directory(path, &mut |file_path| {
            if let Ok(analysis) = Self::analyze_file(file_path) {
                total_metrics.merge(&analysis.metrics);

                let stats = by_language
                    .entry(analysis.language)
                    .or_insert_with(|| LanguageStats {
                        language: analysis.language,
                        file_count: 0,
                        metrics: CodeMetrics::default(),
                    });
                stats.file_count += 1;
                stats.metrics.merge(&analysis.metrics);

                files.push(analysis);
            }
        })?;

        Ok(DirectoryAnalysis {
            path: path.to_path_buf(),
            files,
            by_language: by_language.into_values().collect(),
            total_metrics,
        })
    }

    /// Walk directory recursively.
    fn walk_directory<F>(path: &Path, callback: &mut F) -> Result<()>
    where
        F: FnMut(&Path),
    {
        if path.is_file() {
            callback(path);
            return Ok(());
        }

        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();

            // Skip hidden files/dirs
            if entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false)
            {
                continue;
            }

            // Skip common non-code directories
            let dir_name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            if matches!(
                dir_name,
                "node_modules" | "target" | "dist" | "build" | "__pycache__" | ".git"
            ) {
                continue;
            }

            if entry_path.is_dir() {
                Self::walk_directory(&entry_path, callback)?;
            } else {
                callback(&entry_path);
            }
        }

        Ok(())
    }
}

/// File analysis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAnalysis {
    /// File path.
    pub path: PathBuf,
    /// Language.
    pub language: Language,
    /// Metrics.
    pub metrics: CodeMetrics,
    /// Imports.
    pub imports: Vec<String>,
}

/// Directory analysis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryAnalysis {
    /// Directory path.
    pub path: PathBuf,
    /// Analyzed files.
    pub files: Vec<FileAnalysis>,
    /// Stats by language.
    pub by_language: Vec<LanguageStats>,
    /// Total metrics.
    pub total_metrics: CodeMetrics,
}

impl DirectoryAnalysis {
    /// Get file count.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Get primary language.
    pub fn primary_language(&self) -> Option<Language> {
        self.by_language
            .iter()
            .max_by_key(|s| s.metrics.code_lines)
            .map(|s| s.language)
    }
}

/// Language statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageStats {
    /// Language.
    pub language: Language,
    /// File count.
    pub file_count: u32,
    /// Metrics.
    pub metrics: CodeMetrics,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_from_extension() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("unknown"), Language::Unknown);
    }

    #[test]
    fn test_language_name() {
        assert_eq!(Language::Rust.name(), "Rust");
        assert_eq!(Language::TypeScript.name(), "TypeScript");
    }

    #[test]
    fn test_code_metrics() {
        let content = r#"
// This is a comment
fn main() {
    println!("Hello");
}

/* Block
   comment */
"#;
        let metrics = CodeAnalyzer::compute_metrics(content, Language::Rust);

        assert!(metrics.total_lines > 0);
        assert!(metrics.code_lines > 0);
        assert!(metrics.comment_lines > 0);
        assert!(metrics.functions > 0);
    }

    #[test]
    fn test_metrics_merge() {
        let mut m1 = CodeMetrics {
            total_lines: 100,
            code_lines: 80,
            comment_lines: 10,
            blank_lines: 10,
            functions: 5,
            classes: 2,
            imports: 3,
        };

        let m2 = CodeMetrics {
            total_lines: 50,
            code_lines: 40,
            comment_lines: 5,
            blank_lines: 5,
            functions: 3,
            classes: 1,
            imports: 2,
        };

        m1.merge(&m2);

        assert_eq!(m1.total_lines, 150);
        assert_eq!(m1.functions, 8);
    }

    #[test]
    fn test_comment_ratio() {
        let metrics = CodeMetrics {
            code_lines: 100,
            comment_lines: 25,
            ..Default::default()
        };

        assert!((metrics.comment_ratio() - 0.25).abs() < 0.01);
    }

    #[test]
    fn test_python_analysis() {
        let content = r#"
import os
from pathlib import Path

# This is a comment
def hello():
    print("Hello")

class MyClass:
    pass
"#;
        let metrics = CodeAnalyzer::compute_metrics(content, Language::Python);

        assert!(metrics.imports >= 2);
        assert!(metrics.functions >= 1);
        assert!(metrics.classes >= 1);
    }
}
