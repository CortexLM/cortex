//! Background terminal process management.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;

/// Maximum lines to keep in the log buffer.
const MAX_LOG_LINES: usize = 10000;

/// A background terminal that runs a shell and captures output.
pub struct BackgroundTerminal {
    /// Terminal ID.
    pub id: String,
    /// Terminal name/description.
    pub name: String,
    /// Working directory.
    pub cwd: String,
    /// The shell process.
    process: Option<Child>,
    /// Log buffer (circular).
    logs: Arc<Mutex<VecDeque<LogLine>>>,
    /// Output sender for real-time streaming.
    output_tx: Option<mpsc::UnboundedSender<LogLine>>,
    /// Creation timestamp.
    pub created_at: u64,
    /// Whether the terminal is running.
    pub running: bool,
    /// Exit code if terminated.
    pub exit_code: Option<i32>,
}

/// A single log line with metadata.
#[derive(Debug, Clone)]
pub struct LogLine {
    /// Timestamp (unix millis).
    pub timestamp: u64,
    /// The log content.
    pub content: String,
    /// Stream type (stdout/stderr).
    pub stream: LogStream,
}

/// Log stream type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
    System,
}

impl BackgroundTerminal {
    /// Create a new background terminal.
    pub fn new(id: String, name: String, cwd: String) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Self {
            id,
            name,
            cwd,
            process: None,
            logs: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG_LINES))),
            output_tx: None,
            created_at: now,
            running: false,
            exit_code: None,
        }
    }

    /// Set the output channel for real-time streaming.
    pub fn set_output_channel(&mut self, tx: mpsc::UnboundedSender<LogLine>) {
        self.output_tx = Some(tx);
    }

    /// Start the terminal with a shell.
    pub fn start(&mut self) -> Result<(), String> {
        if self.running {
            return Err("Terminal already running".to_string());
        }

        let shell = if cfg!(windows) { "cmd" } else { "bash" };
        let shell_args: &[&str] = if cfg!(windows) { &[] } else { &["-i"] };

        let mut cmd = Command::new(shell);
        cmd.args(shell_args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // On Unix, create a new process group so we can kill all children
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Create new session/process group - this terminal will be independent
            unsafe {
                cmd.pre_exec(|| {
                    // Create new session (and process group)
                    libc::setsid();
                    Ok(())
                });
            }
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn shell: {}", e))?;

        // Capture stdout
        if let Some(stdout) = child.stdout.take() {
            let logs = self.logs.clone();
            let tx = self.output_tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    let log_line = LogLine {
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64,
                        content: line,
                        stream: LogStream::Stdout,
                    };

                    // Add to buffer
                    {
                        let mut logs = logs.lock().unwrap();
                        if logs.len() >= MAX_LOG_LINES {
                            logs.pop_front();
                        }
                        logs.push_back(log_line.clone());
                    }

                    // Send to channel
                    if let Some(ref tx) = tx {
                        let _ = tx.send(log_line);
                    }
                }
            });
        }

        // Capture stderr
        if let Some(stderr) = child.stderr.take() {
            let logs = self.logs.clone();
            let tx = self.output_tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    let log_line = LogLine {
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64,
                        content: line,
                        stream: LogStream::Stderr,
                    };

                    {
                        let mut logs = logs.lock().unwrap();
                        if logs.len() >= MAX_LOG_LINES {
                            logs.pop_front();
                        }
                        logs.push_back(log_line.clone());
                    }

                    if let Some(ref tx) = tx {
                        let _ = tx.send(log_line);
                    }
                }
            });
        }

        self.add_system_log(format!("Terminal started in {}", self.cwd));
        self.process = Some(child);
        self.running = true;

        Ok(())
    }

    /// Run a command in the terminal.
    pub fn run_command(&mut self, command: &str) -> Result<(), String> {
        if !self.running {
            return Err("Terminal not running".to_string());
        }

        if let Some(ref mut process) = self.process {
            if let Some(ref mut stdin) = process.stdin {
                use std::io::Write;

                // Handle special control sequences
                let trimmed = command.trim();
                if trimmed == "^C" || trimmed == "\x03" {
                    // Send Ctrl+C (ETX character)
                    stdin
                        .write_all(&[0x03])
                        .map_err(|e| format!("Failed to send Ctrl+C: {}", e))?;
                    stdin
                        .flush()
                        .map_err(|e| format!("Failed to flush: {}", e))?;
                    self.add_system_log("Sent Ctrl+C (SIGINT)".to_string());
                } else if trimmed == "^D" || trimmed == "\x04" {
                    // Send Ctrl+D (EOT character)
                    stdin
                        .write_all(&[0x04])
                        .map_err(|e| format!("Failed to send Ctrl+D: {}", e))?;
                    stdin
                        .flush()
                        .map_err(|e| format!("Failed to flush: {}", e))?;
                    self.add_system_log("Sent Ctrl+D (EOF)".to_string());
                } else if trimmed == "^Z" || trimmed == "\x1a" {
                    // Send Ctrl+Z (SUB character - suspend)
                    stdin
                        .write_all(&[0x1a])
                        .map_err(|e| format!("Failed to send Ctrl+Z: {}", e))?;
                    stdin
                        .flush()
                        .map_err(|e| format!("Failed to flush: {}", e))?;
                    self.add_system_log("Sent Ctrl+Z (SIGTSTP)".to_string());
                } else {
                    // Regular command
                    writeln!(stdin, "{}", command)
                        .map_err(|e| format!("Failed to write command: {}", e))?;
                    stdin
                        .flush()
                        .map_err(|e| format!("Failed to flush: {}", e))?;
                    self.add_system_log(format!("$ {}", command));
                }
                Ok(())
            } else {
                Err("Stdin not available".to_string())
            }
        } else {
            Err("Process not available".to_string())
        }
    }

    /// Send a signal to the terminal process (Unix only).
    #[cfg(unix)]
    pub fn send_signal(&mut self, signal: i32) -> Result<(), String> {
        if let Some(ref process) = self.process {
            let pid = process.id();
            unsafe {
                if libc::kill(pid as i32, signal) == 0 {
                    self.add_system_log(format!("Sent signal {} to process {}", signal, pid));
                    Ok(())
                } else {
                    Err(format!(
                        "Failed to send signal {} to process {}",
                        signal, pid
                    ))
                }
            }
        } else {
            Err("No process".to_string())
        }
    }

    /// Send SIGINT (Ctrl+C) to the terminal process.
    #[cfg(unix)]
    pub fn interrupt(&mut self) -> Result<(), String> {
        self.send_signal(libc::SIGINT)
    }

    /// Send SIGINT (Ctrl+C) to the terminal process (Windows stub).
    #[cfg(not(unix))]
    pub fn interrupt(&mut self) -> Result<(), String> {
        // On Windows, we can try sending Ctrl+C via stdin
        self.run_command("^C")
    }

    /// Get logs (tail N lines).
    pub fn get_logs(&self, tail: usize) -> Vec<LogLine> {
        let logs = self.logs.lock().unwrap();
        let len = logs.len();
        let start = if len > tail { len - tail } else { 0 };
        logs.iter().skip(start).cloned().collect()
    }

    /// Get all logs.
    pub fn get_all_logs(&self) -> Vec<LogLine> {
        let logs = self.logs.lock().unwrap();
        logs.iter().cloned().collect()
    }

    /// Kill the terminal and ALL its child processes.
    pub fn kill(&mut self) -> Result<(), String> {
        let (pid, exit_code) = if let Some(ref mut process) = self.process {
            let pid = process.id();

            #[cfg(unix)]
            {
                // Kill the entire process group with SIGKILL
                // The negative PID means "kill process group"
                unsafe {
                    // First try SIGTERM to allow graceful shutdown
                    libc::kill(-(pid as i32), libc::SIGTERM);
                }

                // Give processes a moment to terminate gracefully
                std::thread::sleep(std::time::Duration::from_millis(100));

                // Then force kill with SIGKILL
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }

            #[cfg(not(unix))]
            {
                // On Windows, just kill the process
                let _ = process.kill();
            }

            // Wait for the process to actually terminate
            let exit_code = process.wait().ok().map(|s| s.code().unwrap_or(-1));
            (Some(pid), exit_code)
        } else {
            return Err("No process to kill".to_string());
        };

        // Update state and log after releasing the borrow
        self.exit_code = exit_code;
        self.running = false;
        if let Some(pid) = pid {
            self.add_system_log(format!("Killed process group {} and all children", pid));
        }
        Ok(())
    }

    /// Check if the process is still running.
    pub fn check_status(&mut self) -> bool {
        if let Some(ref mut process) = self.process {
            match process.try_wait() {
                Ok(Some(status)) => {
                    self.exit_code = status.code();
                    self.running = false;
                    self.add_system_log(format!("Process exited with code {:?}", self.exit_code));
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    self.running = false;
                    false
                }
            }
        } else {
            false
        }
    }

    fn add_system_log(&self, message: String) {
        let log_line = LogLine {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            content: message,
            stream: LogStream::System,
        };

        let mut logs = self.logs.lock().unwrap();
        if logs.len() >= MAX_LOG_LINES {
            logs.pop_front();
        }
        logs.push_back(log_line.clone());

        if let Some(ref tx) = self.output_tx {
            let _ = tx.send(log_line);
        }
    }
}

impl Drop for BackgroundTerminal {
    fn drop(&mut self) {
        if self.running {
            let _ = self.kill();
        }
    }
}
