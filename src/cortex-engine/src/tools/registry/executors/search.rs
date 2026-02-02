//! Search tool executors (grep, glob).

use serde_json::Value;

use crate::error::Result;
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::ToolResult;

impl ToolRegistry {
    pub(crate) async fn execute_grep(&self, args: Value) -> Result<ToolResult> {
        let pattern = args
            .get("pattern")
            .and_then(|p| p.as_str())
            .ok_or_else(|| crate::error::CortexError::InvalidInput("pattern is required".into()))?;
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
        let case_insensitive = args
            .get("case_insensitive")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let mut cmd_args = vec!["-r", "-l"];
        if case_insensitive {
            cmd_args.push("-i");
        }
        cmd_args.push(pattern);
        cmd_args.push(path);

        let output = tokio::process::Command::new("grep")
            .args(&cmd_args)
            .output()
            .await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.is_empty() {
                    Ok(ToolResult::success("No matches found"))
                } else {
                    Ok(ToolResult::success(stdout.to_string()))
                }
            }
            Err(e) => Ok(ToolResult::error(format!("Grep failed: {e}"))),
        }
    }

    pub(crate) async fn execute_glob(&self, args: Value) -> Result<ToolResult> {
        let patterns = args
            .get("patterns")
            .and_then(|p| p.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();
        let folder = args.get("folder").and_then(|p| p.as_str()).unwrap_or(".");

        if patterns.is_empty() {
            return Ok(ToolResult::error("At least one pattern is required"));
        }

        let mut results = Vec::new();
        for pattern in &patterns {
            let output = tokio::process::Command::new("find")
                .args([folder, "-name", pattern])
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
            Ok(ToolResult::success("No files found matching the patterns"))
        } else {
            Ok(ToolResult::success(results.join("\n")))
        }
    }
}
