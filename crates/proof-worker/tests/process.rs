#![allow(clippy::expect_used, clippy::unwrap_used)]

use async_trait::async_trait;
use proof_runtime::{RuntimeCall, RuntimeError, RuntimeOperations, RuntimeScope};
use proof_worker::*;
use rustix::process::{kill_process_group, waitpid, Pid, Signal, WaitOptions};
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{watch, Mutex, Notify};
use uuid::Uuid;

#[derive(Default)]
struct Operations {
    calls: Mutex<Vec<Value>>,
    called: Notify,
}
#[async_trait]
impl RuntimeOperations for Operations {
    async fn call(&self, call: RuntimeCall) -> Result<Value, RuntimeError> {
        self.calls.lock().await.push(call.arguments);
        self.called.notify_one();
        Ok(json!({ "accepted": true }))
    }
}
fn setup() -> (tempfile::TempDir, HeadlessProcess) {
    setup_with(None)
}

fn setup_with(pid_namespace: Option<&str>) -> (tempfile::TempDir, HeadlessProcess) {
    let root = tempfile::Builder::new().prefix("cpx-").tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let executable = root.path().join("launcher");
    std::fs::write(&executable, include_bytes!("fixtures/launcher.py")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = HeadlessConfig {
        node: executable.clone(),
        loader: executable.clone(),
        entrypoint: executable.clone(),
        tsconfig: executable.clone(),
        private_root: root.path().into(),
        model_config_file: executable,
        kernel_python: "/usr/bin/python3".into(),
        runtime_pythonpath: root.path().into(),
        pid_namespace: pid_namespace.map(Into::into),
        kernel: KernelLimits {
            image: format!("sha256:{}", "a".repeat(64)),
            memory_mb: 128,
            workspace_mb: 128,
            cpus: 1,
            pids: 64,
            seconds: 30,
        },
        budget: InferenceBudget {
            max_depth: 1,
            max_children: 1,
            max_concurrent_calls: 1,
            max_calls: 2,
            max_reserved_tokens: 100,
            max_reserved_micro_usd: 1,
            timeout_ms: 30_000,
        },
    };
    (root, HeadlessProcess::new(config).unwrap())
}
fn invocation(mode: &str) -> HeadlessInvocation {
    HeadlessInvocation {
        runtime_id: Uuid::new_v4(),
        deadline_ms: i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
            + 30_000,
        resume: false,
        scope: RuntimeScope {
            role: "experiment".into(),
            id: Uuid::new_v4().to_string(),
            commitment: "b".repeat(64),
        },
        prompt: mode.into(),
    }
}

struct Processes {
    parent: Pid,
    descendant: Pid,
}
impl Processes {
    fn from_report(report: &Value) -> Self {
        let pid = |name: &str| {
            Pid::from_raw(i32::try_from(report[name].as_u64().unwrap()).unwrap()).unwrap()
        };
        Self {
            parent: pid("pid"),
            descendant: pid("descendant"),
        }
    }

    fn parent_reaped(&self) -> bool {
        !std::path::Path::new(&format!("/proc/{}", self.parent.as_raw_nonzero())).exists()
    }

    fn descendant_stopped(&self) -> bool {
        let status =
            std::fs::read_to_string(format!("/proc/{}/status", self.descendant.as_raw_nonzero()));
        status.is_err()
            || status
                .unwrap()
                .lines()
                .any(|line| line.starts_with("State:") && line.contains('Z'))
    }

    async fn assert_stopped(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !self.parent_reaped() || !self.descendant_stopped() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("direct child must be reaped and its descendant terminated");
    }
}
impl Drop for Processes {
    fn drop(&mut self) {
        // Failed regressions must not leave their fixture processes running.
        if !self.parent_reaped() || !self.descendant_stopped() {
            let _ = kill_process_group(self.parent, Signal::KILL);
            let _ = waitpid(Some(self.parent), WaitOptions::empty());
        }
    }
}

async fn reported(operations: &Operations) -> Processes {
    tokio::time::timeout(Duration::from_secs(3), operations.called.notified())
        .await
        .expect("fixture must report its process identities");
    Processes::from_report(&operations.calls.lock().await[0])
}

#[tokio::test]
async fn private_process_transports_identity_deadline_and_only_allowlisted_environment() {
    let (_root, process) = setup();
    let call = invocation("finish");
    let operations = Arc::new(Operations::default());
    let (_stop, signal) = watch::channel(false);
    process
        .run(&call, operations.clone(), signal)
        .await
        .unwrap();
    let calls = operations.calls.lock().await;
    assert_eq!(calls[0]["runtime_id"], call.runtime_id.to_string());
    assert_eq!(calls[0]["deadline_ms"], call.deadline_ms);
    let allowed = [
        "HOME",
        "PATH",
        "TSX_TSCONFIG_PATH",
        "PRIME_AGENT_KERNEL_PYTHON",
        "PYTHONPATH",
        "LC_CTYPE",
    ];
    for name in calls[0]["environment"].as_array().unwrap() {
        assert!(allowed.contains(&name.as_str().unwrap()));
    }
}

#[tokio::test]
async fn malformed_excessive_or_duplicate_status_never_acknowledges_success() {
    let (_root, process) = setup();
    let (_stop, signal) = watch::channel(false);
    for mode in ["oversized", "extra", "wrong_resume"] {
        let error = process
            .run(
                &invocation(mode),
                Arc::new(Operations::default()),
                signal.clone(),
            )
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("sentinel"));
        assert!(!error.to_string().contains("stderr"));
    }
}

#[tokio::test]
async fn shutdown_kills_uncooperative_process_group_with_bounded_grace() {
    let (_root, process) = setup();
    let operations = Arc::new(Operations::default());
    let (stop, signal) = watch::channel(false);
    let call = invocation("ignore");
    let (result, ()) = tokio::time::timeout(Duration::from_secs(12), async {
        tokio::join!(process.run(&call, operations.clone(), signal), async {
            operations.called.notified().await;
            stop.send(true).unwrap();
        })
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(WorkerError::Interrupted)));
    let calls = operations.calls.lock().await;
    for field in ["pid", "descendant"] {
        let pid = calls[0][field].as_u64().unwrap();
        let status = std::fs::read_to_string(format!("/proc/{pid}/status"));
        assert!(
            status.is_err()
                || status
                    .unwrap()
                    .lines()
                    .any(|line| line.starts_with("State:") && line.contains('Z'))
        );
    }
}

#[tokio::test]
async fn exhausted_absolute_deadline_refuses_process_start() {
    let (_root, process) = setup();
    let mut call = invocation("finish");
    call.deadline_ms = 1;
    let operations = Arc::new(Operations::default());
    let (_stop, signal) = watch::channel(false);
    assert!(process
        .run(&call, operations.clone(), signal)
        .await
        .is_err());
    assert!(operations.calls.lock().await.is_empty());
}

#[tokio::test]
async fn successful_child_exit_cleans_descendants_even_after_pipes_close() {
    let (_root, process) = setup();
    let operations = Arc::new(Operations::default());
    let (_stop, signal) = watch::channel(false);
    let call = invocation("orphan_detached");
    let (result, processes) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(12),
            process.run(&call, operations.clone(), signal)
        ),
        reported(&operations),
    );
    result.unwrap().unwrap();
    processes.assert_stopped().await;
}

#[tokio::test]
async fn child_exit_is_not_hidden_by_descendants_holding_stdout() {
    for mode in ["orphan_stdout", "failed_orphan"] {
        let (_root, process) = setup();
        let operations = Arc::new(Operations::default());
        let (_stop, signal) = watch::channel(false);
        let call = invocation(mode);
        let (result, processes) = tokio::join!(
            tokio::time::timeout(
                Duration::from_secs(12),
                process.run(&call, operations.clone(), signal)
            ),
            reported(&operations),
        );
        let result = result.expect("inherited stdout must not extend the launcher's lifetime");
        if mode == "orphan_stdout" {
            result.unwrap();
        } else {
            assert!(matches!(result, Err(WorkerError::Runtime("model_failed"))));
        }
        processes.assert_stopped().await;
    }
}

#[tokio::test]
async fn child_exit_cancels_a_blocked_stdin_write() {
    let (root, process) = setup();
    std::fs::write(root.path().join("hold-stdin"), b"").unwrap();
    let (_stop, signal) = watch::channel(false);
    let call = invocation(&"x".repeat(128 * 1024));
    let (result, processes) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(12),
            process.run(&call, Arc::new(Operations::default()), signal),
        ),
        async {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let Ok(bytes) = std::fs::read(root.path().join("processes.json")) {
                        if let Ok(report) = serde_json::from_slice(&bytes) {
                            break Processes::from_report(&report);
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap()
        },
    );
    assert!(result
        .expect("inherited stdin must not extend the launcher's lifetime")
        .is_err());
    processes.assert_stopped().await;
}

#[tokio::test]
async fn shutdown_escalates_after_child_exits_but_descendant_ignores_term() {
    let (_root, process) = setup();
    let operations = Arc::new(Operations::default());
    let (stop, signal) = watch::channel(false);
    let call = invocation("shutdown_child_exits");
    let (result, processes) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(12),
            process.run(&call, operations.clone(), signal),
        ),
        async {
            let processes = reported(&operations).await;
            stop.send(true).unwrap();
            processes
        },
    );
    assert!(matches!(result.unwrap(), Err(WorkerError::Interrupted)));
    processes.assert_stopped().await;
}

#[tokio::test]
async fn descendants_receive_grace_even_when_the_direct_child_exits() {
    let (root, process) = setup();
    let operations = Arc::new(Operations::default());
    let (stop, signal) = watch::channel(false);
    let call = invocation("shutdown_graceful");
    let (result, processes) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(12),
            process.run(&call, operations.clone(), signal),
        ),
        async {
            let processes = reported(&operations).await;
            stop.send(true).unwrap();
            processes
        },
    );
    assert!(matches!(result.unwrap(), Err(WorkerError::Interrupted)));
    assert!(root.path().join("descendant-cleaned").exists());
    processes.assert_stopped().await;
}

#[tokio::test]
async fn dropping_supervision_kills_the_group_and_reaps_the_child() {
    let (_root, process) = setup();
    let operations = Arc::new(Operations::default());
    let (_stop, signal) = watch::channel(false);
    let call = invocation("ignore");
    let mut running = Box::pin(process.run(&call, operations.clone(), signal));
    let processes = tokio::select! {
        _ = &mut running => panic!("fixture should remain running"),
        processes = reported(&operations) => processes,
    };
    drop(running);
    processes.assert_stopped().await;
}

#[tokio::test]
async fn aborting_supervision_kills_the_group_and_reaps_the_child() {
    let (_root, process) = setup();
    let operations = Arc::new(Operations::default());
    let (_stop, signal) = watch::channel(false);
    let task_operations = operations.clone();
    let running = tokio::spawn(async move {
        process
            .run(&invocation("ignore"), task_operations, signal)
            .await
    });
    let processes = reported(&operations).await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    processes.assert_stopped().await;
}

#[tokio::test]
async fn stop_during_descendant_cleanup_does_not_restart_the_grace_period() {
    let (_root, process) = setup();
    let operations = Arc::new(Operations::default());
    let (stop, signal) = watch::channel(false);
    let call = invocation("orphan_stdout");
    let started = Instant::now();
    let (result, processes) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(12),
            process.run(&call, operations.clone(), signal),
        ),
        async {
            let processes = reported(&operations).await;
            tokio::time::sleep(Duration::from_secs(4)).await;
            assert!(processes.parent_reaped());
            assert!(!processes.descendant_stopped());
            stop.send(true).unwrap();
            processes
        },
    );
    assert!(matches!(result.unwrap(), Err(WorkerError::Interrupted)));
    assert!(started.elapsed() < Duration::from_secs(8));
    processes.assert_stopped().await;
}

#[tokio::test]
async fn aborting_during_termination_still_kills_descendants() {
    let (_root, process) = setup();
    let operations = Arc::new(Operations::default());
    let (stop, signal) = watch::channel(false);
    let task_operations = operations.clone();
    let running = tokio::spawn(async move {
        process
            .run(&invocation("shutdown_child_exits"), task_operations, signal)
            .await
    });
    let processes = reported(&operations).await;
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !processes.parent_reaped() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!processes.descendant_stopped());
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    processes.assert_stopped().await;
}

/// Host pids of live (non-zombie) processes whose argv carries `marker`.
fn host_processes_with_marker(marker: &str) -> Vec<u32> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if !cmdline
            .windows(marker.len())
            .any(|w| w == marker.as_bytes())
        {
            continue;
        }
        let zombie = std::fs::read_to_string(entry.path().join("status")).map_or(true, |s| {
            s.lines()
                .any(|line| line.starts_with("State:") && line.contains('Z'))
        });
        if !zombie {
            found.push(pid);
        }
    }
    found
}

async fn escaped_descendant_after_stop(pid_namespace: Option<&str>) -> Vec<u32> {
    let (_root, process) = setup_with(pid_namespace);
    let operations = Arc::new(Operations::default());
    let (stop, signal) = watch::channel(false);
    let call = invocation("escape_group");
    let (result, marker) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(12),
            process.run(&call, operations.clone(), signal),
        ),
        async {
            tokio::time::timeout(Duration::from_secs(3), operations.called.notified())
                .await
                .unwrap();
            let marker = operations.calls.lock().await[0]["marker"]
                .as_str()
                .unwrap()
                .to_owned();
            assert_eq!(host_processes_with_marker(&marker).len(), 1);
            stop.send(true).unwrap();
            marker
        },
    );
    assert!(matches!(result.unwrap(), Err(WorkerError::Interrupted)));
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut remaining = host_processes_with_marker(&marker);
    while !remaining.is_empty() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
        remaining = host_processes_with_marker(&marker);
    }
    for pid in &remaining {
        let _ = kill_process_group(
            Pid::from_raw(i32::try_from(*pid).unwrap()).unwrap(),
            Signal::KILL,
        );
        let _ = rustix::process::kill_process(
            Pid::from_raw(i32::try_from(*pid).unwrap()).unwrap(),
            Signal::KILL,
        );
    }
    remaining
}

#[tokio::test]
async fn process_group_supervision_alone_cannot_stop_a_setsid_escape() {
    // Documents the containment limit the PID namespace closes.
    assert_eq!(escaped_descendant_after_stop(None).await.len(), 1);
}

#[tokio::test]
async fn pid_namespace_kills_descendants_that_escape_the_process_group() {
    let unshare = "/usr/bin/unshare";
    if !std::path::Path::new(unshare).exists() {
        eprintln!("skipped: {unshare} unavailable");
        return;
    }
    assert!(escaped_descendant_after_stop(Some(unshare))
        .await
        .is_empty());
}

#[tokio::test]
async fn pid_namespace_preserves_ipc_status_and_allowlisted_environment() {
    let unshare = "/usr/bin/unshare";
    if !std::path::Path::new(unshare).exists() {
        eprintln!("skipped: {unshare} unavailable");
        return;
    }
    let (_root, process) = setup_with(Some(unshare));
    let call = invocation("finish");
    let operations = Arc::new(Operations::default());
    let (_stop, signal) = watch::channel(false);
    process
        .run(&call, operations.clone(), signal)
        .await
        .unwrap();
    let calls = operations.calls.lock().await;
    assert_eq!(calls[0]["runtime_id"], call.runtime_id.to_string());
    assert_eq!(
        calls[0]["pid"], 1,
        "launcher must be PID 1 of its namespace"
    );
}

#[test]
fn runtime_shutdown_still_reaps_the_direct_child() {
    let (_root, process) = setup();
    let operations = Arc::new(Operations::default());
    let (_stop, signal) = watch::channel(false);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let processes = runtime.block_on(async {
        let task_operations = operations.clone();
        tokio::spawn(async move {
            process
                .run(&invocation("ignore"), task_operations, signal)
                .await
        });
        reported(&operations).await
    });
    drop(runtime);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline
        && (!processes.parent_reaped() || !processes.descendant_stopped())
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(processes.descendant_stopped());
    assert!(
        processes.parent_reaped(),
        "child reaping must not depend on another Tokio runtime being polled"
    );
}
