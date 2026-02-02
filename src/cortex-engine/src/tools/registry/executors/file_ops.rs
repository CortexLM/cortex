//! File operation tool executors (read, write, list, search, edit).

use serde_json::Value;

use crate::error::Result;
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::ToolResult;

impl ToolRegistry {
    pub(crate) async fn execute_read_file(&self, args: Value) -> Result<ToolResult> {
        // Support both "file_path" (new) and "path" (legacy)
        let path = args
            .get("file_path")
            .or_else(|| args.get("path"))
            .and_then(|p| p.as_str())
            .ok_or_else(|| {
                crate::error::CortexError::InvalidInput("file_path is required".into())
            })?;

        match tokio::fs::read_to_string(path).await {
            Ok(content) => Ok(ToolResult::success(content)),
            Err(e) => Ok(ToolResult::error(format!("Failed to read file: {e}"))),
        }
    }

    pub(crate) async fn execute_write_file(&self, args: Value) -> Result<ToolResult> {
        // Support both "file_path" (new) and "path" (legacy)
        let path = args
            .get("file_path")
            .or_else(|| args.get("path"))
            .and_then(|p| p.as_str())
            .ok_or_else(|| {
                crate::error::CortexError::InvalidInput("file_path is required".into())
            })?;
        let content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| crate::error::CortexError::InvalidInput("content is required".into()))?;

        match tokio::fs::write(path, content).await {
            Ok(_) => Ok(ToolResult::success(format!(
                "Wrote {} bytes to {}",
                content.len(),
                path
            ))),
            Err(e) => Ok(ToolResult::error(format!("Failed to write file: {e}"))),
        }
    }

    pub(crate) async fn execute_list_dir(&self, args: Value) -> Result<ToolResult> {
        // Support both "directory_path" (new) and "path" (legacy)
        let path = args
            .get("directory_path")
            .or_else(|| args.get("path"))
            .and_then(|p| p.as_str())
            .unwrap_or(".");

        let mut entries = Vec::new();
        let mut dir = match tokio::fs::read_dir(path).await {
            Ok(d) => d,
            Err(e) => {
                return Ok(ToolResult::error(format!("Failed to read directory: {e}")));
            }
        };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = entry.metadata().await.ok();
            let suffix = if meta
                .as_ref()
                .map(std::fs::Metadata::is_dir)
                .unwrap_or(false)
            {
                "/"
            } else {
                ""
            };
            entries.push(format!("{name}{suffix}"));
        }

        entries.sort();
        Ok(ToolResult::success(entries.join("\n")))
    }

    pub(crate) async fn execute_search_files(&self, args: Value) -> Result<ToolResult> {
        let pattern = args
            .get("pattern")
            .and_then(|p| p.as_str())
            .ok_or_else(|| crate::error::CortexError::InvalidInput("pattern is required".into()))?;
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");

        let output = tokio::process::Command::new("find")
            .args([path, "-name", pattern])
            .output()
            .await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                Ok(ToolResult::success(stdout.to_string()))
            }
            Err(e) => Ok(ToolResult::error(format!("Search failed: {e}"))),
        }
    }

    pub(crate) async fn execute_edit_file(&self, args: Value) -> Result<ToolResult> {
        let file_path = args
            .get("file_path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| {
                crate::error::CortexError::InvalidInput("file_path is required".into())
            })?;
        let old_str = args
            .get("old_str")
            .and_then(|s| s.as_str())
            .ok_or_else(|| crate::error::CortexError::InvalidInput("old_str is required".into()))?;
        let new_str = args
            .get("new_str")
            .and_then(|s| s.as_str())
            .ok_or_else(|| crate::error::CortexError::InvalidInput("new_str is required".into()))?;
        let change_all = args
            .get("change_all")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let content = match tokio::fs::read_to_string(file_path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("Failed to read file: {e}"))),
        };

        if !content.contains(old_str) {
            return Ok(ToolResult::error(format!(
                "Could not find the specified text in {file_path}"
            )));
        }

        if !change_all {
            let count = content.matches(old_str).count();
            if count > 1 {
                return Ok(ToolResult::error(format!(
                    "Found {count} occurrences. Use change_all=true or provide more context."
                )));
            }
        }

        let new_content = if change_all {
            content.replace(old_str, new_str)
        } else {
            content.replacen(old_str, new_str, 1)
        };

        match tokio::fs::write(file_path, new_content).await {
            Ok(_) => Ok(ToolResult::success(format!(
                "Successfully edited {file_path}"
            ))),
            Err(e) => Ok(ToolResult::error(format!("Failed to write file: {e}"))),
        }
    }
}
