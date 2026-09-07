use std::{
    future::IntoFuture,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use proof_runtime::{bind_private_socket, private_router, RuntimeOperations};
use rustix::{
    io::Errno,
    process::{kill_process_group, test_kill_process_group, Pid, Signal},
};
use serde::Deserialize;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::watch,
    time::Instant,
};
use uuid::Uuid;

use crate::{
    launch::{create_private, private_directory},
    HeadlessInvocation, HeadlessProcess, WorkerError,
};

const MAX_STATUS: u64 = 4096;
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const PROCESS_POLL: Duration = Duration::from_millis(25);
const STATUS_DRAIN: Duration = Duration::from_secs(1);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Counts {
    calls: u64,
    reserved_tokens: u64,
    reserved_micro_usd: u64,
    children: u64,
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum Status {
    Started {
        schema_version: u32,
        restored: bool,
        counts: Counts,
    },
    Stopped {
        schema_version: u32,
        reason: StopReason,
        counts: Counts,
    },
    Failed {
        schema_version: u32,
        code: FailureCode,
        counts: Counts,
    },
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum StopReason {
    Completed,
    Cancelled,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FailureCode {
    InvalidLaunch,
    UnsafePaths,
    ModelConfigUnavailable,
    ReattachmentFailed,
    RuntimeFailed,
    ModelFailed,
    DeadlineExceeded,
    CleanupFailed,
}
impl FailureCode {
    fn label(&self) -> &'static str {
        match self {
            Self::InvalidLaunch => "invalid_launch",
            Self::UnsafePaths => "unsafe_paths",
            Self::ModelConfigUnavailable => "model_config_unavailable",
            Self::ReattachmentFailed => "reattachment_failed",
            Self::RuntimeFailed => "runtime_failed",
            Self::ModelFailed => "model_failed",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::CleanupFailed => "cleanup_failed",
        }
    }
}

struct SocketDirectory(PathBuf);
impl Drop for SocketDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("s"));
        let _ = std::fs::remove_dir(&self.0);
    }
}
struct ProcessGroup {
    child: Option<Child>,
    pid: Option<Pid>,
    shutdown_deadline: Option<Instant>,
}
impl ProcessGroup {
    async fn terminate(&mut self) -> Result<(), WorkerError> {
        let Some(pid) = self.pid else {
            return Ok(());
        };
        let child = self.child.as_mut().ok_or(WorkerError::Unavailable)?;
        let deadline = if let Some(deadline) = self.shutdown_deadline {
            deadline
        } else {
            signal_group(pid, Signal::TERM)?;
            let deadline = Instant::now() + TERMINATION_GRACE;
            self.shutdown_deadline = Some(deadline);
            deadline
        };
        loop {
            // The leader exiting does not mean its descendants have exited.
            if child.try_wait()?.is_some() && !group_exists(pid)? {
                self.pid = None;
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep_until(deadline.min(Instant::now() + PROCESS_POLL)).await;
        }
        signal_group(pid, Signal::KILL)?;
        child.kill().await?;
        self.pid = None;
        Ok(())
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            let _ = kill_process_group(pid, Signal::KILL);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            if matches!(child.try_wait(), Ok(None)) {
                // Tokio's orphan reaper need not run after runtime shutdown.
                let reaper = ChildReaper(child);
                let _ = std::thread::Builder::new()
                    .name("proof-process-reaper".into())
                    .spawn(move || drop(reaper));
            }
        }
    }
}

struct ChildReaper(Child);
impl Drop for ChildReaper {
    fn drop(&mut self) {
        // If thread creation fails, dropping its closure also performs this wait.
        while matches!(self.0.try_wait(), Ok(None)) {
            std::thread::sleep(PROCESS_POLL);
        }
    }
}

impl HeadlessProcess {
    /// Serve attempt-scoped IPC while supervising the complete existing Atlas
    /// launcher. Stdout is bounded status only; stderr never escapes.
    ///
    /// # Errors
    /// Invalid recovery/configuration, shutdown, original deadline or process failure.
    pub async fn run(
        &self,
        invocation: &HeadlessInvocation,
        operations: Arc<dyn RuntimeOperations>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), WorkerError> {
        private_directory(&self.config.private_root)?;
        let remaining = remaining(invocation.deadline_ms)?;
        if invocation.runtime_id.is_nil()
            || invocation.prompt.len() > 128 * 1024
            || *stop.borrow()
            || stop.has_changed().is_err()
        {
            return Err(WorkerError::Interrupted);
        }
        let root = self
            .config
            .private_root
            .join(invocation.runtime_id.to_string());
        create_private(&root)?;
        let socket_dir = self.config.private_root.join(format!(
            "ipc-{}",
            &Uuid::new_v4().simple().to_string()[..16]
        ));
        create_private(&socket_dir)?;
        let socket_dir = SocketDirectory(socket_dir);
        let socket = socket_dir.0.join("s");
        let listener = bind_private_socket(&socket)?;
        let server = axum::serve(listener, private_router(operations)).into_future();
        let input = self.launch_json(invocation, &root, &socket)?;
        let child = self.spawn()?;
        let pid = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw)
            .filter(|pid| pid.as_raw_nonzero().get() > 1)
            .ok_or(WorkerError::Unavailable)?;
        let mut group = ProcessGroup {
            child: Some(child),
            pid: Some(pid),
            shutdown_deadline: None,
        };
        let result = {
            let io = self.communicate(&mut group, input, invocation.resume);
            tokio::pin!(io, server);
            tokio::select! {
                result = &mut io => result,
                _ = &mut server => Err(WorkerError::Unavailable),
                _ = stop.wait_for(|value| *value) => Err(WorkerError::Interrupted),
                () = tokio::time::sleep(remaining) => Err(WorkerError::Invalid),
            }
        };
        group.terminate().await?;
        result
    }

    fn spawn(&self) -> Result<Child, WorkerError> {
        let mut command = match &self.config.pid_namespace {
            Some(unshare) => {
                let mut command = Command::new(unshare);
                // The launcher becomes PID 1 of a private PID namespace: the
                // kernel kills every descendant once it exits or is killed.
                command
                    .arg("--user")
                    .arg("--map-current-user")
                    .arg("--pid")
                    .arg("--fork")
                    .arg("--kill-child=SIGKILL")
                    .arg("--")
                    .arg(&self.config.node);
                command
            }
            None => Command::new(&self.config.node),
        };
        Ok(command
            .arg("--import")
            .arg(&self.config.loader)
            .arg(&self.config.entrypoint)
            .arg("--stdin")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.config.private_root)
            .env("TSX_TSCONFIG_PATH", &self.config.tsconfig)
            .env("PRIME_AGENT_KERNEL_PYTHON", &self.config.kernel_python)
            .env("PYTHONPATH", &self.config.runtime_pythonpath)
            .current_dir(&self.config.private_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?)
    }

    async fn communicate(
        &self,
        group: &mut ProcessGroup,
        input: Vec<u8>,
        resume: bool,
    ) -> Result<(), WorkerError> {
        let child = group.child.as_mut().ok_or(WorkerError::Unavailable)?;
        let mut stdin = child.stdin.take().ok_or(WorkerError::Unavailable)?;
        let stdout = child.stdout.take().ok_or(WorkerError::Unavailable)?;
        let read = async {
            let mut bytes = Vec::new();
            stdout.take(MAX_STATUS + 1).read_to_end(&mut bytes).await?;
            if bytes.len() as u64 > MAX_STATUS {
                return Err(WorkerError::Invalid);
            }
            Ok::<_, WorkerError>(bytes)
        };
        tokio::pin!(read);
        let mut bytes = None;
        let mut input_sent = false;
        let status = {
            let write = async move {
                stdin.write_all(&input).await?;
                stdin.shutdown().await
            };
            tokio::pin!(write);
            loop {
                tokio::select! {
                    biased;
                    result = &mut write, if !input_sent => {
                        result?;
                        input_sent = true;
                    }
                    result = &mut read, if bytes.is_none() => bytes = Some(result?),
                    status = child.wait() => break status?,
                }
            }
        };
        // Stop inherited-pipe owners before draining; never wait for their EOF
        // or their reading stdin before noticing that the launcher has exited.
        group.terminate().await?;
        let bytes = match bytes {
            Some(bytes) => bytes,
            None => tokio::time::timeout(STATUS_DRAIN, read)
                .await
                .map_err(|_| WorkerError::Unavailable)??,
        };
        if !input_sent {
            return Err(WorkerError::Unavailable);
        }
        self.validate_status(&bytes, resume, status.success())
    }

    fn launch_json(
        &self,
        invocation: &HeadlessInvocation,
        root: &std::path::Path,
        socket: &std::path::Path,
    ) -> Result<Vec<u8>, WorkerError> {
        serde_json::to_vec(&json!({
            "schema_version": 1, "runtime_id": invocation.runtime_id, "deadline_ms": invocation.deadline_ms,
            "resume": invocation.resume, "scope": invocation.scope, "controller_socket": socket,
            "state_dir": root.join("state"), "workspace": root.join("workspace"), "sandbox_dir": root.join("sandbox"),
            "model_config_file": self.config.model_config_file, "kernel": self.config.kernel,
            "budget": self.config.budget, "prompt": invocation.prompt,
        })).map_err(|_| WorkerError::Invalid)
    }

    fn validate_status(
        &self,
        bytes: &[u8],
        resume: bool,
        success: bool,
    ) -> Result<(), WorkerError> {
        let events: Vec<Status> = bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice)
            .collect::<Result<_, _>>()
            .map_err(|_| WorkerError::Invalid)?;
        for event in &events {
            let (version, counts) = match event {
                Status::Started {
                    schema_version,
                    counts,
                    ..
                }
                | Status::Stopped {
                    schema_version,
                    counts,
                    ..
                } => (*schema_version, counts),
                Status::Failed {
                    schema_version,
                    counts,
                    code,
                } => {
                    let _ = code;
                    (*schema_version, counts)
                }
            };
            if version != 1
                || counts.calls > u64::from(self.config.budget.max_calls)
                || counts.reserved_tokens > self.config.budget.max_reserved_tokens
                || counts.reserved_micro_usd > self.config.budget.max_reserved_micro_usd
                || counts.children > u64::from(self.config.budget.max_children)
            {
                return Err(WorkerError::Invalid);
            }
        }
        if let Some(Status::Failed { code, .. }) = events.last() {
            return Err(WorkerError::Runtime(code.label()));
        }
        match events.as_slice() {
            [Status::Started { restored, .. }, Status::Stopped {
                reason: StopReason::Completed,
                ..
            }] if success && *restored == resume => Ok(()),
            _ => Err(WorkerError::Unavailable),
        }
    }
}

fn remaining(deadline_ms: i64) -> Result<Duration, WorkerError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| WorkerError::Invalid)?
        .as_millis();
    let deadline = u128::try_from(deadline_ms).map_err(|_| WorkerError::Invalid)?;
    let remaining = deadline
        .checked_sub(now)
        .filter(|ms| *ms > 0 && *ms <= 86_400_000)
        .ok_or(WorkerError::Invalid)?;
    Ok(Duration::from_millis(
        u64::try_from(remaining).map_err(|_| WorkerError::Invalid)?,
    ))
}

fn signal_group(pid: Pid, signal: Signal) -> Result<(), WorkerError> {
    match kill_process_group(pid, signal) {
        Ok(()) | Err(Errno::SRCH) => Ok(()),
        Err(_) => Err(WorkerError::Unavailable),
    }
}

fn group_exists(pid: Pid) -> Result<bool, WorkerError> {
    match test_kill_process_group(pid) {
        Ok(()) => Ok(true),
        Err(Errno::SRCH) => Ok(false),
        Err(_) => Err(WorkerError::Unavailable),
    }
}
