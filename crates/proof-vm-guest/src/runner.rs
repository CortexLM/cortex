//! Operator runner adaptors: how the guest turns a job into a process and a
//! process's output into a protocol document.
//!
//! An adaptor is a directory `<runners_dir>/<runner id>/` baked into the
//! guest image by the operator, holding executables named after the job
//! kinds it supports:
//!
//! | Entrypoint | Job | Writes under `PROOF_OUTPUT_DIR` |
//! |------------|-----|---------------------------------|
//! | `run` | `Baseline`, `Evaluate` | `report.json` — [`RunnerReport`]; **Evaluate** also `results.json` |
//! | `inspect` | `Inspect` | `checklist.json` — `[{"id", "pass", "evidence"}]` |
//! | `propose_rules` | `ProposeRules` (optional) | `rules.json` — `[{"id", "text"}]` |
//!
//! Every entrypoint receives the same environment contract
//! ([`env::*`](env)): the job kind, the identities (topic, custom id,
//! submission, artefact), the metric name and direction, the pack directory
//! and digest, the artefact directory (when the request carries a locator),
//! the model pin, seed, deadline, FLOP figures, the secrets directory, and
//! one `PROOF_PARAM_<KEY>` per `constraints.params` entry (key upper-cased,
//! `-` → `_`; two signed names that collide after that are refused before
//! anything runs). The adaptor decides what those mean; this module never
//! does.
//!
//! The process is held to the request's deadline, its stdout / stderr are
//! drained into a **rolling tail** bounded at [`MAX_TAIL_BYTES`] per stream
//! while it runs (a flooding adaptor costs the guest no more memory than
//! that) and **redacted** (every staged secret value is blanked), and a
//! missing / malformed / non-finite report is a failed job.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use proof_canon::validate_rules;
use proof_experiment::{ExperimentBinding, RunPolicy};
use proof_results::load_evaluate;
use proof_rlm::{
    ArtifactFile, Checklist, CustomRunReport, CustomRunRequest, InspectOutcome, LogFile, RuleSet,
    RunOutcome, RUN_REPORT_SCHEMA,
};
use proof_task::{ChecklistRule, TopicDocument};
use serde::Deserialize;

use crate::staging::{secret_names, secret_values, StagedPack};
use crate::GuestConfig;

/// Environment variable names of the adaptor contract.
pub mod env {
    /// Runner id the topic selected.
    pub const RUNNER_ID: &str = "PROOF_RUNNER_ID";
    /// `baseline` / `evaluate` / `inspect` / `propose_rules`.
    pub const JOB: &str = "PROOF_JOB";
    /// Topic id.
    pub const TOPIC_ID: &str = "PROOF_TOPIC_ID";
    /// Custom metric id.
    pub const CUSTOM_ID: &str = "PROOF_CUSTOM_ID";
    /// Primary metric name the report fills.
    pub const PRIMARY_METRIC: &str = "PROOF_PRIMARY_METRIC";
    /// `max` / `min`.
    pub const METRIC_DIRECTION: &str = "PROOF_METRIC_DIRECTION";
    /// Frozen submission digest.
    pub const SUBMISSION_DIGEST: &str = "PROOF_SUBMISSION_DIGEST";
    /// Artefact digest (sha256 hex of the served tar).
    pub const ARTIFACT_DIGEST: &str = "PROOF_ARTIFACT_DIGEST";
    /// Unpacked artefact tree (set iff the request carried a locator).
    pub const ARTIFACT_DIR: &str = "PROOF_ARTIFACT_DIR";
    /// Unpacked experiment pack (paid jobs).
    pub const PACK_DIR: &str = "PROOF_PACK_DIR";
    /// `sha256:<hex>` of the pack.
    pub const PACK_DIGEST: &str = "PROOF_PACK_DIGEST";
    /// `constraints.model_pin`, when the topic carries one.
    pub const MODEL_PIN: &str = "PROOF_MODEL_PIN";
    /// `constraints.task_slice`, when the topic carries one.
    pub const TASK_SLICE: &str = "PROOF_TASK_SLICE";
    /// Seed every paid call must use.
    pub const SEED: &str = "PROOF_SEED";
    /// Wall-clock the run is held to (seconds).
    pub const DEADLINE_S: &str = "PROOF_DEADLINE_S";
    /// FLOPs the miner declared.
    pub const DECLARED_FLOPS: &str = "PROOF_DECLARED_FLOPS";
    /// Topic FLOP budget.
    pub const FLOPS_BUDGET: &str = "PROOF_FLOPS_BUDGET";
    /// Miner claim text file.
    pub const CLAIM_FILE: &str = "PROOF_CLAIM_FILE";
    /// Where the adaptor writes its answer.
    pub const OUTPUT_DIR: &str = "PROOF_OUTPUT_DIR";
    /// Scratch for the adaptor (writable disk).
    pub const WORK_DIR: &str = "PROOF_WORK_DIR";
    /// Directory of staged owner key files (read them; never print them).
    pub const SECRETS_DIR: &str = "PROOF_SECRETS_DIR";
    /// Comma-separated names of the staged secret files.
    pub const SECRET_FILES: &str = "PROOF_SECRET_FILES";
    /// Directory holding one file per miner BYOK variable (paid runs only).
    pub const MINER_ENV_DIR: &str = "PROOF_MINER_ENV_DIR";
    /// Comma-separated names of the miner BYOK variables exported for this
    /// run. Each is also exported under its own name.
    pub const MINER_ENV_NAMES: &str = "PROOF_MINER_ENV_NAMES";
    /// Rules JSON the inspector ticks (`inspect`).
    pub const RULES_FILE: &str = "PROOF_RULES_FILE";
    /// Signed topic JSON (`propose_rules`).
    pub const TOPIC_FILE: &str = "PROOF_TOPIC_FILE";
    /// Prefix of one variable per `constraints.params` entry.
    pub const PARAM_PREFIX: &str = "PROOF_PARAM_";
}

/// Bounded stdout + stderr tail kept from a run — the most the run log the
/// host sees can be. Each stream is drained into a rolling half of it
/// ([`STREAM_TAIL_BYTES`]) **while the process runs**, so a flooding adaptor
/// costs the guest no more memory than this, whatever it writes.
pub const MAX_TAIL_BYTES: usize = 64 * 1024;
/// Rolling tail kept per stream while draining (half of [`MAX_TAIL_BYTES`]).
pub const STREAM_TAIL_BYTES: usize = MAX_TAIL_BYTES / 2;
/// Largest `report.json` / `checklist.json` / `rules.json` read back.
pub const MAX_OUTPUT_DOC_BYTES: u64 = 8 * 1024 * 1024;
/// Deadline for jobs that carry none (`ProposeRules`).
pub const DEFAULT_UNPAID_DEADLINE: Duration = Duration::from_mins(30);
/// Host kills the process this long after its own deadline; the guest cuts
/// a little earlier so the failure is reported, not inferred.
pub const DEADLINE_SLACK: Duration = Duration::from_secs(5);
/// Caps on the artefact tree shipped back with an inspection.
pub const MAX_ARTIFACT_FILES: usize = 256;
/// See [`MAX_ARTIFACT_FILES`].
pub const MAX_ARTIFACT_FILE_BYTES: u64 = 256 * 1024;
/// See [`MAX_ARTIFACT_FILES`].
pub const MAX_ARTIFACT_TOTAL_BYTES: u64 = 4 * 1024 * 1024;

/// Which job an entrypoint serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// Topic reference run.
    Baseline,
    /// Paid miner run.
    Evaluate,
    /// Rule ticking, no paid inference.
    Inspect,
    /// Rule (re)writing.
    ProposeRules,
}

impl JobKind {
    /// Wire / directory name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Evaluate => "evaluate",
            Self::Inspect => "inspect",
            Self::ProposeRules => "propose_rules",
        }
    }

    /// Entrypoint file name under the adaptor directory.
    #[must_use]
    pub const fn entrypoint(self) -> &'static str {
        match self {
            Self::Baseline | Self::Evaluate => "run",
            Self::Inspect => "inspect",
            Self::ProposeRules => "propose_rules",
        }
    }
}

/// A resolved adaptor: the topic's binding and the directory that serves it.
#[derive(Debug, Clone)]
pub struct Adaptor {
    /// `<runners_dir>/<runner id>`.
    pub dir: PathBuf,
    /// What the topic selected.
    pub binding: ExperimentBinding,
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

impl Adaptor {
    /// The adaptor the request's topic selects, iff it is installed here.
    pub fn resolve(runners_dir: &Path, request: &CustomRunRequest) -> Result<Self, String> {
        let binding = Self::binding_of(&request.constraints.params)?.ok_or_else(|| {
            format!(
                "topic selects no in-guest runner (constraints.params.{}); this guest runs no default and never reports a placeholder value",
                proof_experiment::PARAM_RUNNER
            )
        })?;
        Self::installed(runners_dir, binding)
    }

    /// The binding a params map carries, refused when malformed.
    pub fn binding_of(
        params: &BTreeMap<String, String>,
    ) -> Result<Option<ExperimentBinding>, String> {
        ExperimentBinding::from_params(params).map_err(|e| format!("experiment binding: {e}"))
    }

    /// `binding`'s adaptor, iff `<runners_dir>/<runner>/run` is executable.
    pub fn installed(runners_dir: &Path, binding: ExperimentBinding) -> Result<Self, String> {
        let dir = runners_dir.join(&binding.runner);
        if !executable(&dir.join(JobKind::Baseline.entrypoint())) {
            return Err(format!(
                "runner {:?} is not installed in this guest image (no executable {}); bake the adaptor or re-sign the topic",
                binding.runner,
                dir.join("run").display()
            ));
        }
        Ok(Self { dir, binding })
    }

    /// The executable for `kind`, if the adaptor ships it.
    pub fn entrypoint(&self, kind: JobKind) -> Result<PathBuf, String> {
        let path = self.dir.join(kind.entrypoint());
        if executable(&path) {
            Ok(path)
        } else {
            Err(format!(
                "runner {:?} has no {} entrypoint ({}); this guest performs no {} without one",
                self.binding.runner,
                kind.entrypoint(),
                path.display(),
                kind.as_str()
            ))
        }
    }
}

/// What `run` writes to `report.json`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RunnerReport {
    /// The measured primary metric. Must be finite.
    pub primary_value: f64,
    /// Whether the miner's claim held (default: it did not).
    #[serde(default)]
    pub claim_holds: bool,
    /// Measured usage; absent = the runner measured nothing (the control
    /// plane refuses that against a budget — no number is substituted).
    #[serde(default)]
    pub flops_used: Option<u64>,
    /// Opaque evidence (per-task rows, timings). Redacted before it travels.
    #[serde(default)]
    pub evidence: BTreeMap<String, serde_json::Value>,
}

/// One item of `checklist.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct InspectItem {
    /// Rule id.
    pub id: String,
    /// Verdict.
    pub pass: bool,
    /// What the inspector saw.
    #[serde(default)]
    pub evidence: String,
}

/// Outcome of one exec.
#[derive(Debug)]
struct Exec {
    exit: Option<i32>,
    timed_out: bool,
    tail: String,
}

/// A rolling tail: keeps the last `cap` bytes pushed, counts what it dropped.
/// Memory is bounded by `cap` however much the writer produces.
#[derive(Debug)]
pub(crate) struct Tail {
    cap: usize,
    buf: Vec<u8>,
    dropped: u64,
}

impl Tail {
    /// A tail keeping at most `cap` bytes.
    #[must_use]
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap,
            buf: Vec::new(),
            dropped: 0,
        }
    }

    /// Append `chunk`, forgetting the oldest bytes beyond the cap.
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= self.cap {
            self.dropped += (self.buf.len() + chunk.len() - self.cap) as u64;
            self.buf.clear();
            self.buf.extend_from_slice(&chunk[chunk.len() - self.cap..]);
            return;
        }
        let excess = (self.buf.len() + chunk.len()).saturating_sub(self.cap);
        if excess > 0 {
            self.buf.drain(..excess);
            self.dropped += excess as u64;
        }
        self.buf.extend_from_slice(chunk);
    }

    /// Bytes kept.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Bytes forgotten so far.
    #[cfg(test)]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The kept bytes as text, prefixed with a marker when anything was cut.
    #[must_use]
    pub(crate) fn text(&self) -> String {
        let body = String::from_utf8_lossy(&self.buf);
        if self.dropped == 0 {
            body.into_owned()
        } else {
            format!("[... {} earlier bytes dropped ...]\n{body}", self.dropped)
        }
    }
}

/// Drain `reader` to EOF into a bounded tail (never into memory unbounded).
async fn drain_bounded<R: tokio::io::AsyncRead + Unpin>(reader: Option<R>, cap: usize) -> Tail {
    let mut tail = Tail::new(cap);
    let Some(mut reader) = reader else {
        return tail;
    };
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match tokio::io::AsyncReadExt::read(&mut reader, &mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => tail.push(&chunk[..n]),
        }
    }
    tail
}

/// Last `max` bytes as text.
fn tail(bytes: &[u8], max: usize) -> String {
    let start = bytes.len().saturating_sub(max);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// Blank every secret value in `text`.
#[must_use]
pub fn redact(text: &str, secrets: &[Vec<u8>]) -> String {
    let mut out = text.to_owned();
    for s in secrets {
        if let Ok(s) = std::str::from_utf8(s) {
            if !s.is_empty() {
                out = out.replace(s, "[REDACTED]");
            }
        }
    }
    out
}

fn redact_value(v: &mut serde_json::Value, secrets: &[Vec<u8>]) {
    match v {
        serde_json::Value::String(s) => *s = redact(s, secrets),
        serde_json::Value::Array(items) => items.iter_mut().for_each(|i| redact_value(i, secrets)),
        serde_json::Value::Object(map) => map.values_mut().for_each(|i| redact_value(i, secrets)),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

/// The env var name a topic param key maps to: upper-cased, `-` → `_`.
fn param_env_name(key: &str) -> String {
    let normalised: String = key
        .chars()
        .map(|c| {
            if c == '-' {
                '_'
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect();
    format!("{}{normalised}", env::PARAM_PREFIX)
}

/// `PROOF_PARAM_<KEY>` for every topic param. Two **distinct** signed names
/// that normalise to the same variable (`foo-bar` / `foo_bar`) are refused:
/// the adaptor environment cannot carry both, and a run must never execute
/// with one of its signed inputs silently replaced by another.
fn param_env(params: &BTreeMap<String, String>) -> Result<Vec<(String, String)>, String> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    let mut vars = Vec::with_capacity(params.len());
    for (k, v) in params {
        let name = param_env_name(k);
        if let Some(first) = seen.insert(name.clone(), k) {
            return Err(format!(
                "constraints.params {first:?} and {k:?} both map to {name}; the adaptor environment cannot carry both — re-sign the topic with names that stay distinct after upper-casing and '-' → '_'"
            ));
        }
        vars.push((name, v.clone()));
    }
    Ok(vars)
}

/// The contract for one job. Fails on a param-name collision (nothing runs).
#[allow(clippy::too_many_arguments)]
fn job_env(
    cfg: &GuestConfig,
    runner: &str,
    request: &CustomRunRequest,
    kind: JobKind,
    work: &Path,
    output: &Path,
    pack: Option<&StagedPack>,
    artifact: Option<&Path>,
) -> Result<Vec<(String, String)>, String> {
    // Generic run-policy knobs are shape-checked before the adaptor sees
    // them (no spend on a malformed signed value); the adaptor interprets.
    RunPolicy::from_params(&request.constraints.params).map_err(|e| e.to_string())?;
    let params = param_env(&request.constraints.params)?;
    let direction = match request.direction {
        proof_task::MetricDirection::Max => "max",
        proof_task::MetricDirection::Min => "min",
    };
    let mut vars = vec![
        (env::RUNNER_ID.to_owned(), runner.to_owned()),
        (env::JOB.to_owned(), kind.as_str().to_owned()),
        (env::TOPIC_ID.to_owned(), request.topic_id.clone()),
        (env::CUSTOM_ID.to_owned(), request.custom_id.clone()),
        (env::PRIMARY_METRIC.to_owned(), request.primary.clone()),
        (env::METRIC_DIRECTION.to_owned(), direction.to_owned()),
        (
            env::SUBMISSION_DIGEST.to_owned(),
            request.submission_digest.clone(),
        ),
        (
            env::ARTIFACT_DIGEST.to_owned(),
            request.artifact_digest.clone(),
        ),
        (env::SEED.to_owned(), request.seed.to_string()),
        (
            env::DEADLINE_S.to_owned(),
            request.sandbox.deadline_s.to_string(),
        ),
        (
            env::DECLARED_FLOPS.to_owned(),
            request.declared_flops.to_string(),
        ),
        (
            env::FLOPS_BUDGET.to_owned(),
            request.flops_budget.to_string(),
        ),
        (env::OUTPUT_DIR.to_owned(), output.display().to_string()),
        (env::WORK_DIR.to_owned(), work.display().to_string()),
        (
            env::SECRETS_DIR.to_owned(),
            cfg.secrets_dir.display().to_string(),
        ),
        (
            env::SECRET_FILES.to_owned(),
            secret_names(&cfg.secrets_dir).join(","),
        ),
    ];
    if let Some(p) = pack {
        vars.push((env::PACK_DIR.to_owned(), p.dir.display().to_string()));
        vars.push((env::PACK_DIGEST.to_owned(), p.digest.clone()));
    }
    if let Some(a) = artifact {
        vars.push((env::ARTIFACT_DIR.to_owned(), a.display().to_string()));
    }
    if let Some(m) = &request.constraints.model_pin {
        vars.push((env::MODEL_PIN.to_owned(), m.clone()));
    }
    if let Some(s) = &request.constraints.task_slice {
        vars.push((env::TASK_SLICE.to_owned(), s.clone()));
    }
    vars.extend(params);
    Ok(vars)
}

/// Export the miner's own BYOK environment for a **paid** run: one variable
/// per name the signed topic declared, plus [`env::MINER_ENV_DIR`] /
/// [`env::MINER_ENV_NAMES`] and one 0600 file per value under the guest's
/// secrets root.
///
/// Evaluate on a `miner_byok` topic always gets [`env::MINER_ENV_DIR`] even
/// when the request carried no values: the adaptor then fails closed on a
/// **missing key file**, never because the directory variable was unset.
/// Baseline with an empty map still skips (operator-paid; owner key path).
///
/// Fail-closed and last: the control plane already held these names to the
/// topic's allowlist, and this checks the shape again and refuses any name
/// that would shadow something the contract or the base environment already
/// set. A miner variable can therefore add to what an adaptor sees and never
/// rewrite it — `PROOF_…`, `PATH`, and the rest stay the guest's own facts.
///
/// Returns the values to blank out of everything the guest ships back.
/// Nothing here is logged: the names travel, the values do not.
fn inject_miner_env(
    cfg: &GuestConfig,
    request: &CustomRunRequest,
    kind: JobKind,
    vars: &mut Vec<(String, String)>,
) -> Result<Vec<Vec<u8>>, String> {
    let evaluate_needs_dir =
        kind == JobKind::Evaluate && !request.constraints.miner_env_required().is_empty();
    if request.miner_env.is_empty() && !evaluate_needs_dir {
        return Ok(Vec::new());
    }
    let taken: std::collections::BTreeSet<String> = vars
        .iter()
        .chain(base_env(cfg).iter())
        .map(|(k, _)| k.clone())
        .collect();
    let mut miner = Vec::with_capacity(request.miner_env.len());
    for (name, value) in request.miner_env.iter() {
        if !proof_canon::is_env_name(name) {
            return Err(format!(
                "miner env {name:?} is not a variable a miner may bring; the run is refused rather than run with it"
            ));
        }
        if taken.contains(name) {
            return Err(format!(
                "miner env {name:?} is already part of the adaptor contract; a miner variable never replaces a guest fact"
            ));
        }
        miner.push((name.to_owned(), value.to_owned()));
    }
    let dir = crate::staging::stage_miner_env(&cfg.secrets_dir, &miner, cfg.run_as)?;
    let names: Vec<&str> = miner.iter().map(|(n, _)| n.as_str()).collect();
    if !names.is_empty() {
        tracing::info!(
            names = names.join(",").as_str(),
            "miner byok environment exported to the adaptor (values not logged)"
        );
    }
    vars.push((env::MINER_ENV_DIR.to_owned(), dir.display().to_string()));
    vars.push((env::MINER_ENV_NAMES.to_owned(), names.join(",")));
    let secrets = miner
        .iter()
        .filter(|(_, v)| v.len() >= crate::staging::MIN_SECRET_LEN)
        .map(|(_, v)| v.as_bytes().to_vec())
        .collect();
    vars.extend(miner);
    Ok(secrets)
}

/// Base environment every adaptor gets, regardless of the job.
fn base_env(cfg: &GuestConfig) -> Vec<(String, String)> {
    let mut vars = vec![
        (
            "PATH".to_owned(),
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_owned(),
        ),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
    ];
    if let Some((uid, _)) = cfg.run_as {
        vars.push(("HOME".to_owned(), format!("/home/uid{uid}")));
        vars.push(("XDG_RUNTIME_DIR".to_owned(), format!("/run/user/{uid}")));
    } else {
        vars.push(("HOME".to_owned(), "/root".to_owned()));
    }
    vars
}

/// Exec `entrypoint` with `vars` in `cwd`, cut at `deadline`. Kills the whole
/// process group on the way out. Never inherits the agent's environment.
async fn exec(
    cfg: &GuestConfig,
    entrypoint: &Path,
    vars: Vec<(String, String)>,
    cwd: &Path,
    deadline: Duration,
) -> Result<Exec, String> {
    let mut cmd = tokio::process::Command::new(entrypoint);
    cmd.env_clear()
        .envs(base_env(cfg))
        .envs(vars)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    if let Some((uid, gid)) = cfg.run_as {
        cmd.uid(uid).gid(gid);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", entrypoint.display()))?;
    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Both streams are drained concurrently (a full stderr pipe never stalls
    // a chatty stdout) into rolling tails: whatever the adaptor floods, the
    // guest holds at most MAX_TAIL_BYTES of it.
    let drain = async {
        let (out, err) = tokio::join!(
            drain_bounded(stdout, STREAM_TAIL_BYTES),
            drain_bounded(stderr, STREAM_TAIL_BYTES)
        );
        let mut text = out.text();
        text.push_str("\n--- stderr ---\n");
        text.push_str(&err.text());
        text
    };
    let budget = deadline
        .saturating_sub(DEADLINE_SLACK)
        .max(Duration::from_secs(1));
    let waited = tokio::time::timeout(budget, async {
        let (status, output) = tokio::join!(child.wait(), drain);
        (status, output)
    })
    .await;
    if let Ok((status, output)) = waited {
        return Ok(Exec {
            exit: status.ok().and_then(|s| s.code()),
            timed_out: false,
            tail: tail(output.as_bytes(), MAX_TAIL_BYTES),
        });
    }
    kill_group(pid).await;
    let _ = child.start_kill();
    let _ = child.wait().await;
    Ok(Exec {
        exit: None,
        timed_out: true,
        tail: String::new(),
    })
}

/// Best-effort kill of the adaptor's process group (its containers' helper
/// processes included) through the guest's own `kill`.
async fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    let _ = tokio::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

fn sync_file(path: &Path) -> Result<(), String> {
    std::fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("sync {}: {e}", path.display()))
}

fn sync_tree(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Err(format!("sync {}: not a directory", root.display()));
    }
    let mut stack = vec![root.to_path_buf()];
    let mut dirs = Vec::new();
    while let Some(d) = stack.pop() {
        dirs.push(d.clone());
        let entries = std::fs::read_dir(&d).map_err(|e| format!("read {}: {e}", d.display()))?;
        for e in entries {
            let e = e.map_err(|e| format!("read {}: {e}", d.display()))?;
            let p = e.path();
            let meta = std::fs::symlink_metadata(&p)
                .map_err(|err| format!("sync {}: {err}", p.display()))?;
            if meta.file_type().is_symlink() {
                if std::fs::metadata(&p).is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                sync_file(&p)?;
            } else if meta.is_dir() {
                stack.push(p);
            } else {
                sync_file(&p)?;
            }
        }
    }
    for d in dirs {
        sync_file(&d)?;
    }
    // Best-effort syncfs so retain-on-fail of the virtio-blk image does
    // not need e2fsck journal replay to see work/ (metal tbench-x0004).
    let _ = std::process::Command::new("sync")
        .arg("-f")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

/// Flush `root` before Done or Failed. Missing dir is a no-op (no job yet).
pub(crate) fn persist_work(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    sync_tree(root)
}

fn read_output_doc<T: for<'de> Deserialize<'de>>(path: &Path, what: &str) -> Result<T, String> {
    let meta = std::fs::metadata(path)
        .map_err(|_| format!("adaptor wrote no {what} ({})", path.display()))?;
    if meta.len() > MAX_OUTPUT_DOC_BYTES {
        return Err(format!(
            "{what} is {} bytes (cap {MAX_OUTPUT_DOC_BYTES})",
            meta.len()
        ));
    }
    let body = std::fs::read_to_string(path).map_err(|e| format!("read {what}: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("{what} did not parse: {e}"))
}

/// The job's work + output directories, owned by the user adaptors run as
/// (a root agent staging for a rootless adaptor).
fn prepare(cfg: &GuestConfig, work: &Path) -> Result<PathBuf, String> {
    let output = work.join("output");
    std::fs::create_dir_all(&output).map_err(|e| format!("mkdir {}: {e}", output.display()))?;
    if let Some((uid, gid)) = cfg.run_as {
        for dir in [work, &output] {
            std::os::unix::fs::chown(dir, Some(uid), Some(gid))
                .map_err(|e| format!("chown {}: {e}", dir.display()))?;
        }
    }
    Ok(output)
}

fn write_doc<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let body =
        serde_json::to_vec_pretty(value).map_err(|e| format!("encode {}: {e}", path.display()))?;
    std::fs::write(path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

fn run_log(exec: &Exec, secrets: &[Vec<u8>]) -> LogFile {
    LogFile {
        name: "runner.log".into(),
        bytes: redact(&exec.tail, secrets).into_bytes(),
    }
}

/// Last `n` chars of `s`.
fn last_chars(s: &str, n: usize) -> &str {
    let cut = s
        .char_indices()
        .rev()
        .nth(n.saturating_sub(1))
        .map_or(0, |(i, _)| i);
    &s[cut..]
}

fn describe(exec: &Exec, secrets: &[Vec<u8>], deadline_s: u64) -> String {
    if exec.timed_out {
        return format!("in-guest run cut at the deadline of {deadline_s}s; no report is evidence");
    }
    let redacted = redact(&exec.tail, secrets);
    format!(
        "exit {:?}; tail: {}",
        exec.exit,
        last_chars(&redacted, 2_000).trim()
    )
}

/// Baseline / evaluate through the adaptor's `run`.
#[allow(clippy::too_many_arguments)]
pub async fn run_paid(
    cfg: &GuestConfig,
    adaptor: &Adaptor,
    request: &CustomRunRequest,
    kind: JobKind,
    pack: &StagedPack,
    work: &Path,
    artifact: Option<&Path>,
) -> Result<RunOutcome, String> {
    let entry = adaptor.entrypoint(kind)?;
    let output = prepare(cfg, work)?;
    std::fs::write(work.join("claim.txt"), &request.claim).map_err(|e| format!("claim: {e}"))?;
    let mut vars = job_env(
        cfg,
        &adaptor.binding.runner,
        request,
        kind,
        work,
        &output,
        Some(pack),
        artifact,
    )?;
    vars.push((
        env::CLAIM_FILE.to_owned(),
        work.join("claim.txt").display().to_string(),
    ));
    // The miner's own key reaches their own paid run, and only here:
    // inspection ticks rules without spending, so it is never given one.
    let miner_secrets = inject_miner_env(cfg, request, kind, &mut vars)?;
    let mut secrets = secret_values(&cfg.secrets_dir);
    secrets.extend(miner_secrets);
    let exec = exec(
        cfg,
        &entry,
        vars,
        work,
        Duration::from_secs(request.sandbox.deadline_s),
    )
    .await?;
    // Also on timeout / adaptor-fail: retain-on-fail of tbench-x0004
    // looked empty until e2fsck replayed the journal. Do not send Done
    // if this barrier fails (metal tbench-x0002 / a21f).
    persist_work(work)?;
    if exec.timed_out {
        return Err(describe(&exec, &secrets, request.sandbox.deadline_s));
    }
    let report_path = output.join("report.json");
    let report: RunnerReport = read_output_doc(&report_path, "report.json").map_err(|e| {
        format!(
            "{e} ({})",
            describe(&exec, &secrets, request.sandbox.deadline_s)
        )
    })?;
    if !report.primary_value.is_finite() {
        return Err("report.json primary_value is not finite".into());
    }
    let mut evidence = report.evidence;
    for v in evidence.values_mut() {
        redact_value(v, &secrets);
    }
    evidence.insert("runner".into(), serde_json::json!(adaptor.binding.runner));
    evidence.insert("pack_digest".into(), serde_json::json!(pack.digest));
    evidence.insert("exit_code".into(), serde_json::json!(exec.exit));
    let results = (kind == JobKind::Evaluate)
        .then(|| {
            let bind = request.results_bind(report.primary_value, report.claim_holds);
            load_evaluate(&output, &request.constraints.params, &bind)
                .map(|mut v| {
                    redact_value(&mut v, &secrets);
                    v
                })
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    Ok(RunOutcome {
        report: CustomRunReport {
            schema_version: RUN_REPORT_SCHEMA,
            topic_id: request.topic_id.clone(),
            custom_id: request.custom_id.clone(),
            submission_digest: request.submission_digest.clone(),
            artifact_digest: request.artifact_digest.clone(),
            rules_version: request.rules_version,
            primary_value: report.primary_value,
            claim_holds: report.claim_holds,
            sandboxed: true,
            flops_used: report.flops_used,
            evidence,
            results,
        },
        logs: vec![run_log(&exec, &secrets)],
    })
}

/// Walk the unpacked artefact for the inspection record (bounded).
fn collect_artifact(dir: &Path) -> Vec<ArtifactFile> {
    let mut out = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(m) = std::fs::metadata(&p) {
                if out.len() >= MAX_ARTIFACT_FILES
                    || m.len() > MAX_ARTIFACT_FILE_BYTES
                    || total + m.len() > MAX_ARTIFACT_TOTAL_BYTES
                {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&p) {
                    total += m.len();
                    out.push(ArtifactFile {
                        path: p.strip_prefix(dir).unwrap_or(&p).display().to_string(),
                        bytes,
                    });
                }
            }
        }
    }
    out
}

/// Inspection through the adaptor's `inspect`: every rule of `rules` is
/// recorded; a rule the inspector did not answer fails with that as the
/// evidence (red, no spend).
pub async fn inspect(
    cfg: &GuestConfig,
    adaptor: &Adaptor,
    request: &CustomRunRequest,
    rules: &RuleSet,
    work: &Path,
    artifact: Option<&Path>,
) -> Result<InspectOutcome, String> {
    let entry = adaptor.entrypoint(JobKind::Inspect)?;
    let output = prepare(cfg, work)?;
    let rules_file = work.join("rules.json");
    write_doc(&rules_file, rules)?;
    let mut vars = job_env(
        cfg,
        &adaptor.binding.runner,
        request,
        JobKind::Inspect,
        work,
        &output,
        None,
        artifact,
    )?;
    vars.push((env::RULES_FILE.to_owned(), rules_file.display().to_string()));
    let secrets = secret_values(&cfg.secrets_dir);
    let exec = exec(
        cfg,
        &entry,
        vars,
        work,
        Duration::from_secs(request.sandbox.deadline_s),
    )
    .await?;
    if exec.timed_out {
        return Err(describe(&exec, &secrets, request.sandbox.deadline_s));
    }
    let items: Vec<InspectItem> = read_output_doc(&output.join("checklist.json"), "checklist.json")
        .map_err(|e| {
            format!(
                "{e} ({})",
                describe(&exec, &secrets, request.sandbox.deadline_s)
            )
        })?;
    let mut checklist = Checklist::new(rules, &request.submission_digest, &request.artifact_digest);
    for rule in &rules.rules {
        match items.iter().find(|i| i.id == rule.id) {
            Some(i) => {
                checklist.record(&rule.id, i.pass, &redact(&i.evidence, &secrets));
            }
            None => {
                checklist.record(
                    &rule.id,
                    false,
                    "inspector produced no verdict for this rule",
                );
            }
        }
    }
    Ok(InspectOutcome {
        checklist,
        artifact: artifact.map(collect_artifact).unwrap_or_default(),
    })
}

/// Rule proposal: the adaptor's `propose_rules` when the topic selects a
/// runner that ships one, else the signed topic's own checklist.
pub async fn propose_rules(
    cfg: &GuestConfig,
    topic: &TopicDocument,
    current_version: Option<u32>,
    work: &Path,
) -> Result<Vec<ChecklistRule>, String> {
    let adaptor = match Adaptor::binding_of(&topic.constraints.params)? {
        Some(binding) => Adaptor::installed(&cfg.runners_dir, binding).ok(),
        None => None,
    };
    let Some(entry) = adaptor
        .as_ref()
        .and_then(|a| a.entrypoint(JobKind::ProposeRules).ok())
    else {
        tracing::info!(topic_id = %topic.id, "no propose_rules adaptor; proposing the signed checklist");
        return Ok(topic.checklist.clone());
    };
    let adaptor = adaptor.ok_or_else(|| "adaptor vanished".to_owned())?;
    let params = param_env(&topic.constraints.params)?;
    let output = prepare(cfg, work)?;
    let topic_file = work.join("topic.json");
    write_doc(&topic_file, topic)?;
    let mut vars = vec![
        (env::RUNNER_ID.to_owned(), adaptor.binding.runner.clone()),
        (
            env::JOB.to_owned(),
            JobKind::ProposeRules.as_str().to_owned(),
        ),
        (env::TOPIC_ID.to_owned(), topic.id.clone()),
        (env::CUSTOM_ID.to_owned(), topic.metric.custom_id.clone()),
        (env::TOPIC_FILE.to_owned(), topic_file.display().to_string()),
        (env::OUTPUT_DIR.to_owned(), output.display().to_string()),
        (env::WORK_DIR.to_owned(), work.display().to_string()),
        (
            env::SECRETS_DIR.to_owned(),
            cfg.secrets_dir.display().to_string(),
        ),
        (
            env::SECRET_FILES.to_owned(),
            secret_names(&cfg.secrets_dir).join(","),
        ),
        (
            "PROOF_CURRENT_RULES_VERSION".to_owned(),
            current_version.map_or(String::new(), |v| v.to_string()),
        ),
    ];
    vars.extend(params);
    let secrets = secret_values(&cfg.secrets_dir);
    let exec = exec(cfg, &entry, vars, work, DEFAULT_UNPAID_DEADLINE).await?;
    if exec.timed_out {
        return Err("propose_rules cut at its deadline".into());
    }
    let mut rules: Vec<ChecklistRule> = read_output_doc(&output.join("rules.json"), "rules.json")
        .map_err(|e| {
        format!(
            "{e} ({})",
            describe(&exec, &secrets, DEFAULT_UNPAID_DEADLINE.as_secs())
        )
    })?;
    for r in &mut rules {
        r.text = redact(&r.text, &secrets);
    }
    validate_rules(&rules).map_err(|e| format!("rules.json {}: {}", e.field, e.why))?;
    Ok(rules)
}

#[cfg(test)]
mod sync_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::{persist_work, sync_file, sync_tree};
    use std::path::Path;

    #[test]
    fn sync_file_reports_open_failure() {
        let err = sync_file(Path::new("/no/such/proof-vm-guest-sync-file")).unwrap_err();
        assert!(err.contains("sync"), "{err}");
        assert!(err.contains("no/such/proof-vm-guest-sync-file"), "{err}");
    }

    #[test]
    fn sync_tree_reports_missing_root() {
        let err = sync_tree(Path::new("/no/such/proof-vm-guest-sync-tree")).unwrap_err();
        assert!(err.contains("sync"), "{err}");
    }

    #[test]
    fn persist_work_skips_missing_root() {
        persist_work(Path::new("/no/such-proof-vm-guest-persist")).expect("missing is ok");
    }

    #[test]
    fn sync_tree_syncs_a_real_tree() {
        let d = std::env::temp_dir().join(format!("proof-vm-guest-sync-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a"), b"x").unwrap();
        std::fs::write(d.join("sub").join("b"), b"y").unwrap();
        persist_work(&d).expect("persist");
        let _ = std::fs::remove_dir_all(&d);
    }
}
