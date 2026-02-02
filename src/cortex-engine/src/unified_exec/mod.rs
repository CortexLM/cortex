//! Unified execution system.
//!
//! Provides a unified interface for executing commands, tools, and
//! scripts with consistent error handling, timeouts, and output capture.

pub mod errors;
pub mod session;

pub use errors::{ExecError, ExecErrorKind};
pub use session::{ExecSession, ExecSessionManager, SessionConfig};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::error::{CortexError, Result};
use crate::shell::ShellType;

/// Execution request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRequest {
    /// Command to execute.
    pub command: String,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Timeout.
    pub timeout: Option<Duration>,
    /// Shell type to use.
    pub shell: Option<ShellType>,
    /// Capture stdout.
    pub capture_stdout: bool,
    /// Capture stderr.
    pub capture_stderr: bool,
    /// Stream output.
    pub stream: bool,
    /// Stdin input.
    pub stdin: Option<String>,
}

impl ExecRequest {
    /// Create a new execution request.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            cwd: None,
            env: HashMap::new(),
            timeout: Some(Duration::from_secs(300)),
            shell: None,
            capture_stdout: true,
            capture_stderr: true,
            stream: false,
            stdin: None,
        }
    }

    /// Set working directory.
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Add environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set shell type.
    pub fn shell(mut self, shell: ShellType) -> Self {
        self.shell = Some(shell);
        self
    }

    /// Enable streaming.
    pub fn stream(mut self) -> Self {
        self.stream = true;
        self
    }

    /// Set stdin.
    pub fn stdin(mut self, input: impl Into<String>) -> Self {
        self.stdin = Some(input.into());
        self
    }
}

/// Execution result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    /// Exit code.
    pub exit_code: i32,
    /// Stdout output.
    pub stdout: String,
    /// Stderr output.
    pub stderr: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the command timed out.
    pub timed_out: bool,
    /// Whether the command was killed.
    pub killed: bool,
}

impl ExecResult {
    /// Check if execution succeeded.
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out && !self.killed
    }

    /// Get combined output.
    pub fn output(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }

    /// Get trimmed stdout.
    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim()
    }

    /// Get trimmed stderr.
    pub fn stderr_trimmed(&self) -> &str {
        self.stderr.trim()
    }
}

/// Output event during streaming.
#[derive(Debug, Clone)]
pub enum OutputEvent {
    /// Stdout line.
    Stdout(String),
    /// Stderr line.
    Stderr(String),
    /// Process started with PID.
    Started(u32),
    /// Process exited with code.
    Exited(i32),
    /// Error occurred.
    Error(String),
}

/// Unified executor.
pub struct UnifiedExecutor {
    /// Default shell.
    default_shell: ShellType,
    /// Default timeout.
    default_timeout: Duration,
    /// Default working directory.
    default_cwd: Option<PathBuf>,
    /// Environment variables to always include.
    default_env: HashMap<String, String>,
}

impl UnifiedExecutor {
    /// Create a new executor.
    pub fn new() -> Self {
        Self {
            default_shell: ShellType::detect(),
            default_timeout: Duration::from_secs(300),
            default_cwd: None,
            default_env: HashMap::new(),
        }
    }

    /// Set default shell.
    pub fn with_shell(mut self, shell: ShellType) -> Self {
        self.default_shell = shell;
        self
    }

    /// Set default timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Set default working directory.
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.default_cwd = Some(cwd.into());
        self
    }

    /// Add default environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_env.insert(key.into(), value.into());
        self
    }

    /// Execute a command.
    pub async fn execute(&self, request: ExecRequest) -> Result<ExecResult> {
        let shell = request.shell.unwrap_or(self.default_shell);
        let timeout = request.timeout.unwrap_or(self.default_timeout);
        let cwd = request.cwd.as_ref().or(self.default_cwd.as_ref());

        // Build command
        let (program, args) = self.build_shell_command(&shell, &request.command);

        let mut cmd = Command::new(&program);
        cmd.args(&args);

        // Set working directory
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        // Set environment
        for (key, value) in &self.default_env {
            cmd.env(key, value);
        }
        for (key, value) in &request.env {
            cmd.env(key, value);
        }

        // Configure stdio
        if request.capture_stdout {
            cmd.stdout(Stdio::piped());
        }
        if request.capture_stderr {
            cmd.stderr(Stdio::piped());
        }
        if request.stdin.is_some() {
            cmd.stdin(Stdio::piped());
        } else {
            // Prevent interactive prompts from blocking
            cmd.stdin(Stdio::null());
        }

        let start = std::time::Instant::now();

        // Spawn process
        let mut child = cmd.spawn().map_err(CortexError::Io)?;

        // Write stdin if provided
        if let Some(input) = &request.stdin
            && let Some(mut stdin) = child.stdin.take()
        {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(input.as_bytes()).await?;
        }

        // Wait with timeout
        let result = if request.stream {
            self.wait_streaming(child, timeout).await
        } else {
            self.wait_buffered(child, timeout).await
        };

        let duration = start.elapsed();

        match result {
            Ok((exit_code, stdout, stderr, timed_out, killed)) => Ok(ExecResult {
                exit_code,
                stdout,
                stderr,
                duration_ms: duration.as_millis() as u64,
                timed_out,
                killed,
            }),
            Err(e) => Err(e),
        }
    }

    /// Execute with streaming output.
    pub async fn execute_streaming(
        &self,
        request: ExecRequest,
    ) -> Result<(
        mpsc::Receiver<OutputEvent>,
        tokio::task::JoinHandle<ExecResult>,
    )> {
        let shell = request.shell.unwrap_or(self.default_shell);
        let _timeout = request.timeout.unwrap_or(self.default_timeout);
        let cwd = request.cwd.clone().or_else(|| self.default_cwd.clone());

        let (program, args) = self.build_shell_command(&shell, &request.command);

        let mut cmd = Command::new(&program);
        cmd.args(&args);

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        for (key, value) in &self.default_env {
            cmd.env(key, value);
        }
        for (key, value) in &request.env {
            cmd.env(key, value);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(CortexError::Io)?;

        let (tx, rx) = mpsc::channel(100);

        let pid = child.id().unwrap_or(0);
        let _ = tx.send(OutputEvent::Started(pid)).await;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let handle = tokio::spawn(async move {
            let start = std::time::Instant::now();
            let mut stdout_content = String::new();
            let mut stderr_content = String::new();

            // Stream stdout
            if let Some(stdout) = stdout {
                let tx = tx.clone();
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    stdout_content.push_str(&line);
                    stdout_content.push('\n');
                    let _ = tx.send(OutputEvent::Stdout(line)).await;
                }
            }

            // Stream stderr
            if let Some(stderr) = stderr {
                let tx = tx.clone();
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    stderr_content.push_str(&line);
                    stderr_content.push('\n');
                    let _ = tx.send(OutputEvent::Stderr(line)).await;
                }
            }

            // Wait for exit
            let exit_status = child.wait().await;
            let duration = start.elapsed();

            let exit_code = exit_status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            let _ = tx.send(OutputEvent::Exited(exit_code)).await;

            ExecResult {
                exit_code,
                stdout: stdout_content,
                stderr: stderr_content,
                duration_ms: duration.as_millis() as u64,
                timed_out: false,
                killed: false,
            }
        });

        Ok((rx, handle))
    }

    /// Build shell command.
    fn build_shell_command(&self, shell: &ShellType, command: &str) -> (String, Vec<String>) {
        match shell {
            ShellType::Bash => (
                "bash".to_string(),
                vec!["-c".to_string(), command.to_string()],
            ),
            ShellType::Zsh => (
                "zsh".to_string(),
                vec!["-c".to_string(), command.to_string()],
            ),
            ShellType::Fish => (
                "fish".to_string(),
                vec!["-c".to_string(), command.to_string()],
            ),
            ShellType::PowerShell => {
                #[cfg(windows)]
                {
                    (
                        "powershell.exe".to_string(),
                        vec!["-Command".to_string(), command.to_string()],
                    )
                }
                #[cfg(not(windows))]
                {
                    (
                        "pwsh".to_string(),
                        vec!["-Command".to_string(), command.to_string()],
                    )
                }
            }
            ShellType::Cmd => (
                "cmd.exe".to_string(),
                vec!["/c".to_string(), command.to_string()],
            ),
            _ => (
                "sh".to_string(),
                vec!["-c".to_string(), command.to_string()],
            ),
        }
    }

    /// Wait for process with buffered output.
    async fn wait_buffered(
        &self,
        child: tokio::process::Child,
        timeout: Duration,
    ) -> Result<(i32, String, String, bool, bool)> {
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

        match result {
            Ok(Ok(output)) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                Ok((exit_code, stdout, stderr, false, false))
            }
            Ok(Err(e)) => Err(CortexError::Io(e)),
            Err(_) => {
                // Timeout - we can't kill since we already passed ownership
                Ok((
                    -1,
                    String::new(),
                    "Command timed out".to_string(),
                    true,
                    true,
                ))
            }
        }
    }

    /// Wait for process with streaming output.
    async fn wait_streaming(
        &self,
        mut child: tokio::process::Child,
        timeout: Duration,
    ) -> Result<(i32, String, String, bool, bool)> {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let mut stdout_content = String::new();
        let mut stderr_content = String::new();

        // Read stdout
        if let Some(stdout) = stdout {
            let mut reader = BufReader::new(stdout);
            let mut buf = String::new();
            while reader.read_line(&mut buf).await.unwrap_or(0) > 0 {
                stdout_content.push_str(&buf);
                buf.clear();
            }
        }

        // Read stderr
        if let Some(stderr) = stderr {
            let mut reader = BufReader::new(stderr);
            let mut buf = String::new();
            while reader.read_line(&mut buf).await.unwrap_or(0) > 0 {
                stderr_content.push_str(&buf);
                buf.clear();
            }
        }

        let result = tokio::time::timeout(timeout, child.wait()).await;

        match result {
            Ok(Ok(status)) => {
                let exit_code = status.code().unwrap_or(-1);
                Ok((exit_code, stdout_content, stderr_content, false, false))
            }
            Ok(Err(e)) => Err(CortexError::Io(e)),
            Err(_) => {
                let _ = child.kill().await;
                Ok((
                    -1,
                    stdout_content,
                    "Command timed out".to_string(),
                    true,
                    true,
                ))
            }
        }
    }
}

impl Default for UnifiedExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Quick execution helper.
pub async fn exec(command: &str) -> Result<ExecResult> {
    UnifiedExecutor::new()
        .execute(ExecRequest::new(command))
        .await
}

/// Quick execution with working directory.
pub async fn exec_in(command: &str, cwd: impl AsRef<Path>) -> Result<ExecResult> {
    UnifiedExecutor::new()
        .execute(ExecRequest::new(command).cwd(cwd.as_ref()))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_exec_simple() {
        let result = exec("echo hello").await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_exec_exit_code() {
        let result = exec("exit 1").await.unwrap();
        assert!(!result.success());
        assert_eq!(result.exit_code, 1);
    }

    #[tokio::test]
    #[cfg_attr(
        windows,
        ignore = "Unix shell variable syntax not available on Windows"
    )]
    async fn test_exec_with_env() {
        let result = UnifiedExecutor::new()
            .execute(ExecRequest::new("echo $TEST_VAR").env("TEST_VAR", "hello"))
            .await
            .unwrap();
        assert!(result.stdout.contains("hello"));
    }

    #[test]
    fn test_exec_request_builder() {
        let request = ExecRequest::new("ls")
            .cwd("/tmp")
            .env("FOO", "bar")
            .timeout(Duration::from_secs(10));

        assert_eq!(request.command, "ls");
        assert_eq!(request.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(request.env.get("FOO"), Some(&"bar".to_string()));
    }
}
