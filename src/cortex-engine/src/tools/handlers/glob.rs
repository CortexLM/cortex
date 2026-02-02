//! Glob tool handler for file path searching.

use std::fs;
use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::Result;
use crate::tools::artifacts::{ArtifactConfig, process_tool_result};

/// Handler for glob tool.
pub struct GlobHandler;

#[derive(Debug, Deserialize)]
struct GlobArgs {
    patterns: Vec<String>,
    directory: Option<String>,
    #[serde(default)]
    exclude_patterns: Vec<String>,
}

impl GlobHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GlobHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for GlobHandler {
    fn name(&self) -> &str {
        "Glob"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: GlobArgs = serde_json::from_value(arguments)?;
        let search_path = args
            .directory
            .map(|p| context.resolve_path(&p))
            .unwrap_or_else(|| context.cwd.clone());

        if !search_path.exists() {
            return Ok(ToolResult::error(format!(
                "Directory not found: {}",
                search_path.display()
            )));
        }

        let mut results = Vec::new();
        glob_search(
            &search_path,
            &search_path,
            &args.patterns,
            &args.exclude_patterns,
            &mut results,
        );

        results.sort();
        if results.is_empty() {
            Ok(ToolResult::success(format!(
                "No files found matching patterns {:?} in '{}'. Search completed successfully with 0 results. Do NOT retry this search - no files match these patterns in this location.",
                args.patterns,
                search_path.display()
            )))
        } else {
            let result = ToolResult::success(results.join("\n"));

            // Process through artifact system for large results
            let artifact_config = ArtifactConfig::default();
            process_tool_result(result, &context.conversation_id, "Glob", &artifact_config)
        }
    }
}

fn glob_search(
    root: &Path,
    current: &Path,
    patterns: &[String],
    exclude_patterns: &[String],
    results: &mut Vec<String>,
) {
    if let Ok(dir) = fs::read_dir(current) {
        for entry in dir.flatten() {
            let entry_path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden and common ignore patterns by default
            if name.starts_with('.')
                || name == "node_modules"
                || name == "target"
                || name == ".git"
                || name == "__pycache__"
            {
                continue;
            }

            let relative = entry_path
                .strip_prefix(root)
                .unwrap_or(&entry_path)
                .to_string_lossy()
                .to_string();

            // Check exclude patterns
            if exclude_patterns
                .iter()
                .any(|p| glob_match(p, &relative) || glob_match(p, &name))
            {
                continue;
            }

            if entry_path.is_dir() {
                glob_search(root, &entry_path, patterns, exclude_patterns, results);
            } else {
                // Check if matches any include pattern
                for pattern in patterns {
                    if glob_match(pattern, &name) || glob_match(pattern, &relative) {
                        results.push(entry_path.display().to_string());
                        break;
                    }
                }
            }
        }
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.trim_start_matches("**/");
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();

    fn match_helper(pattern: &[char], text: &[char]) -> bool {
        match (pattern.first(), text.first()) {
            (None, None) => true,
            (Some('*'), _) => {
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
