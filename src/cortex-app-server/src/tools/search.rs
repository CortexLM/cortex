//! Search operations (grep, glob).

use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::process::Command;

use super::types::ToolResult;

/// Search file contents for a pattern.
pub async fn grep(cwd: &Path, args: Value) -> ToolResult {
    let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ToolResult::error("pattern is required"),
    };

    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");

    let case_insensitive = args
        .get("case_insensitive")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let line_numbers = args
        .get("line_numbers")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let full_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };

    // Try ripgrep first, fall back to grep
    let mut cmd_args = vec!["-r"];
    if case_insensitive {
        cmd_args.push("-i");
    }
    if line_numbers {
        cmd_args.push("-n");
    }

    let output = Command::new("rg")
        .args(&cmd_args)
        .arg(pattern)
        .arg(&full_path)
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() || output.status.code() == Some(1) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.is_empty() {
                ToolResult::success("No matches found")
            } else {
                ToolResult::success(stdout.to_string())
            }
        }
        _ => {
            // Fall back to grep
            let output = Command::new("grep")
                .args(&cmd_args)
                .arg(pattern)
                .arg(&full_path)
                .output()
                .await;

            match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.is_empty() {
                        ToolResult::success("No matches found")
                    } else {
                        ToolResult::success(stdout.to_string())
                    }
                }
                Err(e) => ToolResult::error(format!("Grep failed: {e}")),
            }
        }
    }
}

/// Find files matching glob patterns.
pub async fn glob(cwd: &Path, args: Value) -> ToolResult {
    let patterns = match args.get("patterns").and_then(|v| v.as_array()) {
        Some(arr) => arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>(),
        None => return ToolResult::error("patterns is required (array of strings)"),
    };

    let folder = args.get("folder").and_then(|v| v.as_str()).unwrap_or(".");

    let full_path = if PathBuf::from(folder).is_absolute() {
        PathBuf::from(folder)
    } else {
        cwd.join(folder)
    };

    let mut results = Vec::new();
    for pattern in &patterns {
        let output = Command::new("find")
            .arg(&full_path)
            .args(["-name", pattern])
            .output()
            .await;

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if !line.is_empty() && !results.contains(&line.to_string()) {
                    results.push(line.to_string());
                }
            }
        }
    }

    results.sort();
    if results.is_empty() {
        ToolResult::success("No files found matching the patterns")
    } else {
        ToolResult::success(results.join("\n"))
    }
}
