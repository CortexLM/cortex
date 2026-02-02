//! Grep tool handler for searching file contents.

use std::fs;
use std::path::Path;

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::Result;
use crate::tools::artifacts::{ArtifactConfig, process_tool_result};

/// Handler for grep tool.
pub struct GrepHandler;

#[derive(Debug, Deserialize)]
struct GrepArgs {
    pattern: String,
    path: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    line_numbers: bool,
    context_before: Option<usize>,
    context_after: Option<usize>,
    context: Option<usize>,
    glob_pattern: Option<String>,
    #[serde(default = "default_output_mode")]
    output_mode: String,
    max_results: Option<usize>,
    #[serde(default)]
    multiline: bool,
    #[serde(default)]
    fixed_string: bool,
    file_type: Option<String>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    follow_symlinks: bool,
}

fn default_output_mode() -> String {
    "file_paths".to_string()
}

impl GrepHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GrepHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for GrepHandler {
    fn name(&self) -> &str {
        "Grep"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: GrepArgs = serde_json::from_value(arguments)?;
        let search_path = args
            .path
            .map(|p| context.resolve_path(&p))
            .unwrap_or_else(|| context.cwd.clone());

        // Build regex pattern
        let pattern = if args.fixed_string {
            // Escape regex special characters for literal matching
            regex::escape(&args.pattern)
        } else {
            args.pattern.clone()
        };

        let pattern = if args.case_insensitive {
            format!("(?i){}", pattern)
        } else {
            pattern
        };

        let pattern = if args.multiline {
            format!("(?m){}", pattern)
        } else {
            pattern
        };

        let regex = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult::error(format!("Invalid regex pattern: {e}")));
            }
        };

        let mut results = Vec::new();
        let content_mode = args.output_mode == "content";
        let context_lines = args.context.unwrap_or(0);
        let before = args.context_before.unwrap_or(context_lines);
        let after = args.context_after.unwrap_or(context_lines);

        search_content(
            &search_path,
            &regex,
            &args.glob_pattern,
            &args.file_type,
            &mut results,
            content_mode,
            args.line_numbers,
            before,
            after,
            args.include_hidden,
            args.follow_symlinks,
        );

        // Apply max_results limit
        if let Some(limit) = args.max_results {
            results.truncate(limit);
        }

        if results.is_empty() {
            Ok(ToolResult::success(format!(
                "No matches found for pattern '{}' in '{}'. Search completed successfully with 0 results. Do NOT retry this search - the pattern does not exist in this location.",
                args.pattern,
                search_path.display()
            )))
        } else {
            let result = ToolResult::success(results.join("\n"));

            // Process through artifact system for large results
            let artifact_config = ArtifactConfig::default();
            process_tool_result(result, &context.conversation_id, "Grep", &artifact_config)
        }
    }
}

fn search_content(
    path: &Path,
    regex: &Regex,
    glob_pattern: &Option<String>,
    file_type: &Option<String>,
    results: &mut Vec<String>,
    content_mode: bool,
    line_numbers: bool,
    context_before: usize,
    context_after: usize,
    include_hidden: bool,
    follow_symlinks: bool,
) {
    if path.is_file() {
        search_file(
            path,
            regex,
            results,
            content_mode,
            line_numbers,
            context_before,
            context_after,
        );
        return;
    }

    if let Ok(dir) = fs::read_dir(path) {
        for entry in dir.flatten() {
            let entry_path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if entry_path.is_symlink() && !follow_symlinks {
                continue;
            }

            if !include_hidden && name.starts_with('.') {
                continue;
            }

            if name == "node_modules" || name == "target" || name == ".git" || name == "__pycache__"
            {
                continue;
            }

            if entry_path.is_dir() {
                search_content(
                    &entry_path,
                    regex,
                    glob_pattern,
                    file_type,
                    results,
                    content_mode,
                    line_numbers,
                    context_before,
                    context_after,
                    include_hidden,
                    follow_symlinks,
                );
            } else {
                if let Some(glob) = glob_pattern
                    && !glob_match(glob, &name)
                {
                    continue;
                }

                if let Some(ftype) = file_type {
                    if !name.ends_with(&format!(".{}", ftype)) {
                        continue;
                    }
                }

                search_file(
                    &entry_path,
                    regex,
                    results,
                    content_mode,
                    line_numbers,
                    context_before,
                    context_after,
                );
            }
        }
    }
}

fn search_file(
    path: &Path,
    regex: &Regex,
    results: &mut Vec<String>,
    content_mode: bool,
    line_numbers: bool,
    context_before: usize,
    context_after: usize,
) {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return, // Skip binary or unreadable files
    };

    let lines: Vec<&str> = content.lines().collect();
    let mut matched = false;
    let mut match_lines: Vec<(usize, &str)> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if regex.is_match(line) {
            matched = true;
            if content_mode {
                // Collect lines with context
                let start = i.saturating_sub(context_before);
                let end = (i + context_after + 1).min(lines.len());
                for j in start..end {
                    if !match_lines.iter().any(|(idx, _)| *idx == j) {
                        match_lines.push((j, lines[j]));
                    }
                }
            }
        }
    }

    if matched {
        if content_mode {
            match_lines.sort_by_key(|(i, _)| *i);
            for (i, line) in match_lines {
                let prefix = if line_numbers {
                    format!("{}:{}:", path.display(), i + 1)
                } else {
                    format!("{}:", path.display())
                };
                results.push(format!("{prefix}{line}"));
            }
        } else {
            results.push(path.display().to_string());
        }
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
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
