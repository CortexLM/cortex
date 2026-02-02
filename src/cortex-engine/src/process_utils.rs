//! Process utilities.
//!
//! Provides utilities for process management
//! including spawning, monitoring, and communication.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{CortexError, Result};

/// Process status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessStatus {
    /// Running.
    Running,
    /// Completed successfully.
    Success,
    /// Failed.
    Failed,
    /// Killed.
    Killed,
    /// Timed out.
    TimedOut,
}

/// Process info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Process ID.
    pub pid: u32,
    /// Command.
    pub command: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Status.
    pub status: ProcessStatus,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Start time (unix timestamp).
    pub start_time: u64,
    /// End time (unix timestamp).
    pub end_time: Option<u64>,
    /// Duration (ms).
    pub duration_ms: Option<u64>,
}

impl ProcessInfo {
    /// Create new process info.
    pub fn new(pid: u32, command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            pid,
            command: command.into(),
            args,
            cwd: None,
            status: ProcessStatus::Running,
            exit_code: None,
            start_time: timestamp_now(),
            end_time: None,
            duration_ms: None,
        }
    }

    /// Mark as completed.
    pub fn complete(&mut self, exit_code: i32) {
        self.exit_code = Some(exit_code);
        self.end_time = Some(timestamp_now());
        self.duration_ms = self.end_time.map(|e| (e - self.start_time) * 1000);
        self.status = if exit_code == 0 {
            ProcessStatus::Success
        } else {
            ProcessStatus::Failed
        };
    }

    /// Mark as killed.
    pub fn kill(&mut self) {
        self.end_time = Some(timestamp_now());
        self.duration_ms = self.end_time.map(|e| (e - self.start_time) * 1000);
        self.status = ProcessStatus::Killed;
    }

    /// Mark as timed out.
    pub fn timeout(&mut self) {
        self.end_time = Some(timestamp_now());
        self.duration_ms = self.end_time.map(|e| (e - self.start_time) * 1000);
        self.status = ProcessStatus::TimedOut;
    }

    /// Check if running.
    pub fn is_running(&self) -> bool {
        self.status == ProcessStatus::Running
    }

    /// Check if success.
    pub fn is_success(&self) -> bool {
        self.status == ProcessStatus::Success
    }
}

/// Process manager.
pub struct ProcessManager {
    /// Running processes.
    processes: HashMap<u32, ProcessInfo>,
    /// Max concurrent processes.
    max_concurrent: usize,
    /// Default timeout.
    default_timeout: Duration,
}

impl ProcessManager {
    /// Create a new manager.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            processes: HashMap::new(),
            max_concurrent,
            default_timeout: Duration::from_secs(300),
        }
    }

    /// Set default timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Get running count.
    pub fn running_count(&self) -> usize {
        self.processes.values().filter(|p| p.is_running()).count()
    }

    /// Check if can spawn.
    pub fn can_spawn(&self) -> bool {
        self.running_count() < self.max_concurrent
    }

    /// Spawn a process.
    pub async fn spawn(
        &mut self,
        command: &str,
        args: &[String],
        cwd: Option<&PathBuf>,
    ) -> Result<u32> {
        if !self.can_spawn() {
            return Err(CortexError::Internal(
                "Max concurrent processes reached".to_string(),
            ));
        }

        let mut cmd = Command::new(command);
        cmd.args(args);

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let child = cmd.spawn().map_err(|e| CortexError::ToolExecution {
            tool: "process".to_string(),
            message: e.to_string(),
        })?;

        let pid = child.id().unwrap_or(0);
        let mut info = ProcessInfo::new(pid, command, args.to_vec());
        info.cwd = cwd.cloned();

        self.processes.insert(pid, info);

        Ok(pid)
    }

    /// Get process info.
    pub fn get(&self, pid: u32) -> Option<&ProcessInfo> {
        self.processes.get(&pid)
    }

    /// Get all processes.
    pub fn list(&self) -> Vec<&ProcessInfo> {
        self.processes.values().collect()
    }

    /// Get running processes.
    pub fn running(&self) -> Vec<&ProcessInfo> {
        self.processes.values().filter(|p| p.is_running()).collect()
    }

    /// Update process status.
    pub fn update(&mut self, pid: u32, exit_code: i32) {
        if let Some(info) = self.processes.get_mut(&pid) {
            info.complete(exit_code);
        }
    }

    /// Kill process.
    pub fn kill(&mut self, pid: u32) {
        if let Some(info) = self.processes.get_mut(&pid) {
            info.kill();
        }
    }

    /// Clear completed processes.
    pub fn clear_completed(&mut self) {
        self.processes.retain(|_, p| p.is_running());
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new(10)
    }
}

/// Process pool for parallel execution.
pub struct ProcessPool {
    /// Pool size.
    size: usize,
    /// Running tasks.
    running: usize,
    /// Queue.
    queue: Vec<ProcessTask>,
}

impl ProcessPool {
    /// Create a new pool.
    pub fn new(size: usize) -> Self {
        Self {
            size,
            running: 0,
            queue: Vec::new(),
        }
    }

    /// Submit a task.
    pub fn submit(&mut self, task: ProcessTask) {
        self.queue.push(task);
    }

    /// Get available slots.
    pub fn available(&self) -> usize {
        self.size.saturating_sub(self.running)
    }

    /// Check if has pending tasks.
    pub fn has_pending(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Get next task.
    pub fn next(&mut self) -> Option<ProcessTask> {
        if self.running < self.size && !self.queue.is_empty() {
            self.running += 1;
            Some(self.queue.remove(0))
        } else {
            None
        }
    }

    /// Mark task complete.
    pub fn complete(&mut self) {
        if self.running > 0 {
            self.running -= 1;
        }
    }

    /// Get queue length.
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
}

/// Process task.
#[derive(Debug, Clone)]
pub struct ProcessTask {
    /// Task ID.
    pub id: String,
    /// Command.
    pub command: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Environment.
    pub env: HashMap<String, String>,
    /// Timeout.
    pub timeout: Option<Duration>,
}

impl ProcessTask {
    /// Create a new task.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            id: generate_id(),
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            timeout: None,
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
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Set environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Process result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessResult {
    /// Task ID.
    pub task_id: String,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Stdout.
    pub stdout: String,
    /// Stderr.
    pub stderr: String,
    /// Duration (ms).
    pub duration_ms: u64,
    /// Success.
    pub success: bool,
    /// Error message.
    pub error: Option<String>,
}

impl ProcessResult {
    /// Create success result.
    pub fn success(
        task_id: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
        duration_ms: u64,
    ) -> Self {
        Self {
            task_id,
            exit_code: Some(exit_code),
            stdout,
            stderr,
            duration_ms,
            success: exit_code == 0,
            error: None,
        }
    }

    /// Create error result.
    pub fn error(task_id: String, error: String) -> Self {
        Self {
            task_id,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            success: false,
            error: Some(error),
        }
    }
}

/// Signal type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// SIGTERM.
    Term,
    /// SIGKILL.
    Kill,
    /// SIGINT.
    Int,
    /// SIGHUP.
    Hup,
}

impl Signal {
    /// Get signal number.
    #[cfg(unix)]
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Term => 15,
            Self::Kill => 9,
            Self::Int => 2,
            Self::Hup => 1,
        }
    }

    /// Get signal name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Term => "SIGTERM",
            Self::Kill => "SIGKILL",
            Self::Int => "SIGINT",
            Self::Hup => "SIGHUP",
        }
    }
}

/// Process watcher.
pub struct ProcessWatcher {
    /// Watched PIDs.
    watched: HashMap<u32, ProcessWatch>,
}

impl ProcessWatcher {
    /// Create a new watcher.
    pub fn new() -> Self {
        Self {
            watched: HashMap::new(),
        }
    }

    /// Watch a process.
    pub fn watch(&mut self, pid: u32, on_exit: Option<Box<dyn Fn(i32) + Send>>) {
        self.watched.insert(
            pid,
            ProcessWatch {
                pid,
                on_exit,
                start_time: Instant::now(),
            },
        );
    }

    /// Unwatch a process.
    pub fn unwatch(&mut self, pid: u32) {
        self.watched.remove(&pid);
    }

    /// Get watched count.
    pub fn count(&self) -> usize {
        self.watched.len()
    }

    /// Get watched PIDs.
    pub fn pids(&self) -> Vec<u32> {
        self.watched.keys().copied().collect()
    }
}

impl Default for ProcessWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Process watch entry.
#[allow(dead_code)]
struct ProcessWatch {
    pid: u32,
    on_exit: Option<Box<dyn Fn(i32) + Send>>,
    start_time: Instant,
}

/// Generate unique ID.
fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{now:x}")
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_info() {
        let mut info = ProcessInfo::new(1234, "test", vec!["arg".to_string()]);

        assert!(info.is_running());
        assert!(!info.is_success());

        info.complete(0);

        assert!(!info.is_running());
        assert!(info.is_success());
    }

    #[test]
    fn test_process_manager() {
        let manager = ProcessManager::new(5);

        assert!(manager.can_spawn());
        assert_eq!(manager.running_count(), 0);
    }

    #[test]
    fn test_process_pool() {
        let mut pool = ProcessPool::new(3);

        assert_eq!(pool.available(), 3);

        pool.submit(ProcessTask::new("echo").arg("test"));
        assert!(pool.has_pending());

        let task = pool.next();
        assert!(task.is_some());
        assert_eq!(pool.available(), 2);

        pool.complete();
        assert_eq!(pool.available(), 3);
    }

    #[test]
    fn test_process_task() {
        let task = ProcessTask::new("test")
            .arg("--verbose")
            .arg("file.txt")
            .cwd("/tmp")
            .env("KEY", "value")
            .timeout(Duration::from_secs(60));

        assert_eq!(task.command, "test");
        assert_eq!(task.args.len(), 2);
        assert!(task.cwd.is_some());
        assert!(task.env.contains_key("KEY"));
    }

    #[test]
    fn test_process_result() {
        let success = ProcessResult::success(
            "task1".to_string(),
            0,
            "output".to_string(),
            String::new(),
            100,
        );
        assert!(success.success);

        let error = ProcessResult::error("task2".to_string(), "failed".to_string());
        assert!(!error.success);
    }

    #[test]
    fn test_signal() {
        assert_eq!(Signal::Term.name(), "SIGTERM");
        assert_eq!(Signal::Kill.name(), "SIGKILL");

        #[cfg(unix)]
        {
            assert_eq!(Signal::Term.as_i32(), 15);
            assert_eq!(Signal::Kill.as_i32(), 9);
        }
    }

    #[test]
    fn test_process_watcher() {
        let mut watcher = ProcessWatcher::new();

        watcher.watch(1234, None);
        assert_eq!(watcher.count(), 1);

        watcher.unwatch(1234);
        assert_eq!(watcher.count(), 0);
    }
}
