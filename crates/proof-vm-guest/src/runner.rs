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
//! | `propose_rules` | `ProposeRules` (optional) | `authoring.json` — the whole set ([`AuthoredSet::Complete`]); `rules.json` — `[{"id", "text"}]`, the same vector, read by a guest baked before the set existed |
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
    RunOutcome, TopicAuthoring, RUN_REPORT_SCHEMA,
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
    /// Rule version the RLM is superseding (`propose_rules`; empty = none).
    pub const CURRENT_RULES_VERSION: &str = "PROOF_CURRENT_RULES_VERSION";
    /// Where the set this RLM authored **last time** was written
    /// (`propose_rules`; empty = no previous set). The adaptor reads it to
    /// retain the parts it is not changing.
    pub const CURRENT_AUTHORING_FILE: &str = "PROOF_CURRENT_AUTHORING_FILE";
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
/// Largest `report.json` / `checklist.json` / `authoring.json` / `rules.json`
/// read back.
pub const MAX_OUTPUT_DOC_BYTES: u64 = 8 * 1024 * 1024;

/// The file the RLM writes its whole authored set to (`ProposeRules`).
pub const AUTHORING_FILE: &str = "authoring.json";

/// The file a `propose_rules` adaptor writes a bare rule vector to — the
/// **compat fragment**, also written beside `authoring.json` so a guest baked
/// before the set existed answers instead of failing the job
/// (`adaptor wrote no rules.json`). It is not authorship: the host records
/// what it carries with honest `rlm` provenance and refuses to open a topic
/// on it ([`RULES_ONLY_IS_NOT_AUTHORSHIP`]).
///
/// Not to be confused with [`env::RULES_FILE`], the variable naming the
/// **input** vector an `Inspect` job ticks.
pub const RULES_FILE: &str = "rules.json";

/// The file the guest writes the **previous** set to, for a re-authoring run.
pub const CURRENT_AUTHORING_FILE: &str = "current-authoring.json";
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
    let body = read_output_text(path, what)?;
    serde_json::from_str(&body).map_err(|e| format!("{what} did not parse: {e}"))
}

/// Read a bounded output document as text (the parse is the caller's).
fn read_output_text(path: &Path, what: &str) -> Result<String, String> {
    let meta = std::fs::metadata(path)
        .map_err(|_| format!("adaptor wrote no {what} ({})", path.display()))?;
    if meta.len() > MAX_OUTPUT_DOC_BYTES {
        return Err(format!(
            "{what} is {} bytes (cap {MAX_OUTPUT_DOC_BYTES})",
            meta.len()
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("read {what}: {e}"))
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
    // Non-zero adaptor exit is Failed even when report.json / results.json
    // landed: those files are not a successful Evaluated outcome. Harbor may
    // exit nonzero after measured trials; run-harbor still prints
    // scored-already-measured and exits 0 — that path stays Some(0) here.
    if !matches!(exec.exit, Some(0)) {
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
                .map_err(|e| {
                    // Selected adaptor only (`<runners>/<id>/`), not cfg.runners_dir.
                    proof_results::hint_skew_if_runner_unemitted(e, &adaptor.dir).to_string()
                })
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

/// Why a topic whose runner ships no `propose_rules` entrypoint fails closed.
///
/// The guest never echoes the signed `checklist` back as a rule proposal: doing
/// so would let the control plane record the operator's own vector as
/// RLM-authored rules.
pub const NO_RLM_RULES: &str = "the topic's runner ships no propose_rules entrypoint, so the RLM authored no rules: the signed checklist is the operator's vector (source topic_document), never a substitute for RLM authorship";

/// Why a rules-only proposal cannot open a topic.
///
/// A topic's behavior is authored by its RLM in full: the rule vector, the SQL
/// migrations it needs, the routes it exposes, its submission format, and the
/// pin policy it tightens. An adaptor that writes only `rules.json` has
/// authored one part of five, and the host refuses to fill the rest in — from
/// the operator's bundle or from anywhere else — because that is exactly the
/// operator-cloned document the authorship pin exists to refuse.
pub const RULES_ONLY_IS_NOT_AUTHORSHIP: &str = "the runner wrote rules.json but no authoring.json: a topic's behavior is authored by its own RLM (rules, migrations, apis, submission_format, pin_policy), and a rules-only proposal is not that. Ship an adaptor whose propose_rules writes authoring.json; nothing is installed from the operator's bundle in its place";

/// Why a `propose_rules` run that wrote neither file fails closed.
///
/// The guest has no answer to fall back on: the signed `checklist` is the
/// operator's vector, and echoing it back would record the operator's own
/// rules as [`proof_rlm::RuleSource::Rlm`]. A run that wrote nothing authored
/// nothing.
pub const NO_AUTHORING_OR_RULES: &str = "the runner wrote neither authoring.json (the whole set: rules, migrations, apis, submission_format, pin_policy) nor rules.json (the compat fragment): a propose_rules run that writes nothing authors nothing, and the signed checklist is the operator's vector, never a substitute for RLM authorship";

/// Why a dual-written pair whose vectors disagree fails closed.
///
/// `rules.json` exists so a guest baked before `authoring.json` existed still
/// answers; it is the **same** vector as the set's `rules`, or it is a second
/// answer. A pair that disagrees is refused rather than resolved by preferring
/// one: which file a stale guest read would then decide the topic's anti-cheat
/// surface, and the two halves of one authorship claim could disagree forever.
pub const DUAL_EMIT_RULES_DISAGREE: &str = "the runner wrote authoring.json and rules.json but their rule vectors disagree: rules.json is the compat copy of the set's own rules, not a second answer. A guest baked before the set existed reads rules.json, so a pair that disagrees would make the topic's anti-cheat surface depend on which guest harvested it";

/// What the RLM authored, as the guest read it.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthoredSet {
    /// The full set, from `authoring.json`.
    Complete(Box<TopicAuthoring>),
    /// A rules-only proposal, from `rules.json` (an older adaptor).
    RulesOnly(Vec<ChecklistRule>),
}

/// The RLM authors its whole set inside its VM: **only** the adaptor's
/// `propose_rules`.
///
/// The topic's RLM writes every part of its own behavior there. This function
/// therefore has **no** fallback to the signed document's `checklist` or to
/// anything else the operator wrote: echoing the operator's vector back would
/// let the control plane record parts the RLM never wrote as
/// [`proof_rlm::RuleSource::Rlm`], which is the operator-cloned document
/// masquerading as RLM authorship. A topic whose runner ships no
/// `propose_rules` entrypoint is `Failed` (503, no row, nothing scored) —
/// never silently scored under the operator's own rules.
///
/// The signed `checklist` remains the topic's **version 1**
/// ([`proof_rlm::RuleSet::from_topic`], source `topic_document`) and keeps its
/// honest provenance; only a run of this entrypoint advances the store to
/// `rlm`.
///
/// Two output shapes are read, and the difference matters:
///
/// - `authoring.json` — the whole set ([`AuthoredSet::Complete`]). This is
///   what a topic that must **open** needs, and it is read whichever guest
///   harvests the run: a guest baked before the set existed reads the same
///   answer out of `rules.json` (below) rather than failing the job.
/// - `rules.json` — a bare vector ([`AuthoredSet::RulesOnly`]), kept because
///   an adaptor baked before the set existed still writes it. The host
///   records the rules with honest `rlm` provenance and refuses to open the
///   topic, naming the missing parts
///   ([`RULES_ONLY_IS_NOT_AUTHORSHIP`]) — it does not widen a rules-only
///   answer into a whole set, because the parts it would fill in would be the
///   operator's.
///
/// A dual-written pair is the **same** answer twice, so the compat copy is
/// held to the set's own rules: a pair whose vectors disagree is refused
/// ([`DUAL_EMIT_RULES_DISAGREE`]) rather than resolved by preference, because
/// which guest harvested the run would otherwise decide the topic's vector.
pub async fn propose_rules(
    cfg: &GuestConfig,
    topic: &TopicDocument,
    current_version: Option<u32>,
    current: Option<&TopicAuthoring>,
    work: &Path,
) -> Result<AuthoredSet, String> {
    let binding = Adaptor::binding_of(&topic.constraints.params)?;
    let adaptor = binding.and_then(|b| Adaptor::installed(&cfg.runners_dir, b).ok());
    let Some(entry) = adaptor
        .as_ref()
        .and_then(|a| a.entrypoint(JobKind::ProposeRules).ok())
    else {
        tracing::error!(topic_id = %topic.id, "runner ships no propose_rules entrypoint");
        return Err(NO_RLM_RULES.to_owned());
    };
    let adaptor = adaptor.ok_or_else(|| "adaptor vanished".to_owned())?;
    let params = param_env(&topic.constraints.params)?;
    let output = prepare(cfg, work)?;
    let topic_file = work.join("topic.json");
    write_doc(&topic_file, topic)?;
    // The set this RLM authored last time, when the host has one: the adaptor
    // reads it to **retain** the parts it is not changing. Without it a
    // re-authoring run is a rewrite from nothing — an adaptor cannot keep a
    // migration it still needs, and the install would apply that lossy set.
    let current_file = match current {
        Some(set) => {
            let path = work.join(CURRENT_AUTHORING_FILE);
            write_doc(&path, set)?;
            path.display().to_string()
        }
        None => String::new(),
    };
    let mut vars = vec![
        (env::RUNNER_ID.to_owned(), adaptor.binding.runner.clone()),
        (
            env::JOB.to_owned(),
            JobKind::ProposeRules.as_str().to_owned(),
        ),
        (env::TOPIC_ID.to_owned(), topic.id.clone()),
        (env::CUSTOM_ID.to_owned(), topic.metric.custom_id.clone()),
        (env::TOPIC_FILE.to_owned(), topic_file.display().to_string()),
        // Where the previous set is, if there is one. Always set (empty when
        // there is none) so an adaptor branches on one variable rather than on
        // a variable's presence.
        (env::CURRENT_AUTHORING_FILE.to_owned(), current_file),
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
            env::CURRENT_RULES_VERSION.to_owned(),
            current_version.map_or(String::new(), |v| v.to_string()),
        ),
    ];
    vars.extend(params);
    let secrets = secret_values(&cfg.secrets_dir);
    let exec = exec(cfg, &entry, vars, work, DEFAULT_UNPAID_DEADLINE).await?;
    if exec.timed_out {
        return Err("propose_rules cut at its deadline".into());
    }
    let ctx = describe(&exec, &secrets, DEFAULT_UNPAID_DEADLINE.as_secs());
    read_authored_set(topic, &output, &secrets, &ctx)
}

/// Read what a `propose_rules` run wrote, or refuse.
///
/// The two files are one answer, and the precedence between them is the whole
/// point:
///
/// - `authoring.json` present → [`AuthoredSet::Complete`], held to the shape,
///   the deny-list, and the policy-vs-document checks the control plane runs.
///   A `rules.json` beside it must carry **the same** vector
///   ([`DUAL_EMIT_RULES_DISAGREE`]), because a guest baked before the set
///   existed harvests this run out of that file.
/// - only `rules.json` → [`AuthoredSet::RulesOnly`], the compat fragment an
///   older adaptor answers with. The host records it with honest `rlm`
///   provenance and refuses to open a topic on it.
/// - neither → [`NO_AUTHORING_OR_RULES`]. A run that wrote nothing authored
///   nothing, and the signed `checklist` is the operator's vector.
///
/// `ctx` is the failure context of the run that produced the files (its exit
/// status and redacted tail), appended to a read failure so the operator sees
/// why the adaptor wrote nothing.
fn read_authored_set(
    topic: &TopicDocument,
    output: &Path,
    secrets: &[Vec<u8>],
    ctx: &str,
) -> Result<AuthoredSet, String> {
    let authoring_path = output.join(AUTHORING_FILE);
    let rules_path = output.join(RULES_FILE);
    if authoring_path.is_file() {
        let body = read_output_text(&authoring_path, AUTHORING_FILE)
            .map_err(|e| format!("{e} ({ctx})"))?;
        let mut set =
            proof_rlm::authoring_from_json(&body).map_err(|e| format!("{AUTHORING_FILE}: {e}"))?;
        if set.topic_id.trim() != topic.id.trim() {
            return Err(format!(
                "{AUTHORING_FILE} is for topic {:?}, this VM is bound to {:?}",
                set.topic_id, topic.id
            ));
        }
        // The guest holds the set to the same checks the control plane runs,
        // minus the pin (which it does not have): shape, the migration
        // deny-list, and the policy against the document's own knobs.
        set.validate(&topic.id)
            .map_err(|e| format!("{AUTHORING_FILE}: {e}"))?;
        set.pin_policy
            .agrees_with_document(topic)
            .map_err(|e| format!("{AUTHORING_FILE}: {e}"))?;
        if rules_path.is_file() {
            let compat: Vec<ChecklistRule> =
                read_output_doc(&rules_path, RULES_FILE).map_err(|e| format!("{e} ({ctx})"))?;
            // Compared before redaction: the two files carry the same text, so
            // a secret in one is a secret in the other, and redacting first
            // would compare two redactions rather than the RLM's answer.
            if compat != set.rules {
                return Err(format!(
                    "{DUAL_EMIT_RULES_DISAGREE} ({AUTHORING_FILE} carries {} rules, \
                     {RULES_FILE} carries {})",
                    set.rules.len(),
                    compat.len()
                ));
            }
        }
        for rule in &mut set.rules {
            rule.text = redact(&rule.text, secrets);
        }
        return Ok(AuthoredSet::Complete(Box::new(set)));
    }
    if !rules_path.is_file() {
        // Neither file: the run authored nothing. The signed checklist is the
        // operator's vector, so there is no answer to fall back on.
        return Err(format!("{NO_AUTHORING_OR_RULES} ({ctx})"));
    }
    let mut rules: Vec<ChecklistRule> =
        read_output_doc(&rules_path, RULES_FILE).map_err(|e| format!("{e} ({ctx})"))?;
    for r in &mut rules {
        r.text = redact(&r.text, secrets);
    }
    validate_rules(&rules).map_err(|e| format!("{RULES_FILE} {}: {}", e.field, e.why))?;
    Ok(AuthoredSet::RulesOnly(rules))
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

/// The harvest reader itself, without a VM: which file decides, and what a
/// pair of files means.
#[cfg(test)]
mod authored_set_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::{
        read_authored_set, AuthoredSet, AUTHORING_FILE, DUAL_EMIT_RULES_DISAGREE,
        NO_AUTHORING_OR_RULES, RULES_FILE,
    };
    use proof_task::TopicDocument;
    use std::path::Path;

    fn topic() -> TopicDocument {
        let mut doc = TopicDocument {
            id: "topic-a".into(),
            ..TopicDocument::default()
        };
        doc.metric.family = proof_task::MetricFamily::Custom;
        doc.metric.custom_id = "custom_a".into();
        doc
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "proof-vm-guest-authored-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    fn set_body(rules: &str) -> String {
        format!(
            r#"{{"schema_version":1,"topic_id":"topic-a","rules":{rules},"migrations":[{{"name":"0001_scratch","sql":"CREATE TABLE topic_a_scratch (id TEXT)"}}],"apis":[{{"path":"status","method":"GET"}}],"submission_format":{{"kind":"tar"}},"pin_policy":{{}}}}"#
        )
    }

    /// `authoring.json` alone is the whole set, and it is what the harvest
    /// reads first — the live FAIL was a guest that read only `rules.json`.
    #[test]
    fn the_set_is_harvested_from_authoring_json() {
        let d = dir("set-only");
        std::fs::write(
            d.join(AUTHORING_FILE),
            set_body(r#"[{"id":"rlm_rule","text":"t"}]"#),
        )
        .expect("write set");
        let out = read_authored_set(&topic(), &d, &[], "ctx").expect("the set is read");
        let AuthoredSet::Complete(set) = out else {
            panic!("a set is authorship, got {out:?}");
        };
        assert_eq!(set.rules[0].id, "rlm_rule");
        assert!(set.is_complete());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A dual-written pair whose vectors agree is the same answer twice, so
    /// the harvest answers the set (and the compat copy is never the answer).
    #[test]
    fn a_dual_emit_pair_that_agrees_harvests_the_set() {
        let d = dir("pair-agrees");
        let rules = r#"[{"id":"rlm_rule","text":"t"}]"#;
        std::fs::write(d.join(AUTHORING_FILE), set_body(rules)).expect("write set");
        std::fs::write(d.join(RULES_FILE), rules).expect("write fragment");
        let out = read_authored_set(&topic(), &d, &[], "ctx").expect("the pair is read");
        assert!(
            matches!(out, AuthoredSet::Complete(_)),
            "an agreeing pair is the whole set, got {out:?}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A pair that disagrees is refused by name: which guest harvested the run
    /// must not decide the topic's vector.
    #[test]
    fn a_dual_emit_pair_that_disagrees_is_refused() {
        let d = dir("pair-disagrees");
        std::fs::write(
            d.join(AUTHORING_FILE),
            set_body(r#"[{"id":"rlm_rule","text":"t"}]"#),
        )
        .expect("write set");
        std::fs::write(
            d.join(RULES_FILE),
            r#"[{"id":"a_different_rule","text":"u"}]"#,
        )
        .expect("write fragment");
        let err =
            read_authored_set(&topic(), &d, &[], "ctx").expect_err("a disagreement is refused");
        assert!(err.contains(DUAL_EMIT_RULES_DISAGREE), "{err}");
        assert!(err.contains("carries 1 rules"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Neither file: nothing was authored, and there is no fallback.
    #[test]
    fn a_run_that_wrote_neither_file_is_refused() {
        let d = dir("neither");
        let err = read_authored_set(&topic(), &d, &[], "the run exited 0").expect_err("nothing");
        assert!(err.contains(NO_AUTHORING_OR_RULES), "{err}");
        assert!(
            err.contains("the run exited 0"),
            "the refusal carries the run's own context: {err}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `rules.json` alone stays a fragment: the guest never widens it.
    #[test]
    fn a_rules_only_run_stays_a_fragment() {
        let d = dir("rules-only");
        std::fs::write(d.join(RULES_FILE), r#"[{"id":"compat_rule","text":"t"}]"#).expect("write");
        let out = read_authored_set(&topic(), &d, &[], "ctx").expect("a fragment is read");
        let AuthoredSet::RulesOnly(rules) = out else {
            panic!("a fragment stays a fragment, got {out:?}");
        };
        assert_eq!(rules[0].id, "compat_rule");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A set for another topic is refused however it is paired.
    #[test]
    fn a_set_for_another_topic_is_refused() {
        let d = dir("wrong-topic");
        std::fs::write(
            d.join(AUTHORING_FILE),
            set_body(r#"[{"id":"rlm_rule","text":"t"}]"#).replace("topic-a", "topic-b"),
        )
        .expect("write set");
        let err = read_authored_set(&topic(), &d, &[], "ctx").expect_err("wrong topic");
        assert!(err.contains("is for topic"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The compat file's own shape is still checked when it is the answer: a
    /// malformed fragment is refused with the file named, not silently empty.
    #[test]
    fn a_malformed_fragment_is_refused_by_name() {
        let d = dir("bad-fragment");
        std::fs::write(d.join(RULES_FILE), r#"[{"id":"Bad Id","text":"t"}]"#).expect("write");
        let err = read_authored_set(&topic(), &d, &[], "ctx").expect_err("bad id");
        assert!(err.contains(RULES_FILE), "{err}");
        assert!(err.contains("Bad Id"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A redacted secret never reaches the set's rules.
    #[test]
    fn a_staged_secret_is_redacted_out_of_the_harvested_rules() {
        let d = dir("redacted");
        std::fs::write(
            d.join(AUTHORING_FILE),
            set_body(r#"[{"id":"rlm_rule","text":"uses s3cret-value here"}]"#),
        )
        .expect("write set");
        let out =
            read_authored_set(&topic(), &d, &[b"s3cret-value".to_vec()], "ctx").expect("the set");
        let AuthoredSet::Complete(set) = out else {
            panic!("a set, got {out:?}");
        };
        assert!(
            !set.rules[0].text.contains("s3cret-value"),
            "a staged secret travels in a rule text: {}",
            set.rules[0].text
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The reader takes a directory, and an absent one is "nothing written".
    #[test]
    fn a_missing_output_directory_reads_as_nothing_written() {
        let err = read_authored_set(
            &topic(),
            Path::new("/no/such-proof-vm-guest-output-dir"),
            &[],
            "ctx",
        )
        .expect_err("nothing written");
        assert!(err.contains(NO_AUTHORING_OR_RULES), "{err}");
    }
}
