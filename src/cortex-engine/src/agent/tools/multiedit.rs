use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::fs;

use crate::error::Result;
use crate::tools::handlers::ToolHandler;
use crate::tools::spec::ToolMetadata;
use crate::tools::{ToolContext, ToolResult};

/// MultiEditTool allows applying multiple search/replace operations across multiple files atomically.
pub struct MultiEditTool;

#[derive(Debug, Deserialize)]
struct MultiEditArgs {
    edits: Vec<EditBlock>,
}

#[derive(Debug, Deserialize)]
struct EditBlock {
    file_path: String,
    old_str: String,
    new_str: String,
    #[serde(default)]
    change_all: bool,
}

impl MultiEditTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MultiEditTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for MultiEditTool {
    fn name(&self) -> &str {
        "MultiEdit"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: MultiEditArgs = match serde_json::from_value(arguments) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Invalid arguments: {e}"))),
        };

        if args.edits.is_empty() {
            return Ok(ToolResult::error("No edits specified"));
        }

        // 1. Read and verify all files and search strings
        let mut file_contents: HashMap<PathBuf, String> = HashMap::new();
        let mut file_path_map: HashMap<String, PathBuf> = HashMap::new();

        for edit in &args.edits {
            if !file_path_map.contains_key(&edit.file_path) {
                let path = context.resolve_path(&edit.file_path);
                file_path_map.insert(edit.file_path.clone(), path.clone());

                if !file_contents.contains_key(&path) {
                    if !path.exists() {
                        return Ok(ToolResult::error(format!(
                            "File not found: {}",
                            edit.file_path
                        )));
                    }
                    let content = match fs::read_to_string(&path).await {
                        Ok(c) => c,
                        Err(e) => {
                            return Ok(ToolResult::error(format!(
                                "Failed to read {}: {}",
                                edit.file_path, e
                            )));
                        }
                    };
                    file_contents.insert(path.clone(), content);
                }
            }
        }

        // 2. Perform replacements in memory to ensure all blocks can be applied
        // We work on a copy to maintain atomicity if verification fails
        let mut working_contents = file_contents.clone();
        let mut files_modified = Vec::new();

        for (i, edit) in args.edits.iter().enumerate() {
            let path = file_path_map.get(&edit.file_path).unwrap();
            let content = working_contents.get_mut(path).unwrap();

            if !content.contains(&edit.old_str) {
                return Ok(ToolResult::error(format!(
                    "Edit block {} failed: Could not find specified text in {}",
                    i + 1,
                    edit.file_path
                )));
            }

            if !edit.change_all {
                let count = content.matches(&edit.old_str).count();
                if count > 1 {
                    return Ok(ToolResult::error(format!(
                        "Edit block {} failed: Found {} occurrences of the text in {}. Use change_all=true or provide more context.",
                        i + 1,
                        count,
                        edit.file_path
                    )));
                }
                *content = content.replacen(&edit.old_str, &edit.new_str, 1);
            } else {
                *content = content.replace(&edit.old_str, &edit.new_str);
            }

            if !files_modified.contains(&edit.file_path) {
                files_modified.push(edit.file_path.clone());
            }
        }

        // 3. Write all modified files back to disk
        for (path, content) in &working_contents {
            // Only write if content actually changed
            if content != file_contents.get(path).unwrap() {
                if let Err(e) = fs::write(path, content).await {
                    return Ok(ToolResult::error(format!(
                        "Failed to write to {}: {}",
                        path.display(),
                        e
                    )));
                }
            }
        }

        let metadata = ToolMetadata {
            duration_ms: 0,
            exit_code: Some(0),
            files_modified,
            data: Some(json!({
                "edits_applied": args.edits.len(),
                "files_affected": working_contents.len()
            })),
        };

        Ok(ToolResult::success(format!(
            "Successfully applied {} edits across {} files",
            args.edits.len(),
            working_contents.len()
        ))
        .with_metadata(metadata))
    }
}
