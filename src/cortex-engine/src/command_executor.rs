//! Command executor utilities.
//!
//! Provides utilities for executing commands with
//! sandboxing, resource limits, and output capture.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::error::{CortexError, Result};

/// Command configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandConfig {
    /// Command to execute.
    pub command: String,
    /// Arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Clear environment.
    #[serde(default)]
    pub clear_env: bool,
    /// Timeout.
    pub timeout_secs: Option<u64>,
    /// Capture stdout.
    #[serde(default = "default_true")]
    pub capture_stdout: bool,
    /// Capture stderr.
    #[serde(default = "default_true")]
    pub capture_stderr: bool,
    /// Shell mode.
    #[serde(default)]
    pub shell: bool,
}

fn default_true() -> bool {
    true
}

impl CommandConfig {
    /// Create a new command config.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            clear_env: false,
            timeout_secs: None,
            capture_stdout: true,
            capture_stderr: true,
            shell: false,
        }
    }

    /// Add argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args
            .extend(args.into_iter().map(std::convert::Into::into));
        self
    }

    /// Set working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Set environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set timeout.
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = Some(secs);
        self
    }

    /// Enable shell mode.
    pub fn shell_mode(mut self) -> Self {
        self.shell = true;
        self
    }
}

/// Command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutput {
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Stdout.
    pub stdout: String,
    /// Stderr.
    pub stderr: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Success.
    pub success: bool,
}

impl CommandOutput {
    /// Create a new output.
    pub fn new() -> Self {
        Self {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            success: false,
        }
    }

    /// Get combined output.
    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    /// Get lines.
    pub fn lines(&self) -> Vec<&str> {
        self.stdout.lines().collect()
    }

    /// Get stderr lines.
    pub fn stderr_lines(&self) -> Vec<&str> {
        self.stderr.lines().collect()
    }
}

impl Default for CommandOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Command executor.
pub struct CommandExecutor {
    /// Default timeout.
    default_timeout: Duration,
    /// Default working directory.
    default_cwd: Option<PathBuf>,
    /// Default environment.
    default_env: HashMap<String, String>,
}

impl CommandExecutor {
    /// Create a new executor.
    pub fn new() -> Self {
        Self {
            default_timeout: Duration::from_secs(300),
            default_cwd: None,
            default_env: HashMap::new(),
        }
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

    /// Set default environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_env.insert(key.into(), value.into());
        self
    }

    /// Execute a command.
    pub async fn execute(&self, config: CommandConfig) -> Result<CommandOutput> {
        let start = std::time::Instant::now();

        let mut cmd = if config.shell {
            let shell = if cfg!(windows) { "cmd" } else { "sh" };
            let shell_arg = if cfg!(windows) { "/C" } else { "-c" };

            let full_command = if config.args.is_empty() {
                config.command.clone()
            } else {
                format!("{} {}", config.command, config.args.join(" "))
            };

            let mut c = Command::new(shell);
            c.arg(shell_arg).arg(&full_command);
            c
        } else {
            let mut c = Command::new(&config.command);
            c.args(&config.args);
            c
        };

        // Set working directory
        if let Some(ref cwd) = config.cwd.or_else(|| self.default_cwd.clone()) {
            cmd.current_dir(cwd);
        }

        // Set environment
        if config.clear_env {
            cmd.env_clear();
        }

        for (key, value) in &self.default_env {
            cmd.env(key, value);
        }
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        // Configure stdio
        // Set stdin to null to prevent interactive prompts from blocking
        cmd.stdin(Stdio::null());

        if config.capture_stdout {
            cmd.stdout(Stdio::piped());
        }
        if config.capture_stderr {
            cmd.stderr(Stdio::piped());
        }

        // Spawn process
        let mut child = cmd.spawn().map_err(|e| CortexError::ToolExecution {
            tool: "command".to_string(),
            message: format!("Failed to spawn: {e}"),
        })?;

        // Get timeout
        let timeout = config
            .timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(self.default_timeout);

        // Wait with timeout
        let result = tokio::time::timeout(timeout, self.wait_for_output(&mut child)).await;

        let mut output = match result {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                // Timeout - kill process
                let _ = child.kill().await;
                return Err(CortexError::Timeout);
            }
        };

        output.duration_ms = start.elapsed().as_millis() as u64;
        Ok(output)
    }

    /// Wait for process output.
    ///
    /// Reads stdout and stderr concurrently to preserve interleaving order (#2743).
    /// This prevents buffering issues where one stream would be fully read
    /// before the other, causing unpredictable output ordering.
    async fn wait_for_output(&self, child: &mut Child) -> Result<CommandOutput> {
        let mut output = CommandOutput::new();

        // Take both streams at once for concurrent reading
        let stdout_opt = child.stdout.take();
        let stderr_opt = child.stderr.take();

        // Read stdout and stderr concurrently to preserve interleaving order
        let (stdout_result, stderr_result) = tokio::join!(
            async {
                let mut lines = Vec::new();
                if let Some(stdout) = stdout_opt {
                    let mut reader = BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        lines.push(line);
                    }
                }
                lines
            },
            async {
                let mut lines = Vec::new();
                if let Some(stderr) = stderr_opt {
                    let mut reader = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        lines.push(line);
                    }
                }
                lines
            }
        );

        // Combine results
        for line in stdout_result {
            output.stdout.push_str(&line);
            output.stdout.push('\n');
        }
        for line in stderr_result {
            output.stderr.push_str(&line);
            output.stderr.push('\n');
        }

        // Wait for exit
        let status = child.wait().await.map_err(|e| CortexError::ToolExecution {
            tool: "command".to_string(),
            message: format!("Failed to wait: {e}"),
        })?;

        output.exit_code = status.code();
        output.success = status.success();

        Ok(output)
    }

    /// Execute a simple command string.
    pub async fn run(&self, command: &str) -> Result<CommandOutput> {
        self.execute(CommandConfig::new(command).shell_mode()).await
    }

    /// Execute and return stdout.
    pub async fn run_output(&self, command: &str) -> Result<String> {
        let output = self.run(command).await?;
        if output.success {
            Ok(output.stdout.trim().to_string())
        } else {
            Err(CortexError::ToolExecution {
                tool: "command".to_string(),
                message: format!("Command failed: {}", output.stderr),
            })
        }
    }

    /// Execute a command with sandboxing.
    ///
    /// This wraps the command with platform-specific sandbox isolation:
    /// - Linux: Landlock + Seccomp via cortex-linux-sandbox
    /// - macOS: Seatbelt (sandbox-exec)
    /// - Windows: Job Objects + ACLs
    pub async fn execute_sandboxed(
        &self,
        config: CommandConfig,
        sandbox_policy: &crate::sandbox::SandboxPolicyType,
    ) -> Result<CommandOutput> {
        use crate::sandbox::SandboxManager;

        let cwd = config.cwd.clone().unwrap_or_else(|| {
            self.default_cwd
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        });

        let manager = SandboxManager::new(sandbox_policy.clone(), cwd.clone());

        // Build command args
        let mut command_args = vec![config.command.clone()];
        command_args.extend(config.args.clone());

        // Prepare sandboxed command
        let sandboxed = manager.prepare_command(&command_args)?;

        // Create new config with sandboxed command
        let mut sandboxed_config = CommandConfig::new(&sandboxed.program);
        sandboxed_config.args = sandboxed.args;
        sandboxed_config.cwd = Some(cwd);
        sandboxed_config.timeout_secs = config.timeout_secs;
        sandboxed_config.capture_stdout = config.capture_stdout;
        sandboxed_config.capture_stderr = config.capture_stderr;

        // Add sandbox environment variables
        for (key, value) in sandboxed.env {
            sandboxed_config.env.insert(key, value);
        }

        // Merge with original environment
        for (key, value) in config.env {
            sandboxed_config.env.insert(key, value);
        }

        self.execute(sandboxed_config).await
    }

    /// Execute a simple command string with sandboxing.
    pub async fn run_sandboxed(
        &self,
        command: &str,
        sandbox_policy: &crate::sandbox::SandboxPolicyType,
    ) -> Result<CommandOutput> {
        self.execute_sandboxed(CommandConfig::new(command).shell_mode(), sandbox_policy)
            .await
    }
}

impl Default for CommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Streaming command executor.
#[allow(dead_code)]
pub struct StreamingExecutor {
    executor: CommandExecutor,
}

impl StreamingExecutor {
    /// Create a new streaming executor.
    pub fn new() -> Self {
        Self {
            executor: CommandExecutor::new(),
        }
    }

    /// Execute with streaming output.
    pub async fn execute(
        &self,
        config: CommandConfig,
    ) -> Result<(
        mpsc::Receiver<OutputLine>,
        tokio::task::JoinHandle<Result<CommandOutput>>,
    )> {
        let (tx, rx) = mpsc::channel(100);

        let mut cmd = if config.shell {
            let shell = if cfg!(windows) { "cmd" } else { "sh" };
            let shell_arg = if cfg!(windows) { "/C" } else { "-c" };

            let full_command = if config.args.is_empty() {
                config.command.clone()
            } else {
                format!("{} {}", config.command, config.args.join(" "))
            };

            let mut c = Command::new(shell);
            c.arg(shell_arg).arg(&full_command);
            c
        } else {
            let mut c = Command::new(&config.command);
            c.args(&config.args);
            c
        };

        if let Some(ref cwd) = config.cwd {
            cmd.current_dir(cwd);
        }

        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| CortexError::ToolExecution {
            tool: "command".to_string(),
            message: format!("Failed to spawn: {e}"),
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let handle = tokio::spawn(async move {
            let mut output = CommandOutput::new();
            let start = std::time::Instant::now();

            // Stream stdout
            if let Some(stdout) = stdout {
                let tx_clone = tx.clone();
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    output.stdout.push_str(&line);
                    output.stdout.push('\n');
                    let _ = tx_clone.send(OutputLine::Stdout(line)).await;
                }
            }

            // Stream stderr
            if let Some(stderr) = stderr {
                let tx_clone = tx.clone();
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    output.stderr.push_str(&line);
                    output.stderr.push('\n');
                    let _ = tx_clone.send(OutputLine::Stderr(line)).await;
                }
            }

            let status = child.wait().await.map_err(|e| CortexError::ToolExecution {
                tool: "command".to_string(),
                message: format!("Failed to wait: {e}"),
            })?;

            output.exit_code = status.code();
            output.success = status.success();
            output.duration_ms = start.elapsed().as_millis() as u64;

            Ok(output)
        });

        Ok((rx, handle))
    }
}

impl Default for StreamingExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Output line type.
#[derive(Debug, Clone)]
pub enum OutputLine {
    Stdout(String),
    Stderr(String),
}

/// Interactive command session.
pub struct InteractiveSession {
    child: Child,
}

impl InteractiveSession {
    /// Start a new session.
    pub async fn start(command: &str) -> Result<Self> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd.spawn().map_err(|e| CortexError::ToolExecution {
            tool: "command".to_string(),
            message: format!("Failed to spawn: {e}"),
        })?;

        Ok(Self { child })
    }

    /// Send input to the process.
    pub async fn send(&mut self, input: &str) -> Result<()> {
        if let Some(ref mut stdin) = self.child.stdin {
            stdin
                .write_all(input.as_bytes())
                .await
                .map_err(CortexError::Io)?;
            stdin.write_all(b"\n").await.map_err(CortexError::Io)?;
            stdin.flush().await.map_err(CortexError::Io)?;
        }
        Ok(())
    }

    /// Read output line.
    pub async fn read_line(&mut self) -> Result<Option<String>> {
        if let Some(ref mut stdout) = self.child.stdout {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();

            match tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line).await {
                Ok(0) => Ok(None),
                Ok(_) => Ok(Some(line.trim_end().to_string())),
                Err(e) => Err(CortexError::Io(e)),
            }
        } else {
            Ok(None)
        }
    }

    /// Kill the process.
    pub async fn kill(&mut self) -> Result<()> {
        self.child
            .kill()
            .await
            .map_err(|e| CortexError::ToolExecution {
                tool: "command".to_string(),
                message: format!("Failed to kill: {e}"),
            })
    }

    /// Wait for completion.
    pub async fn wait(&mut self) -> Result<ExitStatus> {
        self.child
            .wait()
            .await
            .map_err(|e| CortexError::ToolExecution {
                tool: "command".to_string(),
                message: format!("Failed to wait: {e}"),
            })
    }
}

/// Command pipeline.
pub struct Pipeline {
    commands: Vec<CommandConfig>,
}

impl Pipeline {
    /// Create a new pipeline.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// Add a command.
    pub fn pipe(mut self, config: CommandConfig) -> Self {
        self.commands.push(config);
        self
    }

    /// Execute the pipeline.
    pub async fn execute(&self, executor: &CommandExecutor) -> Result<CommandOutput> {
        let mut input = String::new();

        for config in &self.commands {
            let mut cmd = config.clone();
            if !input.is_empty() {
                // Pass previous output as argument (simplified)
                cmd.args.push(input.trim().to_string());
            }

            let output = executor.execute(cmd).await?;
            if !output.success {
                return Ok(output);
            }
            input = output.stdout;
        }

        Ok(CommandOutput {
            stdout: input,
            stderr: String::new(),
            exit_code: Some(0),
            duration_ms: 0,
            success: true,
        })
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_config() {
        let config = CommandConfig::new("echo")
            .arg("hello")
            .arg("world")
            .cwd("/tmp")
            .timeout(10);

        assert_eq!(config.command, "echo");
        assert_eq!(config.args, vec!["hello", "world"]);
        assert_eq!(config.timeout_secs, Some(10));
    }

    #[test]
    fn test_command_output() {
        let output = CommandOutput {
            exit_code: Some(0),
            stdout: "line1\nline2\n".to_string(),
            stderr: String::new(),
            duration_ms: 100,
            success: true,
        };

        assert_eq!(output.lines().len(), 2);
        assert!(output.success);
    }

    #[tokio::test]
    async fn test_executor() {
        let executor = CommandExecutor::new();
        let output = executor.run("echo hello").await.unwrap();

        assert!(output.success);
        assert!(output.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_executor_with_config() {
        let executor = CommandExecutor::new();
        let config = CommandConfig::new("echo").arg("test").shell_mode();

        let output = executor.execute(config).await.unwrap();
        assert!(output.success);
    }
}
