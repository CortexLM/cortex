//! File operation tool handlers.

use std::fs;
use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::Result;
use crate::tools::artifacts::{ArtifactConfig, process_tool_result};
use crate::tools::spec::ToolMetadata;

// ============================================================
// ReadFileHandler
// ============================================================

/// Handler for read_file tool.
pub struct ReadFileHandler;

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    // Support both new (file_path) and legacy (path) parameter names
    #[serde(alias = "path")]
    file_path: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

impl ReadFileHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ReadFileHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for ReadFileHandler {
    fn name(&self) -> &str {
        "Read"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: ReadFileArgs = serde_json::from_value(arguments)?;
        let file_path = args.file_path.ok_or_else(|| {
            crate::error::CortexError::InvalidInput("file_path is required".into())
        })?;
        let path = context.resolve_path(&file_path);

        if !path.exists() {
            return Ok(ToolResult::error(format!(
                "File not found: {}",
                path.display()
            )));
        }

        // Build metadata first (needed for both empty and non-empty files)
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        let file_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        // Check if file is an image (JPEG or PNG)
        if is_image_file(&path) {
            // Validate image size (max 5MB)
            if file_size > 5 * 1024 * 1024 {
                return Ok(ToolResult::error(format!(
                    "Image file too large: {} bytes (max 5MB)",
                    file_size
                )));
            }

            // Read image file as binary
            match fs::read(&path) {
                Ok(image_data) => {
                    let metadata = ToolMetadata {
                        duration_ms: 0,
                        exit_code: Some(0),
                        files_modified: vec![],
                        data: Some(json!({
                            "path": file_path,
                            "filename": filename,
                            "extension": extension,
                            "size": file_size,
                            "is_image": true,
                            "image_format": extension.to_lowercase()
                        })),
                    };
                    // Return image data as base64 for analysis
                    let base64_data = base64_encode(&image_data);
                    return Ok(ToolResult::success(format!(
                        "data:image/{};base64,{}",
                        extension.to_lowercase(),
                        base64_data
                    ))
                    .with_metadata(metadata));
                }
                Err(e) => {
                    return Ok(ToolResult::error(format!("Failed to read image file: {e}")));
                }
            }
        }

        // Handle text files
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!("Failed to read file: {e}")));
            }
        };

        // Handle empty files explicitly
        if content.is_empty() {
            let metadata = ToolMetadata {
                duration_ms: 0,
                exit_code: Some(0),
                files_modified: vec![],
                data: Some(json!({
                    "path": file_path,
                    "filename": filename,
                    "extension": extension,
                    "size": file_size,
                    "total_lines": 0,
                    "shown_lines": 0,
                    "offset": 0,
                    "truncated": false,
                    "empty": true
                })),
            };
            return Ok(ToolResult::success("(empty file)".to_string()).with_metadata(metadata));
        }

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let offset = args.offset.unwrap_or(0);
        // Default limit is 2400 per spec
        let limit = args.limit.unwrap_or(2400);

        let selected: Vec<&str> = lines.into_iter().skip(offset).take(limit).collect();
        let shown_lines = selected.len();

        let metadata = ToolMetadata {
            duration_ms: 0,
            exit_code: Some(0),
            files_modified: vec![],
            data: Some(json!({
                "path": file_path,
                "filename": filename,
                "extension": extension,
                "size": file_size,
                "total_lines": total_lines,
                "shown_lines": shown_lines,
                "offset": offset,
                "truncated": shown_lines < total_lines
            })),
        };

        Ok(ToolResult::success(selected.join("\n")).with_metadata(metadata))
    }
}

// ============================================================
// WriteFileHandler
// ============================================================

/// Handler for write_file tool.
pub struct WriteFileHandler;

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    // Support both new (file_path) and legacy (path) parameter names
    #[serde(alias = "path")]
    file_path: Option<String>,
    content: String,
}

impl WriteFileHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WriteFileHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for WriteFileHandler {
    fn name(&self) -> &str {
        "Create"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: WriteFileArgs = serde_json::from_value(arguments)?;
        let file_path = args.file_path.ok_or_else(|| {
            crate::error::CortexError::InvalidInput("file_path is required".into())
        })?;
        let path = context.resolve_path(&file_path);

        // Create parent directories if needed
        if let Some(parent) = path.parent()
            && !parent.exists()
            && let Err(e) = fs::create_dir_all(parent)
        {
            return Ok(ToolResult::error(format!(
                "Failed to create directory: {e}"
            )));
        }

        match fs::write(&path, &args.content) {
            Ok(_) => {
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let extension = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_string();
                let content_preview = if args.content.len() > 500 {
                    args.content[..500].to_string()
                } else {
                    args.content.clone()
                };

                let metadata = ToolMetadata {
                    duration_ms: 0,
                    exit_code: Some(0),
                    files_modified: vec![file_path.clone()],
                    data: Some(json!({
                        "path": file_path,
                        "filename": filename,
                        "extension": extension,
                        "size": args.content.len(),
                        "content_preview": content_preview
                    })),
                };

                Ok(ToolResult::success(format!(
                    "Successfully wrote {} bytes to {}",
                    args.content.len(),
                    path.display()
                ))
                .with_metadata(metadata))
            }
            Err(e) => Ok(ToolResult::error(format!("Failed to write file: {e}"))),
        }
    }
}

// ============================================================
// TreeHandler (formerly ListDirHandler)
// ============================================================

/// Handler for tree tool - displays directory structure.
pub struct TreeHandler;

#[derive(Debug, Deserialize)]
struct TreeArgs {
    // Support both new (directory_path) and legacy (path) parameter names
    #[serde(alias = "path")]
    directory_path: Option<String>,
    #[serde(default)]
    #[serde(alias = "ignorePatterns")]
    ignore_patterns: Option<Vec<String>>,
}

impl TreeHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TreeHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for TreeHandler {
    fn name(&self) -> &str {
        "Tree"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        // Handle null/empty arguments - default to current directory
        let args: TreeArgs = if arguments.is_null() || arguments == serde_json::json!({}) {
            TreeArgs {
                directory_path: None,
                ignore_patterns: None,
            }
        } else {
            serde_json::from_value(arguments)?
        };
        // Use directory_path if provided, otherwise default to cwd
        let dir_path = args.directory_path.unwrap_or_else(|| ".".to_string());
        let path = context.resolve_path(&dir_path);

        if !path.exists() {
            return Ok(ToolResult::error(format!(
                "Directory not found: {}",
                path.display()
            )));
        }

        if !path.is_dir() {
            return Ok(ToolResult::error(format!(
                "Not a directory: {}",
                path.display()
            )));
        }

        let mut entries = Vec::new();
        let mut entries_metadata: Vec<serde_json::Value> = Vec::new();

        // Always use recursive tree display
        list_tree_recursive(
            &path,
            &path,
            &mut entries,
            &mut entries_metadata,
            &args.ignore_patterns,
        );

        entries.sort();
        entries_metadata.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        let metadata = ToolMetadata {
            duration_ms: 0,
            exit_code: Some(0),
            files_modified: vec![],
            data: Some(json!({
                "path": dir_path,
                "entries": entries_metadata
            })),
        };

        // Provide explicit message when directory is empty to prevent model hallucination
        let output = if entries.is_empty() {
            format!(
                "Directory '{}' is empty (no files or subdirectories found).",
                path.display()
            )
        } else {
            entries.join("\n")
        };

        let result = ToolResult::success(output).with_metadata(metadata);

        // Process through artifact system for large directory trees
        let artifact_config = ArtifactConfig::default();
        process_tool_result(result, &context.conversation_id, "Tree", &artifact_config)
    }
}

fn list_tree_recursive(
    root: &std::path::Path,
    current: &std::path::Path,
    entries: &mut Vec<String>,
    entries_metadata: &mut Vec<serde_json::Value>,
    ignore_patterns: &Option<Vec<String>>,
) {
    if let Ok(dir) = fs::read_dir(current) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();

            // Check if name matches any ignore pattern
            if let Some(patterns) = ignore_patterns {
                if patterns.iter().any(|pattern| glob_match(pattern, &name)) {
                    continue;
                }
            }

            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let is_dir = path.is_dir();
            let file_type = if is_dir { "dir" } else { "file" };
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

            entries.push(format!("{} {}", file_type, relative.display()));
            entries_metadata.push(json!({
                "name": name,
                "type": if is_dir { "directory" } else { "file" },
                "size": size
            }));

            if is_dir {
                list_tree_recursive(root, &path, entries, entries_metadata, ignore_patterns);
            }
        }
    }
}

// ============================================================
// SearchFilesHandler
// ============================================================

/// Handler for search_files tool.
pub struct SearchFilesHandler;

#[derive(Debug, Deserialize)]
struct SearchFilesArgs {
    pattern: String,
    path: Option<String>,
    content_pattern: Option<String>,
}

impl SearchFilesHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SearchFilesHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for SearchFilesHandler {
    fn name(&self) -> &str {
        "SearchFiles"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: SearchFilesArgs = serde_json::from_value(arguments)?;
        let search_path = args
            .path
            .map(|p| context.resolve_path(&p))
            .unwrap_or_else(|| context.cwd.clone());

        let mut matches = Vec::new();
        search_files_recursive(
            &search_path,
            &args.pattern,
            &args.content_pattern,
            &mut matches,
        );

        if matches.is_empty() {
            Ok(ToolResult::success("No files found matching the pattern"))
        } else {
            let result = ToolResult::success(matches.join("\n"));

            // Process through artifact system for large results
            let artifact_config = ArtifactConfig::default();
            process_tool_result(
                result,
                &context.conversation_id,
                "SearchFiles",
                &artifact_config,
            )
        }
    }
}

fn search_files_recursive(
    path: &std::path::Path,
    pattern: &str,
    content_pattern: &Option<String>,
    matches: &mut Vec<String>,
) {
    if let Ok(dir) = fs::read_dir(path) {
        for entry in dir.flatten() {
            let entry_path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files and common ignore patterns
            if name.starts_with('.') || name == "node_modules" || name == "target" || name == ".git"
            {
                continue;
            }

            if entry_path.is_dir() {
                search_files_recursive(&entry_path, pattern, content_pattern, matches);
            } else {
                // Simple glob matching
                if glob_match(pattern, &name) {
                    // If content pattern specified, check file contents
                    if let Some(content_pat) = content_pattern {
                        if let Ok(content) = fs::read_to_string(&entry_path)
                            && content.contains(content_pat)
                        {
                            matches.push(entry_path.display().to_string());
                        }
                    } else {
                        matches.push(entry_path.display().to_string());
                    }
                }
            }
        }
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    // Simple glob matching (* and ?)
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();

    fn match_helper(pattern: &[char], text: &[char]) -> bool {
        match (pattern.first(), text.first()) {
            (None, None) => true,
            (Some('*'), _) => {
                // Try matching zero or more characters
                match_helper(&pattern[1..], text)
                    || (!text.is_empty() && match_helper(pattern, &text[1..]))
            }
            (Some('?'), Some(_)) => match_helper(&pattern[1..], &text[1..]),
            (Some(p), Some(t)) if p == t => match_helper(&pattern[1..], &text[1..]),
            _ => false,
        }
    }

    match_helper(&pattern_chars, &text_chars)
}

fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let lower = ext.to_lowercase();
            lower == "jpg" || lower == "jpeg" || lower == "png"
        })
        .unwrap_or(false)
}

fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    for chunk in data.chunks(3) {
        let b1 = chunk[0];
        let b2 = chunk.get(1).copied().unwrap_or(0);
        let b3 = chunk.get(2).copied().unwrap_or(0);

        let n = ((b1 as u32) << 16) | ((b2 as u32) << 8) | (b3 as u32);

        let c1 = BASE64_CHARS[((n >> 18) & 0x3F) as usize] as char;
        let c2 = BASE64_CHARS[((n >> 12) & 0x3F) as usize] as char;
        let c3 = if chunk.len() > 1 {
            BASE64_CHARS[((n >> 6) & 0x3F) as usize] as char
        } else {
            '='
        };
        let c4 = if chunk.len() > 2 {
            BASE64_CHARS[(n & 0x3F) as usize] as char
        } else {
            '='
        };

        let _ = write!(result, "{}{}{}{}", c1, c2, c3, c4);
    }

    result
}
