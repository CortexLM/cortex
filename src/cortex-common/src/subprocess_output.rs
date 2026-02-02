//! Subprocess output handling utilities.
//!
//! Provides clean separation of stdout/stderr from child processes
//! to prevent interleaving with main output.
//!
//! # Issue Addressed
//! - #2795: Child process stderr output interleaves with main output and corrupts JSON

use std::io::{self, BufRead, BufReader};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;

/// Output streams from a subprocess, kept separate.
#[derive(Debug, Clone, Default)]
pub struct SeparatedOutput {
    /// Standard output content
    pub stdout: String,
    /// Standard error content
    pub stderr: String,
    /// Exit code if process completed
    pub exit_code: Option<i32>,
}

impl SeparatedOutput {
    /// Create from process output
    pub fn from_output(output: &Output) -> Self {
        Self {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
        }
    }

    /// Check if there was any stderr output
    pub fn has_stderr(&self) -> bool {
        !self.stderr.is_empty()
    }

    /// Check if the process succeeded (exit code 0)
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Get combined output with clear separation
    pub fn combined_with_labels(&self) -> String {
        let mut result = String::new();

        if !self.stdout.is_empty() {
            result.push_str(&self.stdout);
        }

        if !self.stderr.is_empty() {
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str("[stderr]\n");
            result.push_str(&self.stderr);
        }

        result
    }
}

/// Configure how to handle subprocess output
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// Whether to capture stdout
    pub capture_stdout: bool,
    /// Whether to capture stderr
    pub capture_stderr: bool,
    /// Whether to log stderr to our stderr immediately
    pub log_stderr_immediately: bool,
    /// Prefix to add to logged stderr lines
    pub stderr_prefix: Option<String>,
    /// Maximum bytes to capture per stream
    pub max_capture_bytes: Option<usize>,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            capture_stdout: true,
            capture_stderr: true,
            log_stderr_immediately: false,
            stderr_prefix: None,
            max_capture_bytes: None,
        }
    }
}

impl OutputConfig {
    /// Create new config with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable immediate stderr logging with optional prefix
    pub fn log_stderr(mut self, prefix: Option<String>) -> Self {
        self.log_stderr_immediately = true;
        self.stderr_prefix = prefix;
        self
    }

    /// Set maximum capture size
    pub fn max_bytes(mut self, max: usize) -> Self {
        self.max_capture_bytes = Some(max);
        self
    }
}

/// Spawn a process and capture output with clean separation
///
/// This ensures that stderr from the child process is captured separately
/// and doesn't interleave with our output (which could corrupt JSON).
///
/// # Arguments
/// * `command` - The command to run
///
/// # Returns
/// The separated output from the process.
pub fn run_with_separated_output(mut command: Command) -> io::Result<SeparatedOutput> {
    // Configure command to capture stdout and stderr
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    // Spawn the process
    let output = command.output()?;

    Ok(SeparatedOutput::from_output(&output))
}

/// Spawn a process with streaming output handling
///
/// This allows reading stdout and stderr as they come in, keeping them separate.
///
/// # Arguments
/// * `command` - The command to run
/// * `config` - Output configuration
///
/// # Returns
/// A StreamingProcess handle.
pub fn spawn_with_streaming(
    mut command: Command,
    config: OutputConfig,
) -> io::Result<StreamingProcess> {
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn()?;

    // Take ownership of the streams
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    Ok(StreamingProcess {
        child,
        stdout,
        stderr,
        config,
    })
}

/// Handle for a streaming subprocess
pub struct StreamingProcess {
    child: Child,
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
    config: OutputConfig,
}

impl StreamingProcess {
    /// Wait for the process and collect all output
    pub fn wait_with_output(mut self) -> io::Result<SeparatedOutput> {
        let (stdout_tx, stdout_rx) = mpsc::channel::<String>();
        let (stderr_tx, stderr_rx) = mpsc::channel::<String>();

        // Spawn thread to read stdout
        let stdout_handle = if let Some(stdout) = self.stdout.take() {
            let max_bytes = self.config.max_capture_bytes;
            Some(thread::spawn(move || {
                read_stream_to_channel(stdout, stdout_tx, max_bytes);
            }))
        } else {
            None
        };

        // Spawn thread to read stderr
        let log_immediately = self.config.log_stderr_immediately;
        let prefix = self.config.stderr_prefix.clone();
        let stderr_handle = if let Some(stderr) = self.stderr.take() {
            let max_bytes = self.config.max_capture_bytes;
            Some(thread::spawn(move || {
                read_stderr_to_channel(stderr, stderr_tx, max_bytes, log_immediately, prefix);
            }))
        } else {
            None
        };

        // Wait for process
        let status = self.child.wait()?;

        // Collect stdout
        let mut stdout = String::new();
        for line in stdout_rx.iter() {
            stdout.push_str(&line);
        }

        // Collect stderr
        let mut stderr = String::new();
        for line in stderr_rx.iter() {
            stderr.push_str(&line);
        }

        // Wait for reader threads
        if let Some(h) = stdout_handle {
            let _ = h.join();
        }
        if let Some(h) = stderr_handle {
            let _ = h.join();
        }

        Ok(SeparatedOutput {
            stdout,
            stderr,
            exit_code: status.code(),
        })
    }

    /// Kill the process
    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }

    /// Get the process ID
    pub fn id(&self) -> u32 {
        self.child.id()
    }
}

fn read_stream_to_channel<R: io::Read>(
    reader: R,
    tx: mpsc::Sender<String>,
    max_bytes: Option<usize>,
) {
    let reader = BufReader::new(reader);
    let mut total_bytes = 0;

    for line in reader.lines() {
        match line {
            Ok(line) => {
                let line_with_newline = format!("{}\n", line);
                let bytes = line_with_newline.len();

                if let Some(max) = max_bytes
                    && total_bytes + bytes > max
                {
                    break;
                }

                total_bytes += bytes;
                if tx.send(line_with_newline).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn read_stderr_to_channel<R: io::Read>(
    reader: R,
    tx: mpsc::Sender<String>,
    max_bytes: Option<usize>,
    log_immediately: bool,
    prefix: Option<String>,
) {
    let reader = BufReader::new(reader);
    let mut total_bytes = 0;

    for line in reader.lines() {
        match line {
            Ok(line) => {
                let line_with_newline = format!("{}\n", line);
                let bytes = line_with_newline.len();

                if let Some(max) = max_bytes
                    && total_bytes + bytes > max
                {
                    break;
                }

                total_bytes += bytes;

                // Optionally log to stderr immediately
                if log_immediately {
                    if let Some(ref prefix) = prefix {
                        eprintln!("{}{}", prefix, line);
                    } else {
                        eprintln!("{}", line);
                    }
                }

                if tx.send(line_with_newline).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// Check if output is valid JSON (for validation after subprocess execution)
pub fn is_valid_json(output: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(output).is_ok()
}

/// Clean output that may have been corrupted by interleaved stderr
///
/// Attempts to extract valid JSON from output that may have stderr mixed in.
/// This is a recovery mechanism for when proper separation wasn't used.
///
/// # Arguments
/// * `output` - The potentially corrupted output
///
/// # Returns
/// The cleaned output if JSON could be extracted, or the original output.
pub fn try_clean_json_output(output: &str) -> String {
    // If it's already valid JSON, return as-is
    if is_valid_json(output) {
        return output.to_string();
    }

    // Try to find JSON object boundaries
    if let (Some(start), Some(end)) = (output.find('{'), output.rfind('}')) {
        let potential_json = &output[start..=end];
        if is_valid_json(potential_json) {
            return potential_json.to_string();
        }
    }

    // Try to find JSON array boundaries
    if let (Some(start), Some(end)) = (output.find('['), output.rfind(']')) {
        let potential_json = &output[start..=end];
        if is_valid_json(potential_json) {
            return potential_json.to_string();
        }
    }

    // Couldn't clean, return original
    output.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_separated_output() {
        let output = SeparatedOutput {
            stdout: "output\n".to_string(),
            stderr: "error\n".to_string(),
            exit_code: Some(0),
        };

        assert!(output.has_stderr());
        assert!(output.success());

        let combined = output.combined_with_labels();
        assert!(combined.contains("output"));
        assert!(combined.contains("[stderr]"));
        assert!(combined.contains("error"));
    }

    #[test]
    fn test_run_with_separated_output() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");

        let output = run_with_separated_output(cmd).unwrap();
        assert!(output.stdout.contains("hello"));
        assert!(output.success());
    }

    #[test]
    fn test_is_valid_json() {
        assert!(is_valid_json(r#"{"key": "value"}"#));
        assert!(is_valid_json(r#"[1, 2, 3]"#));
        assert!(!is_valid_json("not json"));
        assert!(!is_valid_json(r#"{"key": "value"} extra"#));
    }

    #[test]
    fn test_try_clean_json_output() {
        // Already valid JSON
        let clean = try_clean_json_output(r#"{"key": "value"}"#);
        assert_eq!(clean, r#"{"key": "value"}"#);

        // JSON with stderr prefix
        let corrupted = r#"Warning: something
{"key": "value"}"#;
        let cleaned = try_clean_json_output(corrupted);
        assert_eq!(cleaned, r#"{"key": "value"}"#);

        // JSON with stderr in the middle (can't clean perfectly)
        let badly_corrupted = r#"{"key
Error: bad
": "value"}"#;
        // This won't parse as valid JSON, so returns original
        let result = try_clean_json_output(badly_corrupted);
        assert_eq!(result, badly_corrupted);
    }
}
