//! Custom-metric runner contract and the fail-closed registry.
//!
//! A topic names its metric by `custom_id`. A [`CustomRunner`] registered
//! under that id knows how to inspect an artefact against the topic's rule
//! set and, behind a [`SpendToken`], run it and report a `primary_value`.
//! Nothing is registered by default: [`RunnerRegistry::resolve`] on an
//! unknown id is [`RunnerError::Unregistered`], which the host turns into a
//! 503 before any row or rent. No runner in this repository knows a
//! benchmark, a model, or a repository; those are topic data.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use proof_canon::{is_custom_id, MinerEnv};
use proof_task::{
    Constraints, InferenceOffer, MetricDirection, MetricFamily, ProofPin, TopicDocument,
};
use serde::{Deserialize, Serialize};

use crate::gate::SpendToken;
use crate::rules::{Checklist, RuleSet};

/// Only accepted `schema_version` of a run request.
pub const RUN_REQUEST_SCHEMA: u32 = 1;

/// Internal locator scheme for artefacts staged at gateway intake
/// (`proof_store::STAGED_ARTEFACT_SCHEME`). The KVM host injects vault
/// bytes over vsock; the guest must not HTTP-fetch this scheme.
pub const STAGED_ARTEFACT_SCHEME: &str = "proof-artefact";

/// Whether `uri` is a staged-vault locator (not a miner-hosted fetch).
#[must_use]
pub fn is_staged_artifact_uri(uri: &str) -> bool {
    uri.trim()
        .to_ascii_lowercase()
        .starts_with(&format!("{STAGED_ARTEFACT_SCHEME}://"))
}

/// Exact uploaded artefact bytes on the wire (standard base64). `Debug`
/// never prints the payload.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactTarB64(String);

impl ArtifactTarB64 {
    /// Encode `bytes`.
    #[must_use]
    pub fn encode(bytes: &[u8]) -> Self {
        Self(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            bytes,
        ))
    }

    /// Decode the bytes.
    ///
    /// # Errors
    ///
    /// [`RunnerError::Backend`] on bad base64.
    pub fn decode(&self) -> Result<Vec<u8>, RunnerError> {
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &self.0)
            .map_err(|e| RunnerError::Backend(format!("staged artefact: {e}")))
    }
}

impl std::fmt::Debug for ArtifactTarB64 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArtifactTarB64(<redacted>)")
    }
}

/// Only accepted `schema_version` of a run report.
pub const RUN_REPORT_SCHEMA: u32 = 1;

/// Public fields of the RLM judge offer the runner may show the harness judge.
/// Never the origin, never a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgeRef {
    /// Live judge offer id.
    pub offer_id: String,
    /// Judge model id.
    pub model_ref: String,
    /// Judge config commitment (config knobs + origin, hashed).
    pub config_commitment: String,
}

/// Sandbox policy the runner and the topic VM must honour for miner code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Miner code runs only inside a Firecracker guest under the topic VM.
    pub firecracker_required: bool,
    /// Wall-clock deadline for one proof run (topic `eval_executor`, else pin).
    pub deadline_s: u64,
}

/// Everything a runner needs to inspect and run one submission once.
///
/// Every value is copied from the signed topic, the live judge offer, the
/// rule set, and the submission. Nothing is a default of this crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomRunRequest {
    /// Must equal [`RUN_REQUEST_SCHEMA`].
    pub schema_version: u32,
    /// Topic id.
    pub topic_id: String,
    /// Custom metric id the topic names.
    pub custom_id: String,
    /// Primary metric name the report must fill.
    pub primary: String,
    /// Improvement direction.
    pub direction: MetricDirection,
    /// Relative win the topic demands over the bar.
    pub epsilon_rel: f64,
    /// Frozen submission digest.
    pub submission_digest: String,
    /// Artefact digest: sha256 of the exact file served at `artifact_uri`
    /// (an uncompressed tar of the recipe tree).
    pub artifact_digest: String,
    /// Miner-supplied locator for the same bytes. The runner fetches from it
    /// inside the topic VM, verifies the bytes as received against
    /// `artifact_digest`, and forwards them **verbatim** to the sister (never
    /// a re-tar of the tree); it is never trusted beyond that. Empty /
    /// whitespace is `None`. A `proof-artefact://{digest}` locator means
    /// the miner uploaded bytes: the host injects [`Self::artifact_tar`]
    /// over vsock and the guest must not HTTP-fetch this scheme.
    pub artifact_uri: Option<String>,
    /// Exact uploaded artefact bytes when the miner posted them (gateway
    /// vault). Present only for `proof-artefact://` locators. The KVM host
    /// injects these over vsock before `Run`; URI-only submits leave this
    /// `None` and the guest still GETs `https://` (64 MiB cap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_tar: Option<ArtifactTarB64>,
    /// Miner claim (English).
    pub claim: String,
    /// FLOPs one run may spend (the topic's signed `flops_budget`). Carried
    /// for harvest-family provenance; custom / agent topics do not gate on it.
    pub flops_budget: u64,
    /// FLOPs the miner declared for this run. Optional unused field on
    /// custom / agent topics (signature compat); harvest intake may still
    /// refuse a declaration over the topic budget.
    pub declared_flops: u64,
    /// Topic constraints (sandbox flag, model pin, opaque slice / params).
    pub constraints: Constraints,
    /// Miner BYOK environment: the variables this topic's signed document
    /// declares (`miner_byok` / `miner_env_allowlist`) carrying the values
    /// the miner posted with the submission. Empty unless the topic declares
    /// one — and always empty for a baseline, which is the operator's run.
    ///
    /// This is the only secret a run request carries, it belongs to the
    /// miner, and it travels one way: control plane → the guest that runs
    /// this submission, which exports it to the miner's own process. Owner
    /// key material never travels here — the KVM host stages that from its
    /// own disk and never into a miner guest.
    #[serde(default, skip_serializing_if = "MinerEnv::is_empty")]
    pub miner_env: MinerEnv,
    /// Rule version the checklist must tick.
    pub rules_version: u32,
    /// Digest of that rule set.
    pub rules_digest: String,
    /// Seed every paid call must use (topic baseline seed).
    pub seed: u64,
    /// Public judge reference.
    pub judge: JudgeRef,
    /// Sandbox policy.
    pub sandbox: SandboxPolicy,
    /// Executor offer commitment the topic pins, if any.
    pub executor_commitment: Option<String>,
}

/// Why a run did not happen or is not evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunnerError {
    /// No runner registered under this custom id. Root cause for the 503.
    /// `twin` names a registered id that differs only by `_` ↔ `-` / case
    /// (the operator mix-up between a topic slug and a custom id), if any.
    #[error("custom metric {custom_id:?} has no registered runner{tail}", tail = proof_canon::twin_suffix(twin.as_deref()))]
    Unregistered {
        /// The id the topic named.
        custom_id: String,
        /// A registered id that is this one up to `_` ↔ `-` and case.
        twin: Option<String>,
    },
    /// A runner exists but its backend (topic VM orchestrator) is not configured.
    #[error("runner not wired: {0}")]
    NotWired(String),
    /// The topic is not a custom-family topic.
    #[error("topic {0:?} is not a custom-family topic")]
    NotCustom(String),
    /// Registry key is not a well-formed custom id.
    #[error("custom id {0:?} must match [a-z0-9][a-z0-9_-]{{1,63}}{hint}", hint = proof_canon::custom_id_suffix(.0))]
    BadCustomId(String),
    /// The token covers another submission.
    #[error("spend token does not cover this run")]
    SpendTokenMismatch,
    /// The backend failed.
    #[error("runner backend: {0}")]
    Backend(String),
    /// The runner returned a document that is not evidence.
    #[error("run report: {0}")]
    Report(#[from] ReportError),
}

impl CustomRunRequest {
    /// Build the request from a custom-family topic, the live judge offer,
    /// and the current rule set.
    ///
    /// # Errors
    ///
    /// [`RunnerError::NotCustom`] for `nll` / `throughput` topics.
    #[allow(clippy::too_many_arguments)]
    pub fn from_topic(
        topic: &TopicDocument,
        pin: &ProofPin,
        offer: &InferenceOffer,
        rules: &RuleSet,
        submission_digest: &str,
        artifact_digest: &str,
        artifact_uri: Option<&str>,
        declared_flops: u64,
        claim: &str,
    ) -> Result<Self, RunnerError> {
        if topic.metric.family != MetricFamily::Custom {
            return Err(RunnerError::NotCustom(topic.id.clone()));
        }
        Ok(Self {
            schema_version: RUN_REQUEST_SCHEMA,
            topic_id: topic.id.clone(),
            custom_id: topic.metric.custom_id.trim().to_owned(),
            primary: topic.metric.primary.trim().to_owned(),
            direction: topic.metric.direction,
            epsilon_rel: topic.metric.epsilon_rel,
            submission_digest: submission_digest.trim().to_owned(),
            artifact_digest: artifact_digest.trim().to_ascii_lowercase(),
            artifact_uri: artifact_uri
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_owned),
            artifact_tar: None,
            claim: claim.to_owned(),
            flops_budget: topic.flops_budget,
            declared_flops,
            constraints: topic.constraints.clone(),
            miner_env: MinerEnv::new(),
            rules_version: rules.version,
            rules_digest: rules.digest(),
            seed: topic.baseline.seed,
            judge: JudgeRef {
                offer_id: offer.offer_id.clone(),
                model_ref: offer.config.model_ref.clone(),
                config_commitment: offer.config_commitment.clone(),
            },
            sandbox: SandboxPolicy {
                firecracker_required: topic.constraints.firecracker_required,
                deadline_s: topic
                    .eval_executor
                    .max_proof_deadline_s
                    .unwrap_or(pin.max_proof_deadline_s_ceiling),
            },
            executor_commitment: topic.eval_executor.require_offer_commitment.clone(),
        })
    }

    /// Bind the resolved executor plan the host is running under: the run's
    /// deadline is the tighter of the topic's and the plan's, and the
    /// commitment of the configuration that actually runs replaces the
    /// topic's pin as provenance.
    #[must_use]
    pub fn with_executor_plan(mut self, deadline_s: u64, config_commitment: &str) -> Self {
        if deadline_s > 0 {
            self.sandbox.deadline_s = self.sandbox.deadline_s.min(deadline_s);
        }
        let c = config_commitment.trim();
        if !c.is_empty() {
            self.executor_commitment = Some(c.to_owned());
        }
        self
    }

    /// Facts a results JSON must echo for this request (evaluate bind).
    #[must_use]
    pub fn results_bind(
        &self,
        primary_value: f64,
        claim_holds: bool,
    ) -> proof_results::ReportBind<'_> {
        proof_results::ReportBind::new(
            &self.topic_id,
            &self.custom_id,
            &self.submission_digest,
            &self.artifact_digest,
            primary_value,
            claim_holds,
        )
    }

    /// Bind the miner's own BYOK environment to this run.
    ///
    /// The caller has already held `env` to the signed topic's allowlist
    /// ([`proof_canon::MinerEnv::accept`]); this only carries it. A baseline
    /// keeps the default (empty): the operator's reference run is never paid
    /// for with a miner's key.
    #[must_use]
    pub fn with_miner_env(mut self, env: MinerEnv) -> Self {
        self.miner_env = env;
        self
    }

    /// Bind uploaded artefact bytes (gateway vault) for vsock inject.
    ///
    /// Empty input is ignored — never invent bytes. The host still verifies
    /// the tar against `artifact_digest` before any guest sees it.
    #[must_use]
    pub fn with_artifact_tar(mut self, bytes: &[u8]) -> Self {
        if !bytes.is_empty() {
            self.artifact_tar = Some(ArtifactTarB64::encode(bytes));
        }
        self
    }

    /// Drop vault bytes so the `Run` frame does not duplicate an inject.
    #[must_use]
    pub fn without_artifact_tar(mut self) -> Self {
        self.artifact_tar = None;
        self
    }

    /// Whether the locator is a staged-vault scheme the guest must not fetch.
    #[must_use]
    pub fn is_staged_locator(&self) -> bool {
        self.artifact_uri
            .as_deref()
            .is_some_and(is_staged_artifact_uri)
    }

    /// Decoded vault bytes, if this request carries them.
    ///
    /// # Errors
    ///
    /// [`RunnerError::Backend`] on bad base64.
    pub fn artifact_tar_bytes(&self) -> Result<Option<Vec<u8>>, RunnerError> {
        self.artifact_tar
            .as_ref()
            .map(ArtifactTarB64::decode)
            .transpose()
    }

    /// Whether the signed topic also forwards the miner env into the sister
    /// guest that runs the miner's entrypoint
    /// ([`proof_canon::PARAM_INJECT_MINER_ENV_SISTER`]).
    #[must_use]
    pub fn miner_env_in_sister(&self) -> bool {
        self.constraints.inject_miner_env_sister()
    }
}

/// Runner-authored measurement for one submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomRunReport {
    /// Must equal [`RUN_REPORT_SCHEMA`].
    pub schema_version: u32,
    /// Topic run for.
    pub topic_id: String,
    /// Custom metric id.
    pub custom_id: String,
    /// Frozen submission digest.
    pub submission_digest: String,
    /// Artefact digest that was run.
    pub artifact_digest: String,
    /// Rule version the run was gated by.
    pub rules_version: u32,
    /// The primary metric value (becomes `custom_value`).
    pub primary_value: f64,
    /// Whether the miner's claim matches the measured numbers (runner/judge-filled).
    pub claim_holds: bool,
    /// Whether miner code ran inside the Firecracker guest.
    pub sandboxed: bool,
    /// FLOPs the run consumed, as measured by the runner when it has a
    /// counter. Custom / agent topics treat this as telemetry: a missing
    /// figure is not a bind failure and is not a reject.
    #[serde(default)]
    pub flops_used: Option<u64>,
    /// Opaque runner evidence (per-task rows, timings). Shipped in the artefact.
    pub evidence: BTreeMap<String, serde_json::Value>,
    /// Topic-defined complete results JSON (display/audit). Required on a
    /// successful evaluate; absent on baseline / pre-spend reject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results: Option<serde_json::Value>,
}

/// Why a report is not evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReportError {
    /// JSON did not parse.
    #[error("parse run report: {0}")]
    Parse(String),
    /// Schema drift.
    #[error("run report schema_version {got}, this build reads {RUN_REPORT_SCHEMA}")]
    WrongSchema {
        /// What the document said.
        got: u32,
    },
    /// A binding field does not echo the request.
    #[error("run report {0} does not match the run request")]
    Mismatch(&'static str),
    /// Primary is NaN / infinite.
    #[error("run report primary_value is not finite")]
    NotFinite,
    /// The topic requires Firecracker and the run was not sandboxed.
    #[error("run report says miner code ran outside the Firecracker guest")]
    NotSandboxed,
}

impl CustomRunReport {
    /// Parse `report.json`.
    ///
    /// # Errors
    ///
    /// [`ReportError::Parse`].
    pub fn from_json(body: &str) -> Result<Self, ReportError> {
        serde_json::from_str(body).map_err(|e| ReportError::Parse(e.to_string()))
    }

    /// Pretty JSON for the artefact bundle.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Bind the report to the request that produced it.
    ///
    /// # Errors
    ///
    /// See [`ReportError`]. A report that fails here never becomes a `custom_value`.
    pub fn verify(&self, req: &CustomRunRequest) -> Result<(), ReportError> {
        if self.schema_version != RUN_REPORT_SCHEMA {
            return Err(ReportError::WrongSchema {
                got: self.schema_version,
            });
        }
        let pairs: [(&'static str, bool); 5] = [
            ("topic_id", self.topic_id.trim() == req.topic_id.trim()),
            ("custom_id", self.custom_id.trim() == req.custom_id.trim()),
            (
                "submission_digest",
                self.submission_digest.trim() == req.submission_digest.trim(),
            ),
            (
                "artifact_digest",
                self.artifact_digest
                    .trim()
                    .eq_ignore_ascii_case(req.artifact_digest.trim()),
            ),
            ("rules_version", self.rules_version == req.rules_version),
        ];
        if let Some((field, _)) = pairs.iter().find(|(_, ok)| !ok) {
            return Err(ReportError::Mismatch(field));
        }
        if !self.primary_value.is_finite() {
            return Err(ReportError::NotFinite);
        }
        if req.sandbox.firecracker_required && !self.sandboxed {
            return Err(ReportError::NotSandboxed);
        }
        Ok(())
    }

    /// Runner-measured FLOPs this run consumed, if any. Missing is 0 —
    /// custom / agent topics do not require a measurement.
    #[must_use]
    pub fn flops_used_for(&self) -> u64 {
        self.flops_used.unwrap_or(0)
    }
}

/// One file of the miner's artefact tree as inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactFile {
    /// Relative path inside the artefact (`src/main.rs`).
    pub path: String,
    /// Bytes.
    pub bytes: Vec<u8>,
}

/// A log the runner captured (harness stdout, guest console, judge transcript).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFile {
    /// File name under `logs/` in the artefact (single path segment).
    pub name: String,
    /// Raw bytes.
    pub bytes: Vec<u8>,
}

/// What inspection produced: the ticked checklist and the tree it looked at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectOutcome {
    /// One item per rule of the requested version.
    pub checklist: Checklist,
    /// The artefact tree as inspected (shipped in the artefact zip).
    pub artifact: Vec<ArtifactFile>,
}

/// What a paid run produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    /// Runner-authored measurement.
    pub report: CustomRunReport,
    /// Captured logs.
    pub logs: Vec<LogFile>,
}

/// Inspects and runs submissions for one custom metric id. Paid work needs a token.
#[async_trait]
pub trait CustomRunner: Send + Sync {
    /// Whether this runner could run right now (fail-closed).
    ///
    /// # Errors
    ///
    /// [`RunnerError::NotWired`] when its backend is not configured.
    fn ready(&self) -> Result<(), RunnerError>;

    /// Tick every rule of `rules` over the artefact. **No paid inference.**
    async fn inspect(
        &self,
        req: &CustomRunRequest,
        rules: &RuleSet,
    ) -> Result<InspectOutcome, RunnerError>;

    /// Run the artefact. `spend` must cover the request.
    async fn evaluate(
        &self,
        req: &CustomRunRequest,
        spend: &SpendToken,
    ) -> Result<RunOutcome, RunnerError>;
}

/// `custom_id → runner`. Empty by default; unknown ids fail closed.
#[derive(Default)]
pub struct RunnerRegistry {
    runners: BTreeMap<String, Arc<dyn CustomRunner>>,
}

impl RunnerRegistry {
    /// An empty registry: every custom topic is unscorable until something registers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `runner` under `custom_id` (replaces an earlier registration).
    ///
    /// # Errors
    ///
    /// [`RunnerError::BadCustomId`].
    pub fn register(
        &mut self,
        custom_id: &str,
        runner: Arc<dyn CustomRunner>,
    ) -> Result<(), RunnerError> {
        let id = custom_id.trim();
        if !is_custom_id(id) {
            return Err(RunnerError::BadCustomId(id.to_owned()));
        }
        self.runners.insert(id.to_owned(), runner);
        Ok(())
    }

    /// Builder form of [`Self::register`]; a bad id is dropped with the error
    /// surfaced through [`Self::ids`] being unchanged.
    #[must_use]
    pub fn with(mut self, custom_id: &str, runner: Arc<dyn CustomRunner>) -> Self {
        let _ = self.register(custom_id, runner);
        self
    }

    /// The runner for `custom_id`.
    ///
    /// # Errors
    ///
    /// [`RunnerError::Unregistered`] — the fail-closed default.
    pub fn resolve(&self, custom_id: &str) -> Result<Arc<dyn CustomRunner>, RunnerError> {
        let id = custom_id.trim();
        self.runners.get(id).cloned().ok_or_else(|| {
            // Exact match only — but say so when the operator registered the
            // hyphen / underscore twin (topic slug typed for a custom id).
            let twin = proof_canon::id_twin(id, self.runners.keys().map(String::as_str))
                .map(str::to_owned);
            RunnerError::Unregistered {
                custom_id: id.to_owned(),
                twin,
            }
        })
    }

    /// Registered ids, sorted.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.runners.keys().cloned().collect()
    }

    /// Whether anything is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.runners.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{offer, pin, report_for, request, rules, topic, FakeOrchestrator};
    use crate::vm::{VmBackedRunner, VmTemplate};

    #[test]
    fn the_request_copies_topic_offer_and_rules_and_never_a_secret() {
        let req = request();
        let t = topic();
        assert_eq!(req.custom_id, t.metric.custom_id);
        assert_eq!(req.constraints, t.constraints);
        assert_eq!(req.rules_version, 1);
        assert_eq!(req.rules_digest, rules().digest());
        assert_eq!(req.seed, t.baseline.seed);
        assert_eq!(
            req.flops_budget, t.flops_budget,
            "budget travels to the runner"
        );
        assert_eq!(req.declared_flops, 1, "the miner's declaration travels too");
        assert_eq!(
            req.artifact_uri.as_deref(),
            Some("https://example.invalid/artifact.zip"),
            "the miner locator reaches the runner"
        );
        assert!(req.sandbox.firecracker_required);
        assert_eq!(req.sandbox.deadline_s, pin().max_proof_deadline_s_ceiling);
        assert_eq!(req.judge.offer_id, offer().offer_id);
        let dump = serde_json::to_string(&req).expect("json");
        assert!(
            !dump.contains("127.0.0.1"),
            "judge origin must not travel: {dump}"
        );
        assert!(!dump.contains("base_url"), "{dump}");
        assert!(!dump.contains("api_key"), "{dump}");
        let mut plain = t;
        plain.metric.family = MetricFamily::Nll;
        assert!(matches!(
            CustomRunRequest::from_topic(&plain, &pin(), &offer(), &rules(), "d", "a", None, 1, ""),
            Err(RunnerError::NotCustom(_))
        ));
        let bound = request().with_executor_plan(900, "ab".repeat(32).as_str());
        assert_eq!(bound.sandbox.deadline_s, 900, "plan tightens the deadline");
        assert_eq!(
            bound.executor_commitment.as_deref(),
            Some("ab".repeat(32).as_str())
        );
        let looser = request().with_executor_plan(u64::MAX, "");
        assert_eq!(looser.sandbox.deadline_s, request().sandbox.deadline_s);
        assert_eq!(looser.executor_commitment, request().executor_commitment);
        let mut tight = topic();
        tight.eval_executor.max_proof_deadline_s = Some(600);
        let req =
            CustomRunRequest::from_topic(&tight, &pin(), &offer(), &rules(), "d", "a", None, 1, "")
                .expect("request");
        assert_eq!(req.sandbox.deadline_s, 600);
        let blank = CustomRunRequest::from_topic(
            &topic(),
            &pin(),
            &offer(),
            &rules(),
            "d",
            "a",
            Some("  "),
            1,
            "",
        )
        .expect("request");
        assert_eq!(blank.artifact_uri, None, "whitespace is no locator");
        assert!(blank.artifact_tar.is_none());
        let staged = request().with_artifact_tar(b"recipe-tar-bytes");
        assert!(staged.artifact_tar.is_some());
        assert_eq!(
            staged.artifact_tar_bytes().expect("b64"),
            Some(b"recipe-tar-bytes".to_vec())
        );
        assert!(
            format!("{staged:?}").contains("ArtifactTarB64(<redacted>)"),
            "vault bytes never debug-print"
        );
        assert!(!format!("{staged:?}").contains("recipe-tar-bytes"));
        let uri = format!("{STAGED_ARTEFACT_SCHEME}://{}", "ab".repeat(32));
        assert!(is_staged_artifact_uri(&uri));
        assert!(!is_staged_artifact_uri("https://example.invalid/a.tar"));
        let mut loc = request();
        loc.artifact_uri = Some(uri);
        assert!(loc.is_staged_locator());
        assert!(!request().is_staged_locator());
    }

    /// Custom / agent reports do not require a FLOP measurement and do not
    /// fail to bind when usage is missing or over the signed budget.
    #[test]
    fn a_report_binds_without_flop_accounting() {
        let req = request();
        assert!(req.flops_budget > 0, "fixture topic carries a budget");
        let measured = report_for(&req, 0.7);
        assert_eq!(measured.flops_used_for(), 1);
        let mut none = report_for(&req, 0.7);
        none.flops_used = None;
        assert_eq!(none.flops_used_for(), 0);
        none.verify(&req)
            .expect("missing flops_used is not a bind failure");
        let mut over = report_for(&req, 0.7);
        over.flops_used = Some(req.flops_budget + 1);
        over.verify(&req)
            .expect("over budget is telemetry, not a bind failure");
        assert_eq!(over.flops_used_for(), req.flops_budget + 1);
        let legacy: CustomRunReport =
            serde_json::from_str(&none.to_json()).expect("a report without the field parses");
        assert_eq!(legacy.flops_used, None);
    }

    #[test]
    fn a_report_must_echo_the_request_and_honour_the_sandbox() {
        let req = request();
        let r = report_for(&req, 0.7);
        r.verify(&req).expect("bound");
        let round = CustomRunReport::from_json(&r.to_json()).expect("round trip");
        assert_eq!(round, r);
        let mut other = report_for(&req, 0.7);
        other.custom_id = "other_metric".into();
        assert_eq!(other.verify(&req), Err(ReportError::Mismatch("custom_id")));
        let mut stale = report_for(&req, 0.7);
        stale.rules_version += 1;
        assert_eq!(
            stale.verify(&req),
            Err(ReportError::Mismatch("rules_version"))
        );
        let mut nan = report_for(&req, f64::NAN);
        nan.primary_value = f64::NAN;
        assert_eq!(nan.verify(&req), Err(ReportError::NotFinite));
        let mut host = report_for(&req, 0.7);
        host.sandboxed = false;
        assert_eq!(host.verify(&req), Err(ReportError::NotSandboxed));
        let mut relaxed = req.clone();
        relaxed.sandbox.firecracker_required = false;
        host.verify(&relaxed)
            .expect("topic did not require the guest");
        let mut schema = report_for(&req, 0.7);
        schema.schema_version = 9;
        assert_eq!(
            schema.verify(&req),
            Err(ReportError::WrongSchema { got: 9 })
        );
        assert!(CustomRunReport::from_json("[]").is_err());
    }

    /// The registry is empty by default and resolves nothing: the
    /// fail-closed contract for every custom id nobody registered.
    #[test]
    fn the_registry_is_empty_by_default_and_unknown_ids_fail_closed() {
        let reg = RunnerRegistry::new();
        assert!(reg.is_empty());
        assert!(reg.ids().is_empty());
        for id in ["any_metric", "another_metric", "yet_another_metric"] {
            assert_eq!(
                reg.resolve(id).err(),
                Some(RunnerError::Unregistered {
                    custom_id: id.into(),
                    twin: None,
                })
            );
        }
        let runner: Arc<dyn CustomRunner> = Arc::new(VmBackedRunner::new(
            FakeOrchestrator::new(0.5),
            VmTemplate::unpinned(),
        ));
        let mut reg = RunnerRegistry::new();
        assert_eq!(
            reg.register("Bad Id", runner.clone()),
            Err(RunnerError::BadCustomId("Bad Id".into()))
        );
        let msg = RunnerError::BadCustomId("Bad Id".into()).to_string();
        assert!(msg.contains("did you mean \"bad_id\"?"), "{msg}");
        reg.register("topic_minted_metric", runner.clone())
            .expect("register");
        assert_eq!(reg.ids(), vec!["topic_minted_metric".to_owned()]);
        reg.resolve("topic_minted_metric").expect("resolved");
        assert!(reg.resolve("other").is_err());
        // The hyphenated twin of a registered id is NOT a match — ids are
        // byte-for-byte — but the error names the id that is registered.
        let err = reg
            .resolve("topic-minted-metric")
            .err()
            .expect("twin is not a match");
        assert_eq!(
            err,
            RunnerError::Unregistered {
                custom_id: "topic-minted-metric".into(),
                twin: Some("topic_minted_metric".into()),
            }
        );
        let msg = err.to_string();
        assert!(
            msg.contains("has no registered runner")
                && msg.contains("\"topic_minted_metric\" is registered")
                && msg.contains("PROOF_VM_RUNNER_CUSTOM_IDS"),
            "{msg}"
        );
        let built = RunnerRegistry::new().with("also_minted", runner);
        assert_eq!(built.ids(), vec!["also_minted".to_owned()]);
    }
}
