//! Local shell tool handler.
//!
//! NOTE: Cortex uses DangerFullAccess by default - no sandbox restrictions.
//! Commands are executed directly without Landlock/seccomp filtering.
//!
//! SECURITY: This module uses the exec/runner module which provides:
//! - Process isolation via kill_on_drop and setpgid
//! - Environment filtering (removes sensitive variables)
//! - Non-interactive mode enforcement (CI=true, TERM=dumb, etc.)
//! - Proper timeout handling with process group killing

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;

use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::Result;
use crate::exec::{ExecOptions, ExecOutput, OutputChunk, execute_command_streaming};
use crate::tools::context::ToolOutputChunk;
use crate::tools::spec::ToolMetadata;

/// Characters and patterns that indicate shell metacharacters requiring shell interpretation.
/// These are checked to determine if we need to use shell execution mode.
const SHELL_METACHAR_PATTERNS: &[&str] = &[
    "&&", "||", "|", ";", ">", ">>", "<", "<<", "$", "`", "(", ")", "{", "}", "*", "?", "[", "]",
    "~", "!", "#", "&",
];

/// Patterns that indicate bash-specific syntax not supported by dash/sh.
/// When detected, we prefer bash over /bin/sh to avoid compatibility issues (#2808).
const BASH_SPECIFIC_PATTERNS: &[&str] = &[
    "[[",      // Bash conditional expressions
    "]]",      // Bash conditional expressions (closing)
    "<(",      // Process substitution
    ">(",      // Process substitution
    "${!",     // Indirect expansion
    "${#",     // String length
    "**",      // Bash exponentiation (2**10)
    "source ", // Bash-specific (POSIX uses '.')
    "shopt",   // Bash-specific shell options
    "declare", // Bash-specific variable declaration
    "local ",  // While POSIX has local, some sh don't
    "typeset", // Bash/ksh specific
    "+=",      // Append assignment
    ";&",      // Bash case fall-through
    ";;&",     // Bash case pattern testing
    "&>>",     // Bash append redirect both streams
    "|&",      // Bash pipe both streams
];

/// Handler for local_shell tool.
pub struct LocalShellHandler;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct LocalShellArgs {
    command: Vec<String>,
    workdir: Option<String>,
    timeout: Option<u64>,
    risk_level: Option<String>,
    risk_reason: Option<String>,
    background: Option<bool>,
}

impl LocalShellHandler {
    pub fn new() -> Self {
        Self
    }

    /// Build the final command, handling shell metacharacters properly.
    fn build_command(args: &LocalShellArgs) -> Vec<String> {
        // Check if command contains shell operators that need shell interpretation
        // SECURITY: We check for shell metacharacters to determine execution mode
        let needs_shell = args.command[0] == "cd"
            || args.command.iter().any(|arg| {
                SHELL_METACHAR_PATTERNS
                    .iter()
                    .any(|pattern| arg.contains(pattern))
            });

        if needs_shell {
            // SECURITY: When shell interpretation is needed, properly escape each argument
            // using shlex to prevent injection attacks. Each argument is individually
            // escaped, then joined with spaces for shell execution.
            let escaped_args: Vec<String> = args
                .command
                .iter()
                .map(|arg| {
                    // Use shlex::try_quote which handles all special characters safely
                    // Fall back to single-quote wrapping if shlex fails
                    shlex::try_quote(arg)
                        .map(|s| s.into_owned())
                        .unwrap_or_else(|_| format!("'{}'", arg.replace('\'', "'\\''")))
                })
                .collect();
            let full_cmd = escaped_args.join(" ");

            #[cfg(target_os = "windows")]
            {
                vec!["cmd.exe".to_string(), "/C".to_string(), full_cmd]
            }
            #[cfg(not(target_os = "windows"))]
            {
                // Detect if bash-specific syntax is used (#2808)
                // On Ubuntu and similar systems, /bin/sh is dash which doesn't support
                // bash-specific features like [[, <(), **, source, etc.
                let needs_bash = args.command.iter().any(|arg| {
                    BASH_SPECIFIC_PATTERNS
                        .iter()
                        .any(|pattern| arg.contains(pattern))
                });

                if needs_bash {
                    // Use bash explicitly for bash-specific syntax
                    // Try /bin/bash first, fall back to bash in PATH
                    let bash_path = if std::path::Path::new("/bin/bash").exists() {
                        "/bin/bash".to_string()
                    } else if std::path::Path::new("/usr/bin/bash").exists() {
                        "/usr/bin/bash".to_string()
                    } else {
                        "bash".to_string()
                    };
                    vec![bash_path, "-c".to_string(), full_cmd]
                } else {
                    // Use POSIX sh for simple shell commands
                    vec!["/bin/sh".to_string(), "-c".to_string(), full_cmd]
                }
            }
        } else {
            // Direct execution without shell - arguments are passed safely as array
            args.command.clone()
        }
    }
}

impl Default for LocalShellHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for LocalShellHandler {
    fn name(&self) -> &str {
        "Execute"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: LocalShellArgs = serde_json::from_value(arguments)?;

        if args.command.is_empty() {
            return Ok(ToolResult::error("Empty command"));
        }

        // Build the command (handles shell metacharacters)
        let command = Self::build_command(&args);

        // Resolve working directory
        let cwd = args
            .workdir
            .map(|w| context.resolve_path(&w))
            .unwrap_or_else(|| context.cwd.clone());

        // Build execution options
        let timeout = args
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(60));

        let options = ExecOptions {
            cwd,
            timeout,
            env: context.env.clone(),
            capture_output: true,
            ..Default::default()
        };

        // Create channel for streaming output
        let (tx, mut rx) = mpsc::channel::<OutputChunk>(100);

        // Clone context for the streaming task
        let ctx = context.clone();

        // Spawn task to forward output chunks to the tool context
        let forward_task = tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                match chunk {
                    OutputChunk::Stdout(s) => {
                        ctx.send_output(ToolOutputChunk::Stdout(s)).await;
                    }
                    OutputChunk::Stderr(s) => {
                        ctx.send_output(ToolOutputChunk::Stderr(s)).await;
                    }
                }
            }
        });

        // Execute command with streaming - this uses proper process isolation:
        // - kill_on_drop(true)
        // - env_clear() with safe environment rebuild
        // - setpgid(0,0) on Unix for process group isolation
        // - Filters sensitive environment variables
        // - Forces non-interactive mode (CI=true, TERM=dumb, etc.)
        let result = execute_command_streaming(&command, options, tx).await;

        // Wait for forwarding to complete
        let _ = forward_task.await;

        // Convert ExecOutput to ToolResult
        match result {
            Ok(output) => Ok(exec_output_to_tool_result(output)),
            Err(e) => Ok(ToolResult::error(format!("Execution failed: {e}"))),
        }
    }
}

/// Convert ExecOutput to ToolResult with proper metadata
fn exec_output_to_tool_result(output: ExecOutput) -> ToolResult {
    let mut result_text = String::new();

    if !output.stdout.is_empty() {
        result_text.push_str(&output.stdout);
    }
    if !output.stderr.is_empty() {
        if !result_text.is_empty() {
            result_text.push('\n');
        }
        result_text.push_str(&output.stderr);
    }

    if result_text.is_empty() {
        if output.timed_out {
            result_text = format!("Command timed out after {:?}", output.duration);
        } else {
            result_text = format!("Command completed with exit code {}", output.exit_code);
        }
    }

    let metadata = ToolMetadata {
        duration_ms: output.duration.as_millis() as u64,
        exit_code: Some(output.exit_code),
        files_modified: vec![],
        data: None,
    };

    let result = if output.timed_out {
        ToolResult::error(format!("Command timed out: {result_text}"))
    } else if output.exit_code == 0 {
        ToolResult::success(result_text)
    } else {
        ToolResult::error(format!("Exit code {}: {result_text}", output.exit_code))
    };

    result.with_metadata(metadata)
}
