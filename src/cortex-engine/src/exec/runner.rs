//! Command execution.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use cortex_protocol::SandboxPolicy;

use super::{DEFAULT_TIMEOUT, MAX_OUTPUT_SIZE};
use crate::error::Result;

/// Output chunk from streaming execution
#[derive(Debug, Clone)]
pub enum OutputChunk {
    Stdout(String),
    Stderr(String),
}

/// Patterns in variable names that indicate sensitive data (case-insensitive).
/// These will be excluded from the environment passed to child processes.
const SENSITIVE_PATTERNS: &[&str] = &[
    "KEY",        // API_KEY, SSH_KEY, etc.
    "SECRET",     // AWS_SECRET, etc.
    "TOKEN",      // AUTH_TOKEN, etc.
    "PASSWORD",   // DB_PASSWORD, etc.
    "CREDENTIAL", // GOOGLE_CREDENTIALS, etc.
    "PRIVATE",    // PRIVATE_KEY, etc.
];

/// Options for command execution.
#[derive(Debug, Clone)]
pub struct ExecOptions {
    /// Working directory.
    pub cwd: PathBuf,
    /// Timeout.
    pub timeout: Duration,
    /// Sandbox policy.
    pub sandbox_policy: SandboxPolicy,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Whether to capture output.
    pub capture_output: bool,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            timeout: DEFAULT_TIMEOUT,
            sandbox_policy: SandboxPolicy::default(),
            env: HashMap::new(),
            capture_output: true,
        }
    }
}

/// Output from command execution.
#[derive(Debug, Clone)]
pub struct ExecOutput {
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Combined output in order received.
    pub aggregated: String,
    /// Exit code.
    pub exit_code: i32,
    /// Duration.
    pub duration: Duration,
    /// Whether the command timed out.
    pub timed_out: bool,
}

/// Validate that the current working directory exists and is accessible.
/// Returns a clear error message if the CWD is invalid.
fn validate_cwd(cwd: &PathBuf) -> Result<()> {
    if !cwd.exists() {
        return Err(crate::error::CortexError::ToolExecution {
            tool: "exec".to_string(),
            message: format!(
                "Working directory no longer exists: {}. \
                The directory may have been deleted. Please change to a valid directory.",
                cwd.display()
            ),
        });
    }
    if !cwd.is_dir() {
        return Err(crate::error::CortexError::ToolExecution {
            tool: "exec".to_string(),
            message: format!(
                "Working directory path is not a directory: {}",
                cwd.display()
            ),
        });
    }
    Ok(())
}

/// Execute a command.
pub async fn execute_command(command: &[String], options: ExecOptions) -> Result<ExecOutput> {
    if command.is_empty() {
        return Ok(ExecOutput {
            stdout: String::new(),
            stderr: String::new(),
            aggregated: "Empty command".to_string(),
            exit_code: 1,
            duration: Duration::ZERO,
            timed_out: false,
        });
    }

    // Validate CWD before attempting to execute
    validate_cwd(&options.cwd)?;

    let program = &command[0];
    let args = &command[1..];

    let start = Instant::now();

    // Build command
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(&options.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true); // Clean up child process if we're dropped

    // Build filtered environment for security
    cmd.env_clear();
    let safe_env = build_safe_environment(&options.env);
    cmd.envs(safe_env);

    // Unix-specific: set up process group isolation
    #[cfg(unix)]
    {
        #[allow(unused_imports)]
        use std::os::unix::process::CommandExt;
        // SAFETY: setpgid only changes process group, no undefined behavior
        unsafe {
            cmd.pre_exec(|| {
                // Put child in its own process group for clean termination
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    // Spawn process
    let mut child = cmd.spawn().map_err(|e| {
        crate::error::CortexError::tool_execution(program, format!("Failed to spawn: {e}"))
    })?;

    // Read output with timeout
    let result = tokio::time::timeout(options.timeout, async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        if let Some(mut out) = child.stdout.take() {
            let _ = out.read_to_end(&mut stdout).await;
        }
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_end(&mut stderr).await;
        }

        let status = child.wait().await;
        (stdout, stderr, status)
    })
    .await;

    let duration = start.elapsed();

    match result {
        Ok((stdout_bytes, stderr_bytes, status)) => {
            let exit_code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);

            let stdout = truncate_output(&stdout_bytes);
            let stderr = truncate_output(&stderr_bytes);

            let mut aggregated = String::new();
            if !stdout.is_empty() {
                aggregated.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !aggregated.is_empty() {
                    aggregated.push('\n');
                }
                aggregated.push_str(&stderr);
            }

            Ok(ExecOutput {
                stdout,
                stderr,
                aggregated,
                exit_code,
                duration,
                timed_out: false,
            })
        }
        Err(_) => {
            // Timeout - kill the process
            let _ = child.kill().await;

            Ok(ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                aggregated: format!("Command timed out after {:?}", options.timeout),
                exit_code: -1,
                duration,
                timed_out: true,
            })
        }
    }
}

fn truncate_output(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() > MAX_OUTPUT_SIZE {
        format!(
            "{}...\n[Output truncated, {} bytes total]",
            &s[..MAX_OUTPUT_SIZE],
            s.len()
        )
    } else {
        s.to_string()
    }
}

/// Execute a command with streaming output.
/// Sends output chunks via the provided sender as they arrive.
/// Output is interleaved in the order it is received, preserving stdout/stderr ordering.
pub async fn execute_command_streaming(
    command: &[String],
    options: ExecOptions,
    chunk_sender: mpsc::Sender<OutputChunk>,
) -> Result<ExecOutput> {
    if command.is_empty() {
        return Ok(ExecOutput {
            stdout: String::new(),
            stderr: String::new(),
            aggregated: "Empty command".to_string(),
            exit_code: 1,
            duration: Duration::ZERO,
            timed_out: false,
        });
    }

    // Validate CWD before attempting to execute
    validate_cwd(&options.cwd)?;

    let program = &command[0];
    let args = &command[1..];

    let start = Instant::now();

    // Build command
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(&options.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // Build filtered environment
    cmd.env_clear();
    let safe_env = build_safe_environment(&options.env);
    cmd.envs(safe_env);

    // Unix-specific: set up process group isolation
    #[cfg(unix)]
    {
        #[allow(unused_imports)]
        use std::os::unix::process::CommandExt;
        // SAFETY: setpgid only changes process group, no undefined behavior
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    // Spawn process
    let mut child = cmd.spawn().map_err(|e| {
        crate::error::CortexError::tool_execution(program, format!("Failed to spawn: {e}"))
    })?;

    // Take stdout and stderr handles
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    // stdout_acc and stderr_acc are captured inside the timeout block
    // These are used for timeout case if the process is killed

    // Read stdout and stderr with interleaved ordering using select!
    // This preserves the order in which output arrives from the process.
    let result = tokio::time::timeout(options.timeout, async {
        let mut stdout_acc = String::new();
        let mut stderr_acc = String::new();
        let mut aggregated_acc = String::new();

        // Create buffered line readers
        let mut stdout_lines = stdout_handle.map(|h| BufReader::new(h).lines());
        let mut stderr_lines = stderr_handle.map(|h| BufReader::new(h).lines());

        let mut stdout_done = stdout_lines.is_none();
        let mut stderr_done = stderr_lines.is_none();

        // Read lines in the order they become available using select!
        while !stdout_done || !stderr_done {
            tokio::select! {
                biased; // Check stdout first to maintain fairness

                result = async {
                    if let Some(ref mut lines) = stdout_lines {
                        lines.next_line().await
                    } else {
                        std::future::pending().await
                    }
                }, if !stdout_done => {
                    match result {
                        Ok(Some(line)) => {
                            let line_with_newline = format!("{line}\n");
                            let _ = chunk_sender
                                .send(OutputChunk::Stdout(line_with_newline.clone()))
                                .await;
                            stdout_acc.push_str(&line_with_newline);
                            aggregated_acc.push_str(&line_with_newline);
                        }
                        _ => {
                            stdout_done = true;
                        }
                    }
                }

                result = async {
                    if let Some(ref mut lines) = stderr_lines {
                        lines.next_line().await
                    } else {
                        std::future::pending().await
                    }
                }, if !stderr_done => {
                    match result {
                        Ok(Some(line)) => {
                            let line_with_newline = format!("{line}\n");
                            let _ = chunk_sender
                                .send(OutputChunk::Stderr(line_with_newline.clone()))
                                .await;
                            stderr_acc.push_str(&line_with_newline);
                            aggregated_acc.push_str(&line_with_newline);
                        }
                        _ => {
                            stderr_done = true;
                        }
                    }
                }
            }
        }

        let status = child.wait().await;
        (stdout_acc, stderr_acc, aggregated_acc, status)
    })
    .await;

    let duration = start.elapsed();

    match result {
        Ok((stdout, stderr, aggregated, status)) => {
            let exit_code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);

            let stdout_truncated = if stdout.len() > MAX_OUTPUT_SIZE {
                format!(
                    "{}...\n[Output truncated, {} bytes total]",
                    &stdout[..MAX_OUTPUT_SIZE],
                    stdout.len()
                )
            } else {
                stdout
            };

            let stderr_truncated = if stderr.len() > MAX_OUTPUT_SIZE {
                format!(
                    "{}...\n[Output truncated, {} bytes total]",
                    &stderr[..MAX_OUTPUT_SIZE],
                    stderr.len()
                )
            } else {
                stderr
            };

            // Use the already-ordered aggregated output
            let aggregated_truncated = if aggregated.len() > MAX_OUTPUT_SIZE {
                format!(
                    "{}...\n[Output truncated, {} bytes total]",
                    &aggregated[..MAX_OUTPUT_SIZE],
                    aggregated.len()
                )
            } else {
                aggregated
            };

            Ok(ExecOutput {
                stdout: stdout_truncated,
                stderr: stderr_truncated,
                aggregated: aggregated_truncated,
                exit_code,
                duration,
                timed_out: false,
            })
        }
        Err(_) => {
            // Timeout - kill the process
            let _ = child.kill().await;

            Ok(ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                aggregated: format!("Command timed out after {:?}", options.timeout),
                exit_code: -1,
                duration,
                timed_out: true,
            })
        }
    }
}

/// Build a safe environment for command execution.
/// - Inherits ALL environment variables from parent process
/// - Excludes variables containing sensitive patterns (KEY, SECRET, TOKEN, etc.)
/// - Forces non-interactive mode for common tools
/// - Applies any custom overrides from options.env
fn build_safe_environment(overrides: &HashMap<String, String>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars()
        .filter(|(key, _)| {
            // Exclude variables with sensitive patterns (case-insensitive)
            let key_upper = key.to_uppercase();
            !SENSITIVE_PATTERNS
                .iter()
                .any(|pattern| key_upper.contains(pattern))
        })
        .collect();

    // Force non-interactive mode for common tools
    // This prevents commands from hanging waiting for user input
    env.insert("CI".to_string(), "true".to_string()); // npm/yarn/pnpm/create-* use this
    env.insert("DEBIAN_FRONTEND".to_string(), "noninteractive".to_string()); // apt-get
    env.insert("NPM_CONFIG_YES".to_string(), "true".to_string()); // npm auto-yes
    env.insert(
        "YARN_ENABLE_IMMUTABLE_INSTALLS".to_string(),
        "false".to_string(),
    ); // yarn
    env.insert(
        "PNPM_HOME".to_string(),
        "/root/.local/share/pnpm".to_string(),
    ); // pnpm
    env.insert("NO_COLOR".to_string(), "1".to_string()); // disable color codes
    env.insert("TERM".to_string(), "dumb".to_string()); // simple terminal

    // Apply custom overrides
    for (key, value) in overrides {
        env.insert(key.clone(), value.clone());
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_echo() {
        let output = execute_command(
            &["echo".to_string(), "hello".to_string()],
            ExecOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let output = execute_command(
            &["sleep".to_string(), "10".to_string()],
            ExecOptions {
                timeout: Duration::from_millis(100),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert!(output.timed_out);
    }
}
