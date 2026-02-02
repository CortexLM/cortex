//! Shell command execution.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::process::Command;

use super::security::{is_dangerous_command, parse_shell_command, validate_working_directory};
use super::types::ToolResult;

/// Execute a shell command with proper sandboxing.
pub async fn execute_shell(cwd: &Path, timeout_secs: u64, args: Value) -> ToolResult {
    let command = match args.get("command") {
        Some(Value::String(cmd)) => vec![cmd.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => return ToolResult::error("command is required (string or array)"),
    };

    if command.is_empty() {
        return ToolResult::error("command cannot be empty");
    }

    let workdir = args
        .get("workdir")
        .or_else(|| args.get("cwd"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf());

    let timeout = args
        .get("timeout")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(timeout_secs);

    // Security: Parse and validate command to prevent injection
    let (program, cmd_args) = if command.len() == 1 {
        // Single string command - parse it safely instead of using sh -c
        match parse_shell_command(&command[0]) {
            Ok((prog, args)) => (prog, args),
            Err(e) => return ToolResult::error(format!("Invalid command: {}", e)),
        }
    } else {
        // Array format is already safe - first element is program, rest are args
        (command[0].clone(), command[1..].to_vec())
    };

    // Security: Validate the program is not a blocked command
    if is_dangerous_command(&program) {
        return ToolResult::error(format!("Blocked dangerous command: {}", program));
    }

    // Security: Validate working directory is within allowed paths
    let validated_cwd = match validate_working_directory(&workdir, cwd) {
        Ok(path) => path,
        Err(e) => return ToolResult::error(format!("Invalid working directory: {}", e)),
    };

    let mut cmd = Command::new(&program);
    cmd.args(&cmd_args)
        .current_dir(&validated_cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Clear environment to prevent leaking sensitive vars, then set safe ones
    cmd.env_clear();
    // Set minimal safe environment
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );
    cmd.env(
        "HOME",
        std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()),
    );
    cmd.env(
        "USER",
        std::env::var("USER").unwrap_or_else(|_| "nobody".to_string()),
    );
    cmd.env("LANG", "C.UTF-8");

    let output =
        match tokio::time::timeout(std::time::Duration::from_secs(timeout), cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return ToolResult::error(format!("Failed to execute: {e}")),
            Err(_) => return ToolResult::error("Command timed out"),
        };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        ToolResult {
            success: true,
            output: stdout.to_string(),
            error: if stderr.is_empty() {
                None
            } else {
                Some(stderr.to_string())
            },
            metadata: Some(json!({ "exit_code": 0 })),
        }
    } else {
        ToolResult {
            success: false,
            output: stdout.to_string(),
            error: Some(stderr.to_string()),
            metadata: Some(json!({ "exit_code": output.status.code() })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_shell() {
        let cwd = std::env::current_dir().unwrap();
        let result = execute_shell(&cwd, 60, json!({ "command": "echo hello" })).await;
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }
}
