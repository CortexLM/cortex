//! Obligatory display/audit JSON for a successful Proof custom evaluate.
//!
//! The adaptor writes this file next to `report.json`. The guest, harvest
//! reconstruct, and RLM scorer all bind it to the scored facts
//! (`primary_value`, `claim_holds`, identities) and refuse a pass when it
//! is missing or non-conforming. Consensus scoring still reads only
//! `report.json`; this document must **match** those facts, never invent a
//! second primary.
//!
//! A topic may pin the contract and file name in signed
//! `constraints.params` ([`PARAM_RESULTS_CONTRACT`], [`PARAM_RESULTS_PATH`]).
//! Absent pin: the file's own `contract` must be a known id. Absent path:
//! [`RESULTS_FILE`].

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Only accepted `schema_version` of a results document.
pub const RESULTS_SCHEMA: u32 = 1;

/// Default file name under `PROOF_OUTPUT_DIR` and at the artefact zip root.
pub const RESULTS_FILE: &str = "results.json";

/// Host RCA when the **selected** guest runner lacks the results-emit helper.
///
/// Report-only (`report.json` without `results.json`) is **not** this by
/// itself — that is [`missing_results_detail`]. Append this only when
/// [`runner_tree_emits_results`] is false for **that** runner directory
/// (no `.py` carrying `write_results_next_to_report`). A sibling runner or
/// a notes file does not count. Tipping gateway / challenge alone does not
/// update the in-guest adaptor.
pub const PIN_RUNNER_SKEW_HINT: &str = "guest pin/runner skew: rebake the guest image so /opt/proof/runners matches deploy/guest/runners (tipping gateway/challenge alone is insufficient)";

/// Pathless [`require_evaluate`]: the scored `CustomRunReport` carried no
/// `results` field. Guest Done+results can still be attested while an old
/// orch KEEP drops the field; tip the host past CustomRunReport.results /
/// #294 harvest attach. Not pin/runner rebake.
pub const ORCH_RESULTS_ATTACH_HINT: &str =
    "host orch tip past CustomRunReport.results / #294 harvest attach";

/// Source marker Harbor / tip runners use to write `results.json` next to
/// `report.json`. Absence from the guest runner tree is the skew probe.
pub const WRITE_RESULTS_EMIT: &str = "write_results_next_to_report";

/// Default results document cap (bytes). [`CONTRACT_GENERIC`] and any
/// unidentified `contract` stay on this prior limit.
///
/// Guest summarize prefers 8 KiB `agent_log` / `verifier_log` and shrinks
/// 8 → 4 → 2 KiB, then omits those bodies, measuring the encoded JSON it
/// writes so replacement chars / escapes cannot 503 a paid score.
/// Job-level `logs.harbor_run_tail` is unchanged. A document that still
/// exceeds the **selected** contract's cap is fail-closed (`TooLarge`).
pub const MAX_RESULTS_BYTES: u64 = 256 * 1024;

/// Harbor family cap (bytes): [`CONTRACT_HARBOR_TRIALS`] /
/// [`CONTRACT_TBENCH_HARBOR`] only, after `contract` is identified.
///
/// [`load_file`] may *read* up to this ceiling so a Harbor document between
/// [`MAX_RESULTS_BYTES`] and this size can parse; generic-custom-v1 over
/// [`MAX_RESULTS_BYTES`] is still `TooLarge`.
pub const MAX_HARBOR_RESULTS_BYTES: u64 = 512 * 1024;

/// Signed `constraints.params` key pinning the results contract id.
pub const PARAM_RESULTS_CONTRACT: &str = "results_contract";

/// Signed `constraints.params` key naming the results file (one `.json` segment).
pub const PARAM_RESULTS_PATH: &str = "results_path";

/// Envelope-only contract every custom evaluate may satisfy.
pub const CONTRACT_GENERIC: &str = "generic-custom-v1";

/// Harbor trial-summary contract: the generic id every Harbor-scored topic
/// pins.
pub const CONTRACT_HARBOR_TRIALS: &str = "harbor-trials-v1";

/// Legacy alias of [`CONTRACT_HARBOR_TRIALS`], kept because it is a **wire
/// value**: a topic signed before the generic id existed pins this in its
/// `constraints.params.results_contract`, and a signed document cannot be
/// edited. New topics pin [`CONTRACT_HARBOR_TRIALS`]; the guest harness
/// accepts both, and nothing branches on a topic.
pub const CONTRACT_TBENCH_HARBOR: &str = "tbench-harbor-v1";

/// Harbor trial that produced a verifier reward.
pub const HARBOR_OUTCOME_MEASURED: &str = "measured";

/// Harbor trial the miner's harness crashed on (`agent_exception_policy=zero`).
pub const HARBOR_OUTCOME_EXCEPTION: &str = "agent_exception";

/// Why a results document is not evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResultsError {
    /// File missing or unreadable.
    #[error("results json: {0}")]
    Io(String),
    /// Over the size cap selected for this document's contract.
    #[error("results json is {got} bytes (cap {cap})")]
    TooLarge {
        /// Observed size.
        got: u64,
        /// Cap that applied (`MAX_RESULTS_BYTES` or Harbor's larger allowance).
        cap: u64,
    },
    /// JSON did not parse or was not an object.
    #[error("results json: {0}")]
    Parse(String),
    /// Schema drift.
    #[error("results json schema_version {got}, this build reads {RESULTS_SCHEMA}")]
    WrongSchema {
        /// What the document said.
        got: u64,
    },
    /// `contract` is missing or not a known id.
    #[error("results json contract {0:?} is not a known results contract")]
    UnknownContract(String),
    /// Topic pin does not match the document.
    #[error("results json contract {got:?} does not match topic pin {pinned:?}")]
    ContractMismatch {
        /// Document `contract`.
        got: String,
        /// Topic `results_contract`.
        pinned: String,
    },
    /// A required field is missing or the wrong JSON type.
    #[error("results json {0}")]
    Shape(&'static str),
    /// A binding field does not echo the scored report.
    #[error("results json {0} does not match the scored report")]
    Mismatch(&'static str),
    /// `results_path` is not a single safe `.json` file name.
    #[error("results_path {0:?} is not a single safe .json file name")]
    BadPath(String),
}

/// Scored facts the results document must echo.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportBind<'a> {
    /// Topic run for.
    pub topic_id: &'a str,
    /// Custom metric id.
    pub custom_id: &'a str,
    /// Frozen submission digest.
    pub submission_digest: &'a str,
    /// Artefact digest that was run.
    pub artifact_digest: &'a str,
    /// Paid primary (becomes `custom_value`).
    pub primary_value: f64,
    /// Whether the miner's claim held.
    pub claim_holds: bool,
}

impl<'a> ReportBind<'a> {
    /// Bind identities and the scored primary / claim.
    #[must_use]
    pub const fn new(
        topic_id: &'a str,
        custom_id: &'a str,
        submission_digest: &'a str,
        artifact_digest: &'a str,
        primary_value: f64,
        claim_holds: bool,
    ) -> Self {
        Self {
            topic_id,
            custom_id,
            submission_digest,
            artifact_digest,
            primary_value,
            claim_holds,
        }
    }
}

/// Resolved contract family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contract {
    /// Envelope + a non-empty `display` object.
    Generic,
    /// Harbor trial table.
    HarborTrials,
}

/// Known contract id → family. [`CONTRACT_TBENCH_HARBOR`] is the legacy
/// spelling of [`CONTRACT_HARBOR_TRIALS`].
#[must_use]
pub fn known_contract(id: &str) -> Option<Contract> {
    match id.trim() {
        CONTRACT_GENERIC => Some(Contract::Generic),
        CONTRACT_HARBOR_TRIALS | CONTRACT_TBENCH_HARBOR => Some(Contract::HarborTrials),
        _ => None,
    }
}

/// Byte cap for a results document **after** its `contract` is known.
///
/// Harbor ids get [`MAX_HARBOR_RESULTS_BYTES`]. Everything else, including
/// an unknown or missing id, keeps [`MAX_RESULTS_BYTES`].
#[must_use]
pub fn results_size_cap(contract_id: &str) -> u64 {
    match known_contract(contract_id) {
        Some(Contract::HarborTrials) => MAX_HARBOR_RESULTS_BYTES,
        Some(Contract::Generic) | None => MAX_RESULTS_BYTES,
    }
}

fn results_size_cap_for_value(value: &Value) -> u64 {
    value
        .get("contract")
        .and_then(Value::as_str)
        .map_or(MAX_RESULTS_BYTES, results_size_cap)
}

fn reject_too_large(got: u64, cap: u64) -> Result<(), ResultsError> {
    if got > cap {
        Err(ResultsError::TooLarge { got, cap })
    } else {
        Ok(())
    }
}

fn encoded_len(value: &Value) -> Result<u64, ResultsError> {
    let bytes = serde_json::to_vec(value).map_err(|e| ResultsError::Parse(e.to_string()))?;
    Ok(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
}

/// Single path segment, ends with `.json`, conservative charset.
#[must_use]
pub fn is_results_file_name(name: &str) -> bool {
    let b = name.as_bytes();
    (8..=64).contains(&b.len())
        && Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        && !name.contains('/')
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        && !name.starts_with('.')
        && name != "."
        && name != ".."
}

/// File name the adaptor must write (topic pin or [`RESULTS_FILE`]).
pub fn results_file_name(params: &BTreeMap<String, String>) -> Result<String, ResultsError> {
    match params
        .get(PARAM_RESULTS_PATH)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        None => Ok(RESULTS_FILE.to_owned()),
        Some(name) if is_results_file_name(name) => Ok(name.to_owned()),
        Some(name) => Err(ResultsError::BadPath(name.to_owned())),
    }
}

/// Topic pin, if any. Unknown id is a fail-closed pin, not a silent drop.
pub fn pinned_contract(params: &BTreeMap<String, String>) -> Result<Option<String>, ResultsError> {
    match params
        .get(PARAM_RESULTS_CONTRACT)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        None => Ok(None),
        Some(id) if known_contract(id).is_some() => Ok(Some(id.to_owned())),
        Some(id) => Err(ResultsError::UnknownContract(id.to_owned())),
    }
}

/// `{dir}/{results_file_name(params)}`.
pub fn results_path(
    dir: &Path,
    params: &BTreeMap<String, String>,
) -> Result<PathBuf, ResultsError> {
    Ok(dir.join(results_file_name(params)?))
}

/// Load, parse, and bind the results file under `dir` (topic path or default).
///
/// A missing file is always [`missing_results_detail`] — report-only is
/// not pin/runner skew. Call [`hint_skew_if_runner_unemitted`] when the
/// caller can probe the **selected** runner directory.
pub fn load_evaluate(
    dir: &Path,
    params: &BTreeMap<String, String>,
    bind: &ReportBind<'_>,
) -> Result<Value, ResultsError> {
    let path = results_path(dir, params)?;
    let value = load_file(&path)?;
    let pinned = pinned_contract(params)?;
    validate(&value, bind, pinned.as_deref())?;
    Ok(value)
}

/// Path-only missing-file text. No rebake diagnosis.
#[must_use]
pub fn missing_results_detail(name: &str, path: Option<&Path>) -> String {
    match path {
        Some(p) => format!("adaptor wrote no {name} ({})", p.display()),
        None => format!("adaptor wrote no {name}"),
    }
}

/// Missing results after a scored `report.json`. Same text as
/// [`missing_results_detail`] — do not blame rebake from this alone.
#[must_use]
pub fn report_only_missing_results_detail(name: &str, path: Option<&Path>) -> String {
    missing_results_detail(name, path)
}

/// Harbor production emit paths under the **selected** runner directory.
/// Never walk `runners_dir` or sibling adaptors.
const EMIT_PROBE: &[&str] = &["summarize.py", "harness/summarize.py"];

/// True when the selected runner's Harbor helper carries [`WRITE_RESULTS_EMIT`].
#[must_use]
pub fn runner_tree_emits_results(runner_dir: &Path) -> bool {
    EMIT_PROBE.iter().any(|rel| py_emits(&runner_dir.join(rel)))
}

/// Max adaptor source file we will scan for [`WRITE_RESULTS_EMIT`].
/// Not a results.json allowance (generic stays [`MAX_RESULTS_BYTES`]).
const MAX_EMIT_PROBE_BYTES: u64 = 512 * 1024;

fn py_emits(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_EMIT_PROBE_BYTES {
        return false;
    }
    std::fs::read_to_string(path).is_ok_and(|body| body.contains(WRITE_RESULTS_EMIT))
}

/// Append [`PIN_RUNNER_SKEW_HINT`] only when `runner_dir` (the selected
/// adaptor) lacks the emit helper. A runner that already emits is not
/// pin/runner skew.
#[must_use]
pub fn hint_skew_if_runner_unemitted(err: ResultsError, runner_dir: &Path) -> ResultsError {
    if runner_tree_emits_results(runner_dir) {
        err
    } else {
        with_report_only_skew_hint(err)
    }
}

/// Unconditional skew suffix (tests / probe-failed path only).
#[must_use]
pub fn with_report_only_skew_hint(err: ResultsError) -> ResultsError {
    match err {
        ResultsError::Io(s) if !s.contains(PIN_RUNNER_SKEW_HINT) => {
            ResultsError::Io(format!("{s}; {PIN_RUNNER_SKEW_HINT}"))
        }
        other => other,
    }
}

/// Read and parse a results file. Does not bind scored facts.
///
/// Preserves the OS error. Does **not** diagnose pin/runner skew — a
/// dangling path is not a guest-image miss.
pub fn load_file(path: &Path) -> Result<Value, ResultsError> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(RESULTS_FILE);
    let meta = std::fs::metadata(path).map_err(|e| {
        ResultsError::Io(format!("{}: {e}", missing_results_detail(name, Some(path))))
    })?;
    // Hard read ceiling is Harbor's max so a 257–512 KiB Harbor file can
    // parse. Acceptance of that size happens only after `contract` is a
    // Harbor id — generic-custom-v1 keeps MAX_RESULTS_BYTES.
    reject_too_large(meta.len(), MAX_HARBOR_RESULTS_BYTES)?;
    let body = std::fs::read_to_string(path).map_err(|e| ResultsError::Io(e.to_string()))?;
    let got = u64::try_from(body.len()).unwrap_or(u64::MAX);
    reject_too_large(got, MAX_HARBOR_RESULTS_BYTES)?;
    let value = parse_results(&body)?;
    reject_too_large(got, results_size_cap_for_value(&value))?;
    Ok(value)
}

/// Parse results JSON text.
pub fn parse_results(body: &str) -> Result<Value, ResultsError> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| ResultsError::Parse(e.to_string()))?;
    if !value.is_object() {
        return Err(ResultsError::Parse("document is not a JSON object".into()));
    }
    Ok(value)
}

/// Bind `value` to `bind` under an optional topic pin.
pub fn validate(
    value: &Value,
    bind: &ReportBind<'_>,
    pinned: Option<&str>,
) -> Result<(), ResultsError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ResultsError::Parse("document is not a JSON object".into()))?;
    let schema = uint_field(obj, "schema_version")?;
    if schema != u64::from(RESULTS_SCHEMA) {
        return Err(ResultsError::WrongSchema { got: schema });
    }
    let contract = str_field(obj, "contract")?;
    let family = known_contract(contract)
        .ok_or_else(|| ResultsError::UnknownContract(contract.to_owned()))?;
    reject_too_large(encoded_len(value)?, results_size_cap(contract))?;
    if let Some(pin) = pinned.map(str::trim).filter(|s| !s.is_empty()) {
        let pin_fam =
            known_contract(pin).ok_or_else(|| ResultsError::UnknownContract(pin.to_owned()))?;
        if pin_fam != family {
            return Err(ResultsError::ContractMismatch {
                got: contract.to_owned(),
                pinned: pin.to_owned(),
            });
        }
    }
    same_str(obj, "topic_id", bind.topic_id)?;
    same_str(obj, "custom_id", bind.custom_id)?;
    same_str(obj, "submission_digest", bind.submission_digest)?;
    same_hex(obj, "artifact_digest", bind.artifact_digest)?;
    let primary = finite_field(obj, "primary_value")?;
    if !close(primary, bind.primary_value) {
        return Err(ResultsError::Mismatch("primary_value"));
    }
    let claim = bool_field(obj, "claim_holds")?;
    if claim != bind.claim_holds {
        return Err(ResultsError::Mismatch("claim_holds"));
    }
    match family {
        Contract::Generic => validate_generic(obj),
        Contract::HarborTrials => validate_harbor(obj, primary),
    }
}

/// Require `report_results` on a paid evaluate and bind it.
///
/// `None` is a **pathless** miss: the report object had no `results` field
/// (orch KEEP drop / missing harvest attach), not a guest file path. Hint
/// [`ORCH_RESULTS_ATTACH_HINT`], never [`PIN_RUNNER_SKEW_HINT`].
pub fn require_evaluate(
    report_results: Option<&Value>,
    bind: &ReportBind<'_>,
    params: &BTreeMap<String, String>,
) -> Result<Value, ResultsError> {
    let pinned = pinned_contract(params)?;
    let name = results_file_name(params)?;
    let value = report_results.cloned().ok_or_else(|| {
        ResultsError::Io(format!(
            "{}; {ORCH_RESULTS_ATTACH_HINT}",
            missing_results_detail(&name, None)
        ))
    })?;
    validate(&value, bind, pinned.as_deref())?;
    Ok(value)
}

/// Envelope for tests and in-process fixtures ([`CONTRACT_GENERIC`]).
#[must_use]
pub fn generic_document(bind: &ReportBind<'_>, display: &Value) -> Value {
    serde_json::json!({
        "schema_version": RESULTS_SCHEMA,
        "contract": CONTRACT_GENERIC,
        "topic_id": bind.topic_id,
        "custom_id": bind.custom_id,
        "submission_digest": bind.submission_digest,
        "artifact_digest": bind.artifact_digest,
        "primary_value": bind.primary_value,
        "claim_holds": bind.claim_holds,
        "display": display,
    })
}

fn validate_generic(obj: &Map<String, Value>) -> Result<(), ResultsError> {
    let display = obj
        .get("display")
        .and_then(Value::as_object)
        .ok_or(ResultsError::Shape("display must be a JSON object"))?;
    if display.is_empty() {
        return Err(ResultsError::Shape(
            "display must carry at least one field (not primary_value alone)",
        ));
    }
    Ok(())
}

fn validate_harbor(obj: &Map<String, Value>, primary: f64) -> Result<(), ResultsError> {
    let trials = obj
        .get("trials")
        .and_then(Value::as_array)
        .ok_or(ResultsError::Shape("trials must be a JSON array"))?;
    if trials.is_empty() {
        return Err(ResultsError::Shape("trials must not be empty"));
    }
    let mut rewards = Vec::with_capacity(trials.len());
    let mut n_measured = 0u64;
    let mut n_exceptions = 0u64;
    for t in trials {
        let t = t
            .as_object()
            .ok_or(ResultsError::Shape("each trial must be a JSON object"))?;
        let name = t
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or(ResultsError::Shape("trial.name must be a non-empty string"))?;
        let _ = name;
        let reward = t
            .get("reward")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
            .ok_or(ResultsError::Shape("trial.reward must be a finite number"))?;
        let outcome = t
            .get("outcome")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or(ResultsError::Shape(
                "trial.outcome must be measured or agent_exception",
            ))?;
        match outcome {
            HARBOR_OUTCOME_MEASURED => n_measured = n_measured.saturating_add(1),
            HARBOR_OUTCOME_EXCEPTION => n_exceptions = n_exceptions.saturating_add(1),
            _ => {
                return Err(ResultsError::Shape(
                    "trial.outcome must be measured or agent_exception",
                ));
            }
        }
        optional_trial_log_fields(t)?;
        rewards.push(reward);
    }
    let n_scored = uint_field(obj, "n_scored")?;
    if n_scored != trials.len() as u64 {
        return Err(ResultsError::Shape("n_scored must equal trials.len()"));
    }
    if uint_field(obj, "n_measured")? != n_measured {
        return Err(ResultsError::Shape(
            "n_measured must equal the number of measured trials",
        ));
    }
    if uint_field(obj, "n_agent_exceptions")? != n_exceptions {
        return Err(ResultsError::Shape(
            "n_agent_exceptions must equal agent_exception trials",
        ));
    }
    let mean = finite_field(obj, "mean_reward")?;
    if !close(mean, primary) {
        return Err(ResultsError::Mismatch("mean_reward"));
    }
    let n = u32::try_from(rewards.len()).map_err(|_| ResultsError::Shape("too many trials"))?;
    let computed = rewards.iter().sum::<f64>() / f64::from(n);
    if !close(computed, primary) {
        return Err(ResultsError::Mismatch("primary_value"));
    }
    let agent = str_field(obj, "agent")?;
    if agent.is_empty() {
        return Err(ResultsError::Shape("agent must be a non-empty string"));
    }
    let logs = obj
        .get("logs")
        .and_then(Value::as_object)
        .ok_or(ResultsError::Shape("logs must be a JSON object"))?;
    let tail = logs.get("harbor_run_tail").and_then(Value::as_str);
    let path = logs.get("harbor_run_log").and_then(Value::as_str);
    if tail.is_none() && path.is_none() {
        return Err(ResultsError::Shape(
            "logs must carry harbor_run_tail and/or harbor_run_log",
        ));
    }
    Ok(())
}

/// Optional per-trial Harbor logs. Not required (`agent_log` / `verifier_log`
/// / `log_sources` may be omitted). A present value of the wrong JSON type
/// is fail-closed so the frontend never treats a non-string as a log body.
fn optional_trial_log_fields(t: &Map<String, Value>) -> Result<(), ResultsError> {
    if t.get("agent_log").is_some_and(|v| !v.is_string()) {
        return Err(ResultsError::Shape(
            "trial.agent_log must be a string when present",
        ));
    }
    if t.get("verifier_log").is_some_and(|v| !v.is_string()) {
        return Err(ResultsError::Shape(
            "trial.verifier_log must be a string when present",
        ));
    }
    if let Some(v) = t.get("log_sources") {
        let arr = v.as_array().ok_or(ResultsError::Shape(
            "trial.log_sources must be an array of strings when present",
        ))?;
        if arr.iter().any(|item| !item.is_string()) {
            return Err(ResultsError::Shape(
                "trial.log_sources must be an array of strings when present",
            ));
        }
    }
    Ok(())
}

fn str_field<'a>(obj: &'a Map<String, Value>, key: &'static str) -> Result<&'a str, ResultsError> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or(ResultsError::Shape(field_must(key)))
}

fn uint_field(obj: &Map<String, Value>, key: &'static str) -> Result<u64, ResultsError> {
    obj.get(key)
        .and_then(Value::as_u64)
        .ok_or(ResultsError::Shape(field_must(key)))
}

fn finite_field(obj: &Map<String, Value>, key: &'static str) -> Result<f64, ResultsError> {
    obj.get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or(ResultsError::Shape(field_must(key)))
}

fn bool_field(obj: &Map<String, Value>, key: &'static str) -> Result<bool, ResultsError> {
    obj.get(key)
        .and_then(Value::as_bool)
        .ok_or(ResultsError::Shape(field_must(key)))
}

fn same_str(
    obj: &Map<String, Value>,
    key: &'static str,
    expected: &str,
) -> Result<(), ResultsError> {
    if str_field(obj, key)? == expected.trim() {
        Ok(())
    } else {
        Err(ResultsError::Mismatch(key))
    }
}

fn same_hex(
    obj: &Map<String, Value>,
    key: &'static str,
    expected: &str,
) -> Result<(), ResultsError> {
    if str_field(obj, key)?.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(ResultsError::Mismatch(key))
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

fn field_must(key: &'static str) -> &'static str {
    match key {
        "schema_version" => "schema_version must be an unsigned integer",
        "contract" => "contract must be a string",
        "topic_id" => "topic_id must be a string",
        "custom_id" => "custom_id must be a string",
        "submission_digest" => "submission_digest must be a string",
        "artifact_digest" => "artifact_digest must be a string",
        "primary_value" => "primary_value must be a finite number",
        "claim_holds" => "claim_holds must be a boolean",
        "n_scored" => "n_scored must be an unsigned integer",
        "n_measured" => "n_measured must be an unsigned integer",
        "n_agent_exceptions" => "n_agent_exceptions must be an unsigned integer",
        "mean_reward" => "mean_reward must be a finite number",
        "agent" => "agent must be a string",
        _ => "field has the wrong type",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind() -> ReportBind<'static> {
        ReportBind {
            topic_id: "tbench-x",
            custom_id: "harbor_mean",
            submission_digest: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            artifact_digest: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            primary_value: 0.5,
            claim_holds: true,
        }
    }

    fn harbor_ok(bind: &ReportBind<'_>) -> Value {
        serde_json::json!({
            "schema_version": 1,
            "contract": CONTRACT_HARBOR_TRIALS,
            "topic_id": bind.topic_id,
            "custom_id": bind.custom_id,
            "submission_digest": bind.submission_digest,
            "artifact_digest": bind.artifact_digest,
            "primary_value": 0.5,
            "claim_holds": true,
            "n_scored": 2,
            "n_measured": 1,
            "n_agent_exceptions": 1,
            "mean_reward": 0.5,
            "agent": "proof_python_agent:ProofPythonAgent",
            "agent_source": "artifact_dir/recipe/agent",
            "harness_kind": "python",
            "agent_exception_policy": "zero",
            "harbor_exit": 0,
            "trials": [
                {
                    "name": "task-a__1",
                    "reward": 1.0,
                    "outcome": "measured",
                    "agent_log": "agent: hello\n",
                    "verifier_log": "verifier: ok\n",
                    "log_sources": ["trial.log", "verifier/test-stdout.txt"]
                },
                {
                    "name": "task-b__1",
                    "reward": 0.0,
                    "outcome": "agent_exception",
                    "exception_type": "RuntimeError",
                    "exception_message": "Command timed out"
                }
            ],
            "logs": {
                "harbor_run_log": "logs/harbor.run.log",
                "harbor_run_tail": "harbor: done"
            }
        })
    }

    #[test]
    fn generic_and_harbor_bind_to_scored_facts() {
        let b = bind();
        let g = generic_document(&b, &serde_json::json!({"note": "ok", "n": 2}));
        validate(&g, &b, None).expect("generic");
        validate(&harbor_ok(&b), &b, Some(CONTRACT_TBENCH_HARBOR)).expect("harbor alias pin");
        validate(&harbor_ok(&b), &b, Some(CONTRACT_HARBOR_TRIALS)).expect("harbor pin");
    }

    #[test]
    fn missing_or_divergent_facts_fail_closed() {
        let b = bind();
        let missing = require_evaluate(None, &b, &BTreeMap::new()).expect_err("none");
        assert!(
            matches!(&missing, ResultsError::Io(s) if s.contains("adaptor wrote no")),
            "{missing}"
        );
        assert!(
            missing.to_string().contains(ORCH_RESULTS_ATTACH_HINT),
            "{missing}"
        );
        assert!(
            !missing.to_string().contains(PIN_RUNNER_SKEW_HINT)
                && !missing.to_string().contains("rebake"),
            "{missing}"
        );
        let mut bad = harbor_ok(&b);
        bad["primary_value"] = serde_json::json!(0.9);
        assert!(matches!(
            validate(&bad, &b, None),
            Err(ResultsError::Mismatch("primary_value"))
        ));
        let mut claim = harbor_ok(&b);
        claim["claim_holds"] = serde_json::json!(false);
        assert!(matches!(
            validate(&claim, &b, None),
            Err(ResultsError::Mismatch("claim_holds"))
        ));
        let mut mean = harbor_ok(&b);
        mean["mean_reward"] = serde_json::json!(0.25);
        assert!(matches!(
            validate(&mean, &b, None),
            Err(ResultsError::Mismatch("mean_reward"))
        ));
    }

    #[test]
    fn generic_keeps_256kib_harbor_gets_512kib_after_contract() {
        assert_eq!(MAX_RESULTS_BYTES, 256 * 1024);
        assert_eq!(MAX_HARBOR_RESULTS_BYTES, 512 * 1024);
        assert_eq!(results_size_cap(CONTRACT_GENERIC), MAX_RESULTS_BYTES);
        assert_eq!(
            results_size_cap(CONTRACT_HARBOR_TRIALS),
            MAX_HARBOR_RESULTS_BYTES
        );
        assert_eq!(
            results_size_cap(CONTRACT_TBENCH_HARBOR),
            MAX_HARBOR_RESULTS_BYTES
        );
        assert_eq!(results_size_cap("not-a-contract"), MAX_RESULTS_BYTES);
        // Adaptor source scan; coincidentally 512 KiB, not generic-custom-v1's cap.
        assert_eq!(MAX_EMIT_PROBE_BYTES, 512 * 1024);
        assert_eq!(MAX_EMIT_PROBE_BYTES, MAX_HARBOR_RESULTS_BYTES);
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("proof-results-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn write_contract_sized(dir: &Path, contract: &str, size: usize) -> PathBuf {
        let path = dir.join("results.json");
        let prefix = format!("{{\"contract\":\"{contract}\",\"pad\":\"");
        let suffix = "\"}";
        assert!(size >= prefix.len() + suffix.len());
        let mut body = String::with_capacity(size);
        body.push_str(&prefix);
        body.extend(std::iter::repeat_n('x', size - prefix.len() - suffix.len()));
        body.push_str(suffix);
        assert_eq!(body.len(), size);
        std::fs::write(&path, body).expect("write");
        path
    }

    fn with_encoded_padding(mut value: Value, min_bytes: usize) -> Value {
        let mut n = 1usize;
        loop {
            value["padding"] = Value::String("x".repeat(n));
            let encoded = serde_json::to_vec(&value).expect("encode");
            if encoded.len() >= min_bytes {
                return value;
            }
            n = n.saturating_add(min_bytes.saturating_sub(encoded.len()).saturating_add(8));
        }
    }

    #[test]
    fn load_file_generic_rejects_over_256kib_under_harbor_ceiling() {
        let dir = scratch_dir("generic-over");
        let over = usize::try_from(MAX_RESULTS_BYTES).expect("cap") + 1;
        let path = write_contract_sized(&dir, CONTRACT_GENERIC, over);
        let err = load_file(&path).expect_err("generic over 256");
        assert!(
            matches!(
                err,
                ResultsError::TooLarge {
                    cap: MAX_RESULTS_BYTES,
                    ..
                }
            ),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_file_harbor_accepts_between_256kib_and_512kib() {
        let dir = scratch_dir("harbor-mid");
        let mid = usize::try_from(MAX_RESULTS_BYTES).expect("cap") + 1;
        let path = write_contract_sized(&dir, CONTRACT_HARBOR_TRIALS, mid);
        load_file(&path).expect("harbor mid-size parses");
        let alias = scratch_dir("harbor-alias");
        let path = write_contract_sized(&alias, CONTRACT_TBENCH_HARBOR, mid);
        load_file(&path).expect("tbench-harbor alias mid-size parses");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&alias);
    }

    #[test]
    fn load_file_unknown_contract_keeps_generic_cap() {
        let dir = scratch_dir("unknown-over");
        let over = usize::try_from(MAX_RESULTS_BYTES).expect("cap") + 1;
        let path = write_contract_sized(&dir, "not-a-contract", over);
        let err = load_file(&path).expect_err("unknown is not Harbor");
        assert!(
            matches!(
                err,
                ResultsError::TooLarge {
                    cap: MAX_RESULTS_BYTES,
                    ..
                }
            ),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_file_hard_ceiling_is_harbor_max_before_parse() {
        let dir = scratch_dir("ceiling");
        let path = dir.join("results.json");
        let over = usize::try_from(MAX_HARBOR_RESULTS_BYTES).expect("cap") + 1;
        std::fs::write(&path, vec![b'x'; over]).expect("write");
        let err = load_file(&path).expect_err("over Harbor ceiling");
        assert!(
            matches!(
                err,
                ResultsError::TooLarge {
                    cap: MAX_HARBOR_RESULTS_BYTES,
                    ..
                }
            ),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_applies_family_cap_after_contract() {
        let b = bind();
        let generic_over = with_encoded_padding(
            generic_document(&b, &serde_json::json!({"ok": true})),
            usize::try_from(MAX_RESULTS_BYTES).expect("cap") + 1,
        );
        let err = validate(&generic_over, &b, None).expect_err("generic over");
        assert!(
            matches!(
                err,
                ResultsError::TooLarge {
                    cap: MAX_RESULTS_BYTES,
                    ..
                }
            ),
            "{err}"
        );
        let harbor_mid = with_encoded_padding(
            harbor_ok(&b),
            usize::try_from(MAX_RESULTS_BYTES).expect("cap") + 1,
        );
        validate(&harbor_mid, &b, None).expect("harbor mid-size still binds");
        let harbor_over = with_encoded_padding(
            harbor_ok(&b),
            usize::try_from(MAX_HARBOR_RESULTS_BYTES).expect("cap") + 1,
        );
        let err = validate(&harbor_over, &b, None).expect_err("harbor over 512");
        assert!(
            matches!(
                err,
                ResultsError::TooLarge {
                    cap: MAX_HARBOR_RESULTS_BYTES,
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn unknown_or_mismatched_contract_fails_closed() {
        let b = bind();
        let mut unknown = generic_document(&b, &serde_json::json!({"ok": true}));
        unknown["contract"] = serde_json::json!("not-a-contract");
        assert!(matches!(
            validate(&unknown, &b, None),
            Err(ResultsError::UnknownContract(_))
        ));
        let mut pin = BTreeMap::new();
        pin.insert(PARAM_RESULTS_CONTRACT.into(), "nope".into());
        assert!(matches!(
            pinned_contract(&pin),
            Err(ResultsError::UnknownContract(_))
        ));
        assert!(matches!(
            validate(
                &generic_document(&b, &serde_json::json!({"ok": true})),
                &b,
                Some(CONTRACT_HARBOR_TRIALS)
            ),
            Err(ResultsError::ContractMismatch { .. })
        ));
    }

    #[test]
    fn harbor_rejects_contradictory_trial_metadata() {
        let b = bind();
        let mut unknown = harbor_ok(&b);
        unknown["trials"][0]["outcome"] = serde_json::json!("skipped");
        assert!(
            validate(&unknown, &b, None).is_err(),
            "unknown trial.outcome must fail closed"
        );
        let mut counted = harbor_ok(&b);
        counted["n_measured"] = serde_json::json!(999);
        assert!(
            validate(&counted, &b, None).is_err(),
            "n_measured must match measured trials"
        );
        let mut exceptions = harbor_ok(&b);
        exceptions["n_agent_exceptions"] = serde_json::json!(0);
        assert!(
            validate(&exceptions, &b, None).is_err(),
            "n_agent_exceptions must match agent_exception trials"
        );
    }

    #[test]
    fn harbor_optional_trial_logs_are_typed() {
        let b = bind();
        validate(&harbor_ok(&b), &b, None).expect("optional logs allowed");
        let mut omitted = harbor_ok(&b);
        let trial0 = omitted["trials"][0].as_object_mut().expect("trial 0");
        trial0.remove("agent_log");
        trial0.remove("verifier_log");
        trial0.remove("log_sources");
        validate(&omitted, &b, None).expect("omitted logs allowed");
        let mut bad_agent = harbor_ok(&b);
        bad_agent["trials"][0]["agent_log"] = serde_json::json!(1);
        assert!(
            validate(&bad_agent, &b, None).is_err(),
            "agent_log must be a string when present"
        );
        let mut bad_verifier = harbor_ok(&b);
        bad_verifier["trials"][0]["verifier_log"] = serde_json::json!(true);
        assert!(
            validate(&bad_verifier, &b, None).is_err(),
            "verifier_log must be a string when present"
        );
        let mut bad_sources = harbor_ok(&b);
        bad_sources["trials"][0]["log_sources"] = serde_json::json!([1]);
        assert!(
            validate(&bad_sources, &b, None).is_err(),
            "log_sources must be strings when present"
        );
    }

    #[test]
    fn generic_requires_displayable_content() {
        let b = bind();
        let mut empty = generic_document(&b, &serde_json::json!({}));
        assert!(validate(&empty, &b, None).is_err());
        empty["display"] = serde_json::json!("nope");
        assert!(validate(&empty, &b, None).is_err());
    }

    #[test]
    fn results_path_is_a_safe_json_name() {
        assert_eq!(
            results_file_name(&BTreeMap::new()).expect("default"),
            RESULTS_FILE
        );
        let mut p = BTreeMap::new();
        p.insert(PARAM_RESULTS_PATH.into(), "tbench-results.json".into());
        assert_eq!(results_file_name(&p).expect("pin"), "tbench-results.json");
        p.insert(PARAM_RESULTS_PATH.into(), "audit.v1.final.json".into());
        assert_eq!(
            results_file_name(&p).expect("multi-dot"),
            "audit.v1.final.json"
        );
        p.insert(PARAM_RESULTS_PATH.into(), "audit.Json".into());
        assert_eq!(results_file_name(&p).expect("mixed-case"), "audit.Json");
        p.insert(PARAM_RESULTS_PATH.into(), " audit.json ".into());
        assert_eq!(results_file_name(&p).expect("trim"), "audit.json");
        p.insert(PARAM_RESULTS_PATH.into(), "résultats.json".into());
        assert!(matches!(
            results_file_name(&p),
            Err(ResultsError::BadPath(_))
        ));
        p.insert(PARAM_RESULTS_PATH.into(), "../x.json".into());
        assert!(matches!(
            results_file_name(&p),
            Err(ResultsError::BadPath(_))
        ));
        p.insert(PARAM_RESULTS_PATH.into(), "nope.txt".into());
        assert!(matches!(
            results_file_name(&p),
            Err(ResultsError::BadPath(_))
        ));
    }

    #[test]
    fn fixture_is_a_full_tbench_shaped_harbor_document() {
        let body = include_str!("../fixtures/harbor-trials-v1.json");
        let value = parse_results(body).expect("parse fixture");
        let digest_a = "11".repeat(32);
        let digest_b = "22".repeat(32);
        let b = ReportBind {
            topic_id: "tbench-x0032",
            custom_id: "tbench_terminal_bench",
            submission_digest: &digest_a,
            artifact_digest: &digest_b,
            primary_value: 0.4,
            claim_holds: true,
        };
        validate(&value, &b, Some(CONTRACT_TBENCH_HARBOR)).expect("fixture");
        assert_eq!(value["n_scored"], 10);
        assert_eq!(value["trials"].as_array().expect("trials").len(), 10);
        let trials = value["trials"].as_array().expect("trials");
        assert_eq!(
            trials[0]["agent_log"].as_str().expect("agent_log"),
            "hello-world agent stdout: ran ls\n"
        );
        assert_eq!(
            trials[0]["verifier_log"].as_str().expect("verifier_log"),
            "verifier: reward=1.0\n"
        );
        assert_eq!(
            trials[0]["log_sources"],
            serde_json::json!(["trial.log", "verifier/test-stdout.txt"])
        );
        assert!(
            trials[2].get("agent_log").is_none(),
            "missing Harbor files stay omitted"
        );
        assert_eq!(
            trials[3]["exception_type"].as_str().expect("exc"),
            "RuntimeError"
        );
        assert_eq!(
            trials[3]["agent_log"].as_str().expect("exc agent_log"),
            "build-tmux agent raised during harness\n"
        );
    }

    #[test]
    fn load_file_preserves_io_error_without_skew_hint() {
        let err = load_file(Path::new("/no/such/results.json")).expect_err("missing");
        let text = err.to_string();
        assert!(text.contains("adaptor wrote no results.json"), "{text}");
        assert!(
            text.contains("No such file") || text.contains("os error"),
            "{text}"
        );
        assert!(
            !text.contains(PIN_RUNNER_SKEW_HINT) && !text.contains("rebake"),
            "{text}"
        );
    }

    #[test]
    fn dangling_symlink_preserves_io_without_skew() {
        let dir =
            std::env::temp_dir().join(format!("proof-results-dangling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let link = dir.join("results.json");
        std::os::unix::fs::symlink(dir.join("gone.json"), &link).expect("symlink");
        let err = load_file(&link).expect_err("dangling");
        let text = err.to_string();
        assert!(text.contains("adaptor wrote no results.json"), "{text}");
        assert!(
            !text.contains(PIN_RUNNER_SKEW_HINT) && !text.contains("rebake"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_evaluate_report_only_does_not_name_skew() {
        let dir =
            std::env::temp_dir().join(format!("proof-results-report-only-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("report.json"), "{\"primary_value\":0.0}\n").expect("report");
        let err = load_evaluate(&dir, &BTreeMap::new(), &bind()).expect_err("missing results");
        let text = err.to_string();
        assert!(text.contains("adaptor wrote no results.json"), "{text}");
        assert!(
            !text.contains(PIN_RUNNER_SKEW_HINT) && !text.contains("rebake"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hint_skew_only_when_selected_runner_lacks_emit() {
        let dir =
            std::env::temp_dir().join(format!("proof-results-skew-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let selected = dir.join("runners").join("selected");
        let other = dir.join("runners").join("other");
        std::fs::create_dir_all(&selected).expect("selected");
        std::fs::create_dir_all(&other).expect("other");
        let err = ResultsError::Io("adaptor wrote no results.json".into());
        let hinted = hint_skew_if_runner_unemitted(err.clone(), &selected);
        assert!(
            hinted.to_string().contains(PIN_RUNNER_SKEW_HINT),
            "{hinted}"
        );
        std::fs::write(
            other.join("summarize.py"),
            "def write_results_next_to_report():\n    pass\n",
        )
        .expect("other emit");
        std::fs::write(selected.join("notes.txt"), "write_results_next_to_report\n")
            .expect("notes");
        std::fs::create_dir_all(selected.join("nested")).expect("nested");
        std::fs::write(
            selected.join("nested").join("helper.py"),
            "def write_results_next_to_report():\n    pass\n",
        )
        .expect("nested");
        let still = hint_skew_if_runner_unemitted(err.clone(), &selected);
        assert!(
            still.to_string().contains(PIN_RUNNER_SKEW_HINT),
            "sibling runner / notes / nested py must not count as emit, {still}"
        );
        assert!(!runner_tree_emits_results(&selected));
        std::fs::write(
            selected.join("summarize.py"),
            "def write_results_next_to_report():\n    pass\n",
        )
        .expect("emit");
        let clean = hint_skew_if_runner_unemitted(err, &selected);
        assert!(!clean.to_string().contains(PIN_RUNNER_SKEW_HINT), "{clean}");
        assert!(runner_tree_emits_results(&selected));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_evaluate_without_report_does_not_name_skew() {
        let dir =
            std::env::temp_dir().join(format!("proof-results-no-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let err = load_evaluate(&dir, &BTreeMap::new(), &bind()).expect_err("missing results");
        let text = err.to_string();
        assert!(text.contains("adaptor wrote no"), "{text}");
        assert!(!text.contains(PIN_RUNNER_SKEW_HINT), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
