//! Reconstruct a paid Proof VM job from guest artefacts on the RW scratch
//! when vsock drops `Done`.
//!
//! Harbor writes `report.json` under `/var/lib/proof/work/<seq>-{evaluate,
//! baseline}/output/` on `scratch.ext4` before the guest sends `Done`. The
//! host sees that file as:
//! 1. `{jail_root}/scratch-tree/work/…` (host overlay of guest `/var/lib/proof`)
//! 2. else a **fresh** `debugfs -c -R 'rdump /work …'` of
//!    `{jail_root}/scratch.ext4` into `{jail_dir}/harvest-work` (never a
//!    leftover dump from an earlier job or an earlier incomplete recovery
//!    on the same jail). Each attempt invalidates any retained `harvest-work`
//!    and re-dumps; `debugfs rdump /work` lands at `{dest}/work`, which is
//!    the tree used for reconstruction.
//!
//! Only the highest-numbered matching work directory counts. Called from
//! `proof-fc-host` after `Run` when recv fails — tips via `BUILD_FROM=source`
//! of `proof-vm-orchestrator`, no guest rebake.
//!
//! **Fail-closed / refresh on a stale harvest copy.** Retained `tbench-x0002`
//! and metal shortpack a21f: guest `/work/.../atrx…/` already had
//! `verifier/reward.txt` and a full trial `result.json` (`finished_at` set,
//! `n_completed=6`, `n_running=0`). Host `{jail_dir}/harvest-work` was a
//! Done≈finish-second `debugfs` snapshot (`finished_at=null`, `n_running=1`
//! / `stats.n_running_trials`, atrx `result.json` missing — `reward.txt`
//! can land ~12s earlier). A vsock `Done` still **refreshes** that dump
//! from the overlay / a new `rdump` until those checks would pass; the vsock
//! score is kept (`host_harvest` is not invented). Dump-only reconstruct
//! without a complete overlay still refuses. Guest already wrote
//! `reward.txt` — this is not an adaptor always-write.

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
/// Host dump of guest `/work` (`debugfs rdump`) beside the jail.
pub const HARVEST_WORK: &str = "harvest-work";
/// Serial console capture beside the jail.
pub const CONSOLE_LOG: &str = "console.log";
/// Harbor jobs tree under a work dir (`$PROOF_WORK_DIR/harbor-jobs`).
pub const HARBOR_JOBS: &str = "harbor-jobs";
const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_REPORT_BYTES: u64 = 8 * 1024 * 1024;
/// How often a silent vsock is peeked for a scratch report.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// After a report is on disk, wait this long for a real `Done`.
pub const DONE_GRACE: Duration = Duration::from_secs(5);
/// Re-dump / overlay-copy attempts after vsock `Done` (debugfs race ~12s).
pub const REFRESH_ATTEMPTS: u32 = 8;

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
                    Ok(msg) => after_vsock(msg, jail_root, jail_dir, job).await,
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

async fn after_vsock(
    msg: RlmToHost,
    jail_root: &Path,
    jail_dir: &Path,
    job: &VmJob,
) -> Result<RlmToHost, HvError> {
    let RlmToHost::Done { output } = &msg else {
        return Ok(msg);
    };
    match job {
        VmJob::Evaluate { .. } | VmJob::Baseline { .. } => {}
        VmJob::Inspect { .. } | VmJob::ProposeRules { .. } | VmJob::Archive { .. } => {
            return Ok(msg);
        }
    }
    let evidence = report_from_output(output).map(|r| r.evidence.clone());
    if let Err(e) = refresh_dump_until_complete(jail_root, jail_dir, job, evidence.as_ref()).await {
        tracing::warn!(
            "vsock Done; harvest-work still incomplete after refresh ({e}); keeping vsock score"
        );
        let dest = jail_dir.join(HARVEST_WORK);
        if dump_stale_on_disk(&dest) {
            let _ = std::fs::remove_dir_all(&dest);
        }
    }
    Ok(msg)
}

fn report_from_output(output: &VmJobOutput) -> Option<&CustomRunReport> {
    match output {
        VmJobOutput::Baseline(report) => Some(report),
        VmJobOutput::Evaluated(run) => Some(&run.report),
        VmJobOutput::Inspected(_) | VmJobOutput::Rules(_) | VmJobOutput::Archived => None,
    }
}

fn dump_stale_on_disk(dump: &Path) -> bool {
    if !dump.is_dir() {
        return false;
    }
    let work = work_tree_from_dump(dump.to_path_buf()).unwrap_or_else(|| dump.to_path_buf());
    refuse_stale_harbor_snapshot(&work).is_err() || reward_without_result(&work).is_err()
}

fn or_err(
    jail_root: &Path,
    jail_dir: &Path,
    job: &VmJob,
    err: HvError,
) -> Result<RlmToHost, HvError> {
    match harvest_from_jail(jail_root, jail_dir, job) {
        Ok(output) => {
            tracing::warn!(
                "vsock lost the Done frame ({err}); harvested guest report from scratch"
            );
            Ok(RlmToHost::Done { output })
        }
        Err(harvest_err) => {
            let msg = harvest_err.to_string();
            if msg.contains("harvest-work incomplete") {
                tracing::warn!("vsock lost the Done frame ({err}); {msg}");
                return Err(harvest_err);
            }
            Err(err)
        }
    }
}

/// Reconstruct a paid job from the jail's scratch. `None` = nothing to take.
#[must_use]
pub fn try_from_jail(jail_root: &Path, jail_dir: &Path, job: &VmJob) -> Option<VmJobOutput> {
    harvest_from_jail(jail_root, jail_dir, job).ok()
}

fn harvest_from_jail(
    jail_root: &Path,
    jail_dir: &Path,
    job: &VmJob,
) -> Result<VmJobOutput, HvError> {
    refresh_dump_once(jail_root, jail_dir, job, None)?;
    let work = work_root(jail_root, jail_dir).ok_or_else(|| {
        HvError::Guest("no guest work overlay or harvest-work dump to reconstruct from".into())
    })?;
    from_work_tree(&work, jail_dir, job)
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
    let dest = jail_dir.join(HARVEST_WORK);
    // An early recovery can leave a nonempty harvest-work before the guest
    // writes report.json. Never return that tree: drop it and re-dump so a
    // later completed scratch.ext4 is what we reconstruct from.
    let _ = std::fs::remove_dir_all(&dest);
    dump_ext4_work(&image, &dest).ok()?;
    work_tree_from_dump(dest)
}

/// `debugfs rdump /work dest` copies the directory named `work` into `dest`.
/// Overlay harvest uses `{scratch-tree}/work` itself; keep the same shape.
fn work_tree_from_dump(dest: PathBuf) -> Option<PathBuf> {
    let nested = dest.join("work");
    if nested.is_dir() {
        return Some(nested);
    }
    dest.is_dir().then_some(dest)
}

fn copy_tree(src: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    for e in entries {
        let e = e.map_err(|e| format!("read {}: {e}", src.display()))?;
        let from = e.path();
        let to = dest.join(e.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .map_err(|err| format!("copy {} → {}: {err}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

fn dump_matches_evidence(
    work: &Path,
    evidence: &BTreeMap<String, serde_json::Value>,
) -> Result<(), HvError> {
    refuse_incomplete_harbor_jobs(work, evidence)?;
    refuse_stale_harbor_snapshot(work)?;
    reward_without_result(work)
}

fn check_dump(
    work: &Path,
    evidence: Option<&BTreeMap<String, serde_json::Value>>,
) -> Result<(), HvError> {
    if let Some(ev) = evidence {
        dump_matches_evidence(work, ev)
    } else {
        refuse_stale_harbor_snapshot(work)?;
        reward_without_result(work)
    }
}

fn reward_without_result(work: &Path) -> Result<(), HvError> {
    let mut missing = Vec::new();
    for trial in harbor_trial_dirs(work) {
        let result = trial.join("result.json");
        let reward = trial.join("verifier").join("reward.txt");
        if non_empty_file(&reward) && !trial_has_measured_reward(&result) {
            let name = trial
                .file_name()
                .map_or_else(|| "trial".into(), |n| n.to_string_lossy().into_owned());
            missing.push(name);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(HvError::Guest(format!(
        "harvest-work incomplete vs guest (reward.txt without measured result.json: {}); refusing to publish a partial copy",
        missing.join(", ")
    )))
}

fn refresh_dump_once(
    jail_root: &Path,
    jail_dir: &Path,
    job: &VmJob,
    evidence: Option<&BTreeMap<String, serde_json::Value>>,
) -> Result<(), HvError> {
    match job {
        VmJob::Evaluate { .. } | VmJob::Baseline { .. } => {}
        VmJob::Inspect { .. } | VmJob::ProposeRules { .. } | VmJob::Archive { .. } => {
            return Ok(());
        }
    }
    let dest = jail_dir.join(HARVEST_WORK);
    let overlay = jail_root.join(SCRATCH_TREE).join("work");
    if overlay.is_dir() {
        let _ = std::fs::remove_dir_all(&dest);
        copy_tree(&overlay, &dest)
            .map_err(|e| HvError::Guest(format!("refresh harvest-work from overlay: {e}")))?;
        let work = work_tree_from_dump(dest)
            .ok_or_else(|| HvError::Guest("refresh harvest-work wrote no work tree".into()))?;
        if let Some(why) = harvest_lagging_guest(&overlay, &work) {
            return Err(HvError::Guest(why));
        }
        return check_dump(&work, evidence);
    }
    let image = jail_root.join(SCRATCH_IN_JAIL);
    if !image.is_file() {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(&dest);
    dump_ext4_work(&image, &dest).map_err(HvError::Guest)?;
    let work = work_tree_from_dump(dest)
        .ok_or_else(|| HvError::Guest("debugfs dumped no work tree into harvest-work".into()))?;
    check_dump(&work, evidence)
}

async fn refresh_dump_until_complete(
    jail_root: &Path,
    jail_dir: &Path,
    job: &VmJob,
    evidence: Option<&BTreeMap<String, serde_json::Value>>,
) -> Result<(), HvError> {
    let mut last: Option<HvError> = None;
    for i in 0..REFRESH_ATTEMPTS {
        match refresh_dump_once(jail_root, jail_dir, job, evidence) {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!(
                    attempt = i + 1,
                    "harvest-work dump incomplete ({e}); retrying"
                );
                last = Some(e);
                if i + 1 < REFRESH_ATTEMPTS {
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| HvError::Guest("harvest-work refresh failed".into())))
}

/// Relative Harbor trial files on `guest` missing from `harvest`.
fn harvest_lagging_guest(guest: &Path, harvest: &Path) -> Option<String> {
    let missing = guest_trial_files(guest)
        .into_iter()
        .filter(|rel| !non_empty_file(&harvest.join(rel)))
        .collect::<Vec<_>>();
    let mut reasons: Vec<String> = Vec::new();
    if !missing.is_empty() {
        let shown: Vec<String> = missing
            .iter()
            .take(8)
            .map(|p| p.display().to_string())
            .collect();
        reasons.push(format!(
            "{} trial file(s) missing, e.g. {}",
            missing.len(),
            shown.join(", ")
        ));
    }
    for rel in harbor_job_result_rels(guest) {
        if job_snapshot_finished(&guest.join(&rel))
            && job_snapshot_stale_or_missing(&harvest.join(&rel))
        {
            reasons.push(format!(
                "{} still n_running>0 or finished_at=null on the harvest copy",
                rel.display()
            ));
        }
    }
    if reasons.is_empty() {
        return None;
    }
    Some(format!(
        "harvest-work incomplete vs guest ({}); refusing to publish a partial copy",
        reasons.join("; ")
    ))
}

fn guest_trial_files(work: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for trial in harbor_trial_dirs(work) {
        for rel in ["result.json", "verifier/reward.txt"] {
            let p = trial.join(rel);
            if non_empty_file(&p) {
                if let Ok(rel_path) = p.strip_prefix(work) {
                    out.push(rel_path.to_path_buf());
                }
            }
        }
    }
    out.extend(harbor_job_result_rels(work));
    out
}

fn harbor_job_dirs(work: &Path) -> Vec<PathBuf> {
    let mut jobs = Vec::new();
    for root in harbor_jobs_roots(work) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                jobs.push(p);
            }
        }
    }
    jobs.sort();
    jobs
}

fn harbor_job_result_rels(work: &Path) -> Vec<PathBuf> {
    harbor_job_dirs(work)
        .into_iter()
        .filter_map(|job| {
            let p = job.join("result.json");
            p.is_file()
                .then(|| p.strip_prefix(work).ok().map(Path::to_path_buf))
                .flatten()
        })
        .collect()
}

fn load_json(path: &Path) -> Option<serde_json::Value> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

fn is_harbor_job_snapshot(v: &serde_json::Value) -> bool {
    if v.get("trial_name").is_some() || v.get("verifier_result").is_some() {
        return false;
    }
    if v.get("n_running").is_some()
        || v.get("finished_at").is_some()
        || v.get("n_completed").is_some()
    {
        return true;
    }
    v.get("stats").is_some_and(|s| {
        s.get("n_running_trials").is_some()
            || s.get("n_running").is_some()
            || s.get("n_completed").is_some()
    })
}

fn snapshot_n_running(v: &serde_json::Value) -> Option<u64> {
    v.get("n_running")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            v.get("stats")
                .and_then(|s| s.get("n_running_trials"))
                .and_then(serde_json::Value::as_u64)
        })
        .or_else(|| {
            v.get("stats")
                .and_then(|s| s.get("n_running"))
                .and_then(serde_json::Value::as_u64)
        })
}

fn snapshot_finished_at_null(v: &serde_json::Value) -> bool {
    match v.get("finished_at") {
        Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(s)) => s.is_empty(),
        None => is_harbor_job_snapshot(v),
        Some(_) => false,
    }
}

fn job_snapshot_stale(v: &serde_json::Value) -> bool {
    is_harbor_job_snapshot(v)
        && (snapshot_n_running(v).is_some_and(|n| n > 0) || snapshot_finished_at_null(v))
}

fn job_snapshot_finished(path: &Path) -> bool {
    load_json(path).is_some_and(|v| is_harbor_job_snapshot(&v) && !job_snapshot_stale(&v))
}

fn job_snapshot_stale_or_missing(path: &Path) -> bool {
    match load_json(path) {
        None => true,
        Some(v) => !is_harbor_job_snapshot(&v) || job_snapshot_stale(&v),
    }
}

fn refuse_stale_harbor_snapshot(work_root: &Path) -> Result<(), HvError> {
    for job in harbor_job_dirs(work_root) {
        let path = job.join("result.json");
        if !path.is_file() {
            continue;
        }
        let Some(v) = load_json(&path) else {
            continue;
        };
        if !job_snapshot_stale(&v) {
            continue;
        }
        let n_running = snapshot_n_running(&v).unwrap_or(0);
        return Err(HvError::Guest(format!(
            "harvest-work incomplete vs guest ({} n_running={n_running}, finished_at null); refusing to publish a stale snapshot",
            path.display()
        )));
    }
    Ok(())
}

fn harbor_jobs_roots(work: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if work.join(HARBOR_JOBS).is_dir() {
        roots.push(work.join(HARBOR_JOBS));
    }
    let Ok(entries) = std::fs::read_dir(work) else {
        return roots;
    };
    for e in entries.flatten() {
        let p = e.path().join(HARBOR_JOBS);
        if p.is_dir() {
            roots.push(p);
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn harbor_trial_dirs(work: &Path) -> Vec<PathBuf> {
    let mut trials = Vec::new();
    for job_dir in harbor_job_dirs(work) {
        let Ok(children) = std::fs::read_dir(&job_dir) else {
            continue;
        };
        for child in children.flatten() {
            let trial = child.path();
            if !trial.is_dir() {
                continue;
            }
            let name = match trial.file_name() {
                Some(n) => n.to_string_lossy(),
                None => continue,
            };
            if name == "agent" || name == "verifier" || name == "artifacts" {
                continue;
            }
            trials.push(trial);
        }
    }
    trials.sort();
    trials
}

fn evidence_trial_names(evidence: &BTreeMap<String, serde_json::Value>) -> Vec<String> {
    let Some(trials) = evidence.get("trials").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    trials
        .iter()
        .filter_map(|row| {
            row.get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .filter(|name| !name.is_empty())
        .collect()
}

fn trial_logical_name(dir: &Path) -> Option<String> {
    load_json(&dir.join("result.json"))
        .as_ref()
        .and_then(|v| v.get("trial_name"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn trial_dir_matches(dir: &Path, name: &str) -> bool {
    let Some(n) = dir.file_name() else {
        return false;
    };
    let n = n.to_string_lossy();
    if n == name || n.starts_with(&format!("{name}__")) || n.starts_with(&format!("{name}-")) {
        return true;
    }
    trial_logical_name(dir).as_deref() == Some(name)
}

fn non_empty_file(path: &Path) -> bool {
    path.is_file() && std::fs::metadata(path).is_ok_and(|m| m.len() > 0)
}

fn trial_has_measured_reward(result_path: &Path) -> bool {
    load_json(result_path)
        .as_ref()
        .and_then(|v| v.get("verifier_result"))
        .and_then(|vr| vr.get("rewards"))
        .and_then(|r| r.get("reward"))
        .and_then(serde_json::Value::as_f64)
        .is_some_and(f64::is_finite)
}

/// Per-trial evidence is complete only when both files are non-empty and
/// `result.json` parses as JSON with a finite `verifier_result.rewards.reward`.
/// Zero-byte, truncated, or reward-less payloads are incomplete (fail-closed).
fn trial_files_present(trial_dir: &Path) -> bool {
    let result = trial_dir.join("result.json");
    let reward = trial_dir.join("verifier").join("reward.txt");
    non_empty_file(&result) && trial_has_measured_reward(&result) && non_empty_file(&reward)
}

fn refuse_incomplete_harbor_jobs(
    work_root: &Path,
    evidence: &BTreeMap<String, serde_json::Value>,
) -> Result<(), HvError> {
    let claimed = evidence_trial_names(evidence);
    let n_measured = evidence
        .get("n_measured")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| usize::try_from(n).ok());
    let dirs = harbor_trial_dirs(work_root);
    if dirs.is_empty() && claimed.is_empty() && n_measured.unwrap_or(0) == 0 {
        return Ok(());
    }
    let mut missing: Vec<String> = Vec::new();
    let names: Vec<String> = if claimed.is_empty() {
        dirs.iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect()
    } else {
        claimed
    };
    for name in &names {
        let dir = dirs.iter().find(|d| trial_dir_matches(d, name));
        match dir {
            Some(d) if trial_files_present(d) => {}
            Some(_) => missing.push(format!(
                "{name} (missing measured reward on result.json and/or verifier/reward.txt)"
            )),
            None => missing.push(format!("{name} (trial dir missing from harvest copy)")),
        }
    }
    if let Some(n) = n_measured {
        let complete = dirs.iter().filter(|d| trial_files_present(d)).count();
        if complete != n {
            missing.push(format!(
                "n_measured={n} but harvest copy has {complete} complete trial(s)"
            ));
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(HvError::Guest(format!(
        "harvest-work incomplete vs guest ({}); refusing to publish a partial copy",
        missing.join("; ")
    )))
}

fn debugfs_bin() -> &'static str {
    if Path::new("/usr/sbin/debugfs").is_file() {
        "/usr/sbin/debugfs"
    } else if Path::new("/sbin/debugfs").is_file() {
        "/sbin/debugfs"
    } else {
        "debugfs"
    }
}

fn dump_ext4_work(image: &Path, dest: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    let spec = format!("rdump /work {}", dest.display());
    let out = std::process::Command::new(debugfs_bin())
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
    refuse_incomplete_harbor_jobs(work_root, &guest.evidence)?;
    refuse_stale_harbor_snapshot(work_root)?;
    reward_without_result(work_root)?;
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
