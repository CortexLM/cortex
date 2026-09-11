//! Reconstruct a paid Proof VM job from guest artefacts on the RW scratch
//! when vsock drops `Done`.
//!
//! Harbor writes `report.json` under `/var/lib/proof/work/<seq>-{evaluate,
//! baseline}/output/` on `scratch.ext4` before the guest sends `Done`. The
//! host sees that file as:
//! 1. `{jail_root}/scratch-tree/work/…` (host overlay of guest `/var/lib/proof`)
//! 2. else `debugfs -c -R 'rdump /work …'` on `{jail_root}/scratch.ext4`
//!
//! Only the highest-numbered matching work directory counts. Called from
//! `proof-fc-host` after `Run` when recv fails — tips via `BUILD_FROM=source`
//! of `proof-vm-orchestrator`, no guest rebake.

#![allow(clippy::missing_errors_doc)]

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use proof_rlm::{
    CustomRunReport, CustomRunRequest, LogFile, RunOutcome, VmJob, VmJobOutput, RUN_REPORT_SCHEMA,
};
use proof_vm_agent::HvError;
use proof_vm_proto::guest::RlmToHost;
use serde::Deserialize;

/// Overlay of guest `/var/lib/proof` beside the virtio drives.
pub const SCRATCH_TREE: &str = "scratch-tree";
/// Jail-relative scratch image (guest mounts it at `/var/lib/proof`).
pub const SCRATCH_IN_JAIL: &str = "scratch.ext4";
/// Serial console capture beside the jail.
pub const CONSOLE_LOG: &str = "console.log";
const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_REPORT_BYTES: u64 = 8 * 1024 * 1024;
/// How often a silent vsock is peeked for a scratch report.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// After a report is on disk, wait this long for a real `Done`.
pub const DONE_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
struct GuestReport {
    primary_value: f64,
    #[serde(default)]
    claim_holds: bool,
    #[serde(default)]
    flops_used: Option<u64>,
    #[serde(default)]
    evidence: BTreeMap<String, serde_json::Value>,
}

/// Recv the job answer, or reconstruct it from scratch when vsock dies
/// after `Run` was sent.
///
/// `recv` is **one** vsock-frame future, kept alive for the whole wait.
/// Framing (`read_exact` of a 4-byte length, then the body) is not
/// cancellation-safe: wrapping `recv` in `timeout` and calling it again
/// on the same stream drops a partial header/body and the next read
/// mis-parses length. Scratch and the job deadline are polled beside
/// that future; they never recreate it. Passing `GuestChannel::recv()`
/// from `proof-fc-host` is the live path.
pub async fn recv_job_or_harvest<F>(
    recv: F,
    jail_root: &Path,
    jail_dir: &Path,
    job: &VmJob,
    budget: Duration,
) -> Result<RlmToHost, HvError>
where
    F: Future<Output = Result<RlmToHost, HvError>>,
{
    let deadline = tokio::time::Instant::now() + budget;
    let mut recv = std::pin::pin!(recv);
    let mut report_seen_at: Option<tokio::time::Instant> = None;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return or_err(
                jail_root,
                jail_dir,
                job,
                HvError::Deadline(budget.as_secs()),
            );
        }
        if let Some(seen) = report_seen_at {
            if now.saturating_duration_since(seen) >= DONE_GRACE {
                if let Some(output) = try_from_jail(jail_root, jail_dir, job) {
                    tracing::warn!(
                        "vsock silent after a guest report landed on scratch; harvesting after {:?}",
                        DONE_GRACE
                    );
                    return Ok(RlmToHost::Done { output });
                }
                report_seen_at = None;
            }
        }
        let slice = (deadline - now).min(POLL_INTERVAL);
        tokio::select! {
            biased;
            result = &mut recv => {
                return match result {
                    Ok(msg) => Ok(msg),
                    Err(e) => or_err(jail_root, jail_dir, job, e),
                };
            }
            () = tokio::time::sleep(slice) => {
                if report_seen_at.is_none() && try_from_jail(jail_root, jail_dir, job).is_some() {
                    tracing::warn!(
                        "vsock silent after a guest report landed on scratch; waiting {:?} for Done then harvesting",
                        DONE_GRACE
                    );
                    report_seen_at = Some(tokio::time::Instant::now());
                }
            }
        }
    }
}

fn or_err(
    jail_root: &Path,
    jail_dir: &Path,
    job: &VmJob,
    err: HvError,
) -> Result<RlmToHost, HvError> {
    match try_from_jail(jail_root, jail_dir, job) {
        Some(output) => {
            tracing::warn!(
                "vsock lost the Done frame ({err}); harvested guest report from scratch"
            );
            Ok(RlmToHost::Done { output })
        }
        None => Err(err),
    }
}

/// Reconstruct a paid job from the jail's scratch. `None` = nothing to take.
#[must_use]
pub fn try_from_jail(jail_root: &Path, jail_dir: &Path, job: &VmJob) -> Option<VmJobOutput> {
    let work = work_root(jail_root, jail_dir)?;
    from_work_tree(&work, jail_dir, job).ok()
}

fn work_root(jail_root: &Path, jail_dir: &Path) -> Option<PathBuf> {
    let overlay = jail_root.join(SCRATCH_TREE).join("work");
    if overlay.is_dir() {
        return Some(overlay);
    }
    let image = jail_root.join(SCRATCH_IN_JAIL);
    if !image.is_file() {
        return None;
    }
    let dest = jail_dir.join("harvest-work");
    dump_ext4_work(&image, &dest).ok()?;
    dest.is_dir().then_some(dest)
}

fn dump_ext4_work(image: &Path, dest: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    let spec = format!("rdump /work {}", dest.display());
    let out = std::process::Command::new("debugfs")
        .args(["-c", "-R", &spec, &image.display().to_string()])
        .output()
        .map_err(|e| format!("debugfs: {e}"))?;
    if dest.read_dir().ok().and_then(|mut d| d.next()).is_none() {
        let tail = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "debugfs dumped nothing from {} ({})",
            image.display(),
            tail.chars().rev().take(200).collect::<String>()
        ));
    }
    Ok(())
}

/// Build [`VmJobOutput`] from a guest `work/` tree already on the host.
pub fn from_work_tree(
    work_root: &Path,
    jail_dir: &Path,
    job: &VmJob,
) -> Result<VmJobOutput, HvError> {
    let (kind, request) = match job {
        VmJob::Evaluate { request, .. } => ("evaluate", request),
        VmJob::Baseline { request } => ("baseline", request),
        VmJob::Inspect { .. } | VmJob::ProposeRules { .. } | VmJob::Archive { .. } => {
            return Err(HvError::Guest(
                "scratch harvest is for baseline/evaluate only".into(),
            ));
        }
    };
    let report_path = find_report(work_root, kind).ok_or_else(|| {
        HvError::Guest(format!(
            "no {}/output/report.json in {}",
            kind,
            work_root.display()
        ))
    })?;
    let run = reconstruct(request, &report_path, work_root, jail_dir)?;
    match kind {
        "baseline" => Ok(VmJobOutput::Baseline(run.report)),
        _ => Ok(VmJobOutput::Evaluated(run)),
    }
}

fn find_report(work_root: &Path, kind: &str) -> Option<PathBuf> {
    let suffix = format!("-{kind}");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(work_root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(suffix.as_str()) && e.path().is_dir())
        .map(|e| e.path())
        .collect();
    dirs.sort();
    let last = dirs.pop()?;
    let report = last.join("output").join("report.json");
    report.is_file().then_some(report)
}

fn reconstruct(
    request: &CustomRunRequest,
    report_path: &Path,
    work_root: &Path,
    jail_dir: &Path,
) -> Result<RunOutcome, HvError> {
    let meta = std::fs::metadata(report_path)
        .map_err(|e| HvError::Guest(format!("harvest report {}: {e}", report_path.display())))?;
    if meta.len() == 0 || meta.len() > MAX_REPORT_BYTES {
        return Err(HvError::Guest(format!(
            "harvest report {} is {} bytes",
            report_path.display(),
            meta.len()
        )));
    }
    let body = std::fs::read_to_string(report_path)
        .map_err(|e| HvError::Guest(format!("harvest report: {e}")))?;
    let guest: GuestReport = serde_json::from_str(&body)
        .map_err(|e| HvError::Guest(format!("harvest report did not parse: {e}")))?;
    if !guest.primary_value.is_finite() {
        return Err(HvError::Guest(
            "harvest report primary_value is not finite".into(),
        ));
    }
    let mut evidence = guest.evidence;
    if let Ok(Some(binding)) = request.experiment() {
        evidence
            .entry("runner".into())
            .or_insert_with(|| serde_json::json!(binding.runner));
        evidence
            .entry("pack_digest".into())
            .or_insert_with(|| serde_json::json!(binding.pack.digest));
    }
    evidence
        .entry("host_harvest".into())
        .or_insert_with(|| serde_json::json!("vsock_done_drop"));
    let report = CustomRunReport {
        schema_version: RUN_REPORT_SCHEMA,
        topic_id: request.topic_id.clone(),
        custom_id: request.custom_id.clone(),
        submission_digest: request.submission_digest.clone(),
        artifact_digest: request.artifact_digest.clone(),
        rules_version: request.rules_version,
        primary_value: guest.primary_value,
        claim_holds: guest.claim_holds,
        sandboxed: true,
        flops_used: guest.flops_used,
        evidence,
    };
    report
        .verify(request)
        .map_err(|e| HvError::Guest(format!("harvest report: {e}")))?;
    Ok(RunOutcome {
        report,
        logs: collect_logs(work_root, jail_dir),
    })
}

fn collect_logs(work_root: &Path, jail_dir: &Path) -> Vec<LogFile> {
    let mut logs = Vec::new();
    push_log(&mut logs, "console.log", &jail_dir.join(CONSOLE_LOG));
    if let Ok(entries) = std::fs::read_dir(work_root) {
        for e in entries.flatten() {
            let output = e.path().join("output");
            for name in ["console.log", "runner.log", "harbor.log"] {
                push_log(&mut logs, name, &output.join(name));
            }
        }
    }
    logs
}

fn push_log(logs: &mut Vec<LogFile>, name: &str, path: &Path) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if bytes.is_empty() {
        return;
    }
    let start = bytes.len().saturating_sub(MAX_LOG_BYTES);
    logs.push(LogFile {
        name: name.to_owned(),
        bytes: bytes[start..].to_vec(),
    });
}

#[cfg(test)]
#[path = "harvest_tests.rs"]
mod harvest_tests;
