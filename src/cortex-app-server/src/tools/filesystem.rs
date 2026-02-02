//! Filesystem operations (read, write, edit, list).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::types::ToolResult;

/// Read file contents.
pub async fn read_file(cwd: &Path, args: Value) -> ToolResult {
    let path = match args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())
    {
        Some(p) => p,
        None => return ToolResult::error("file_path is required"),
    };

    let full_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };

    let offset = args
        .get("offset")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .map(|l| l as usize);

    // Get file metadata
    let file_meta = tokio::fs::metadata(&full_path).await.ok();
    let file_size = file_meta.as_ref().map(std::fs::Metadata::len).unwrap_or(0);

    // Detect file extension for syntax highlighting hint
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();

    match tokio::fs::read_to_string(&full_path).await {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();
            let selected: Vec<&str> = lines
                .into_iter()
                .skip(offset)
                .take(limit.unwrap_or(usize::MAX))
                .collect();
            let shown_lines = selected.len();

            ToolResult {
                success: true,
                output: selected.join("\n"),
                error: None,
                metadata: Some(json!({
                    "path": path,
                    "filename": std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(""),
                    "extension": extension,
                    "size": file_size,
                    "total_lines": total_lines,
                    "shown_lines": shown_lines,
                    "offset": offset,
                    "truncated": shown_lines < total_lines
                })),
            }
        }
        Err(e) => ToolResult::error(format!("Failed to read file: {e}")),
    }
}

/// Write content to a file.
pub async fn write_file(cwd: &Path, args: Value) -> ToolResult {
    let path = match args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())
    {
        Some(p) => p,
        None => return ToolResult::error("file_path is required"),
    };

    let content = match args.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return ToolResult::error("content is required"),
    };

    let full_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };

    // Create parent directories if needed
    if let Some(parent) = full_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        return ToolResult::error(format!("Failed to create directories: {e}"));
    }

    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();

    match tokio::fs::write(&full_path, content).await {
        Ok(_) => ToolResult {
            success: true,
            output: format!("Successfully wrote {} bytes to {}", content.len(), path),
            error: None,
            metadata: Some(json!({
                "path": path,
                "filename": std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "extension": extension,
                "size": content.len(),
                "content_preview": if content.len() > 500 { &content[..500] } else { content }
            })),
        },
        Err(e) => ToolResult::error(format!("Failed to write file: {e}")),
    }
}

/// Edit a file by finding and replacing text.
pub async fn edit_file(cwd: &Path, args: Value) -> ToolResult {
    let path = match args.get("file_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ToolResult::error("file_path is required"),
    };

    let old_str = match args.get("old_str").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ToolResult::error("old_str is required"),
    };

    let new_str = match args.get("new_str").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ToolResult::error("new_str is required"),
    };

    let change_all = args
        .get("change_all")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let full_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };

    let content = match tokio::fs::read_to_string(&full_path).await {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to read file: {e}")),
    };

    if !content.contains(old_str) {
        return ToolResult::error(format!("Could not find the specified text in {path}"));
    }

    let count = content.matches(old_str).count();
    if count > 1 && !change_all {
        return ToolResult::error(format!(
            "Found {count} occurrences. Use change_all=true or provide more context."
        ));
    }

    let new_content = if change_all {
        content.replace(old_str, new_str)
    } else {
        content.replacen(old_str, new_str, 1)
    };

    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();

    match tokio::fs::write(&full_path, new_content).await {
        Ok(_) => ToolResult {
            success: true,
            output: format!("Successfully edited {path}"),
            error: None,
            metadata: Some(json!({
                "path": path,
                "filename": std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "extension": extension,
                "old_str": old_str,
                "new_str": new_str,
                "occurrences_replaced": if change_all { count } else { 1 }
            })),
        },
        Err(e) => ToolResult::error(format!("Failed to write file: {e}")),
    }
}

/// List directory contents.
pub async fn list_dir(cwd: &Path, args: Value) -> ToolResult {
    let path = args
        .get("directory_path")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    let full_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };

    let mut entries = match tokio::fs::read_dir(&full_path).await {
        Ok(d) => d,
        Err(e) => return ToolResult::error(format!("Failed to read directory: {e}")),
    };

    let mut items = Vec::new();
    let mut entries_metadata = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = entry.metadata().await.ok();
        let is_dir = meta
            .as_ref()
            .map(std::fs::Metadata::is_dir)
            .unwrap_or(false);
        let size = meta.as_ref().map(std::fs::Metadata::len).unwrap_or(0);

        let suffix = if is_dir { "/" } else { "" };
        items.push(format!("{name}{suffix}"));

        entries_metadata.push(json!({
            "name": name,
            "type": if is_dir { "directory" } else { "file" },
            "size": size
        }));
    }

    items.sort();
    entries_metadata.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });

    ToolResult {
        success: true,
        output: items.join("\n"),
        error: None,
        metadata: Some(json!({
            "path": path,
            "entries": entries_metadata
        })),
    }
}

/// Apply multiple edits to files in a single operation.
pub async fn multi_edit(cwd: &Path, args: Value) -> ToolResult {
    let edits = match args.get("edits").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return ToolResult::error("edits array is required"),
    };

    let mut results = Vec::new();

    for edit in edits {
        let file_path = match edit.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                results.push("Error: file_path missing".to_string());
                continue;
            }
        };

        let old_str = match edit.get("old_str").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                results.push(format!("Error: old_str missing for {file_path}"));
                continue;
            }
        };

        let new_str = match edit.get("new_str").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                results.push(format!("Error: new_str missing for {file_path}"));
                continue;
            }
        };

        let result = edit_file(
            cwd,
            json!({
                "file_path": file_path,
                "old_str": old_str,
                "new_str": new_str
            }),
        )
        .await;

        if result.success {
            results.push(format!("OK: {file_path}"));
        } else {
            results.push(format!(
                "Error: {} - {}",
                file_path,
                result.error.unwrap_or_default()
            ));
        }
    }

    ToolResult::success(results.join("\n"))
}

/// Apply a unified diff patch.
pub async fn apply_patch(cwd: &Path, args: Value) -> ToolResult {
    use tokio::process::Command;

    let patch = match args.get("patch").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ToolResult::error("patch is required"),
    };

    // Write patch to temp file
    let temp_path =
        std::env::temp_dir().join(format!("cortex-patch-{}.diff", uuid::Uuid::new_v4()));

    if let Err(e) = tokio::fs::write(&temp_path, patch).await {
        return ToolResult::error(format!("Failed to write patch: {e}"));
    }

    let output = Command::new("patch")
        .args(["-p1", "-i"])
        .arg(&temp_path)
        .current_dir(cwd)
        .output()
        .await;

    // Cleanup
    let _ = tokio::fs::remove_file(&temp_path).await;

    match output {
        Ok(output) if output.status.success() => {
            ToolResult::success(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(output) => ToolResult {
            success: false,
            output: String::from_utf8_lossy(&output.stdout).to_string(),
            error: Some(String::from_utf8_lossy(&output.stderr).to_string()),
            metadata: None,
        },
        Err(e) => ToolResult::error(format!("Patch failed: {e}")),
    }
}
