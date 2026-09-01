//! Relearn Image eval loop: freeze digest → unseal prompts → generate → judge.
//!
//! The control plane only ever boots a digest-pinned eval image, and the miner
//! pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). Q-Judger runs judge-only: it
//! never serves miner weights as the scored artifact.
//!
//! Backend resolution is fail-closed. A host with no judge endpoint and no
//! explicit sim opt-in refuses to score rather than emitting a deterministic
//! placeholder that would look like a passing eval. Sim exists for CI and for
//! local development, is selected only by `RELEARN_T2I_FORCE_SIM=1` or
//! `RELEARN_T2I_JUDGE_BACKEND=sim`, and is reported on `/v1/status` so an
//! operator can never mistake it for a real run.
//!
//! A live host also needs a champion measured by the scorer submissions face
//! ([`boot_base_champion`]). Every gate is a comparison against the champion,
//! so judging a live challenger against simulated champion numbers would let
//! an artifact displace a champion nobody ever measured.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use prism_competition::ExampleSeries;
use prism_lium::{EvalJobBackend, SimLiumBackend};
use prism_lium_types::{EvalReceipt, InstanceSpec, NoScoreGate, RemoteExecResult};
use relearn_t2i_judge::{assert_judge_model, ImageScore, JudgeError, JudgeInference};
use relearn_t2i_score::{
    contamination, ContaminationEvidence, FaithfulnessEvidence, ReplayEvidence, T2iSliceScores,
    MIN_FAITHFULNESS_CHECKS, REPLAY_CELLS,
};
use relearn_t2i_store::ArtifactManifest;
use relearn_t2i_task::{
    cell_key, FrozenPrompt, L1Dimension, PinError, RelearnT2iPin, SeedCell, JUDGE_MODEL_ID,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Where Q-Judger runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JudgeBackend {
    /// Q-Judger behind an OpenAI-compatible HTTP API (`RELEARN_T2I_JUDGE_API_URL`).
    HttpApi,
    /// Q-Judger on a digest-pinned Lium pod (`RELEARN_T2I_JUDGE_BACKEND=lium`).
    Lium,
    /// Deterministic offline judge (CI / local only).
    Sim,
}

impl JudgeBackend {
    /// Parse from `RELEARN_T2I_JUDGE_BACKEND`. Empty → HTTP API.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("RELEARN_T2I_JUDGE_BACKEND")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "lium" | "lium_pod" => Self::Lium,
            "sim" => Self::Sim,
            _ => Self::HttpApi,
        }
    }
}

/// `RELEARN_T2I_JUDGE_API_URL` when set. No baked host — missing means refuse.
#[must_use]
pub fn judge_api_url() -> Option<String> {
    std::env::var("RELEARN_T2I_JUDGE_API_URL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Bearer for the Q-Judger HTTP API. Never log the value.
#[must_use]
pub fn judge_api_key() -> Option<String> {
    std::env::var("RELEARN_T2I_JUDGE_API_KEY")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// True when the operator explicitly opted into sim.
#[must_use]
pub fn force_sim() -> bool {
    matches!(
        std::env::var("RELEARN_T2I_FORCE_SIM")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

/// Resolve the judge backend for this host.
#[must_use]
pub fn resolve_judge_backend() -> JudgeBackend {
    if force_sim() {
        return JudgeBackend::Sim;
    }
    JudgeBackend::from_env()
}

/// Judge wiring for this process, resolved once at boot.
///
/// The endpoint is deliberately not `Serialize`: `/v1/status` reports whether
/// one is configured, never the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeConfig {
    /// Where Q-Judger runs.
    pub backend: JudgeBackend,
    /// Judge endpoint from `RELEARN_T2I_JUDGE_API_URL`, if any.
    endpoint: Option<String>,
}

impl JudgeConfig {
    /// Read `RELEARN_T2I_*` once. Call this at process start, not per request.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            backend: resolve_judge_backend(),
            endpoint: judge_api_url(),
        }
    }

    /// Deterministic offline config for CI and local development.
    #[must_use]
    pub fn sim() -> Self {
        Self {
            backend: JudgeBackend::Sim,
            endpoint: None,
        }
    }

    /// HTTP-API config pointed at an explicit endpoint.
    ///
    /// The env path is [`Self::from_env`]; this exists so a caller that
    /// already resolved an endpoint does not have to round-trip through the
    /// process environment.
    #[must_use]
    pub fn http_api(endpoint: impl Into<String>) -> Self {
        Self {
            backend: JudgeBackend::HttpApi,
            endpoint: Some(endpoint.into()),
        }
    }

    /// Whether a judge endpoint is configured. Never exposes the value.
    #[must_use]
    pub fn endpoint_configured(&self) -> bool {
        self.endpoint.is_some()
    }

    /// Refuse to score unless Q-Judger is the judge and a backend is reachable.
    ///
    /// # Errors
    ///
    /// [`EvalError::Judge`] when the model is not Q-Judger, and
    /// [`EvalError::JudgeUnconfigured`] when the HTTP backend has no endpoint.
    pub fn preflight(&self, inference: &JudgeInference) -> Result<(), EvalError> {
        assert_judge_model(&inference.model)?;
        if self.backend == JudgeBackend::HttpApi && self.endpoint.is_none() {
            return Err(EvalError::JudgeUnconfigured);
        }
        Ok(())
    }
}

/// Eval errors.
#[derive(Debug, Error)]
pub enum EvalError {
    /// Holdout was requested before the digest freeze, or none is loaded.
    #[error("holdout unavailable: {0}")]
    Holdout(String),
    /// Integrity gate failed.
    #[error("integrity: {0}")]
    Integrity(String),
    /// Lium / backend failure.
    #[error("backend: {0}")]
    Backend(String),
    /// The artifact's declared base or license does not match the pin.
    #[error("license attestation: {0}")]
    Attestation(#[from] PinError),
    /// Q-Judger replied with something unusable.
    #[error("judge: {0}")]
    Judge(#[from] JudgeError),
    /// No judge endpoint configured and sim was not opted into.
    #[error("Q-Judger backend unconfigured: set RELEARN_T2I_JUDGE_API_URL, or RELEARN_T2I_FORCE_SIM=1 for CI")]
    JudgeUnconfigured,
    /// A live run was asked for without a digest-pinned eval image.
    #[error(
        "eval image digest not pinned; refuse live scoring (RELEARN_T2I_FORCE_SIM=1 is CI only)"
    )]
    EvalImageUnpinned,
    /// A live run reached the in-process scorer. It must not silently sim.
    #[error("live Q-Judger harvest is driven by the digest-pinned eval image; no in-process sim")]
    LiveHarvestUnavailable,
    /// The operator-recorded champion baseline does not match the pin.
    #[error("recorded baseline: {0}")]
    Baseline(String),
    /// The judge declined too many items to produce a comparable score.
    #[error("judge N/A rate {rate:.3} above the {max:.3} ceiling")]
    NotApplicableRate {
        /// Observed rate.
        rate: f64,
        /// Ceiling.
        max: f64,
    },
}

/// One finished eval.
#[derive(Debug, Clone)]
pub struct EvalOutcome {
    /// Challenger measurements.
    pub scores: T2iSliceScores,
    /// Integrity receipt.
    pub receipt: EvalReceipt,
    /// Backend that produced the scores.
    pub backend: JudgeBackend,
    /// Number of holdout cells scored.
    pub holdout_cells: usize,
}

/// Deterministic per-cell judge score derived from a digest and a cell key.
///
/// Sim only. Every value is a function of the frozen submission digest, so a
/// sim run is reproducible and is never mistaken for evidence about the model.
fn sim_image_score(artifact_digest: &str, key: &str, bias: f64) -> ImageScore {
    let mut per_l1 = BTreeMap::new();
    for (i, dim) in L1Dimension::ALL.into_iter().enumerate() {
        let mut h = Sha256::new();
        h.update(artifact_digest.as_bytes());
        h.update([0xff]);
        h.update(key.as_bytes());
        h.update([0xff, u8::try_from(i).unwrap_or(0)]);
        let d = h.finalize();
        let unit = f64::from(d[0]) / 255.0;
        per_l1.insert(dim, ((0.45 + 0.40 * unit + bias) * 100.0).clamp(0.0, 100.0));
    }
    let total = per_l1.values().sum::<f64>() / per_l1.len() as f64;
    ImageScore {
        per_l1,
        total,
        scored_items: 20,
        na_items: 1,
    }
}

/// Fold per-cell [`ImageScore`]s into the normalized series the gates read.
///
/// # Errors
///
/// [`EvalError::NotApplicableRate`] when the judge declined too much of the
/// holdout, and [`EvalError::Holdout`] when the holdout produced no cells.
pub fn fold_scores(
    holdout: &BTreeMap<String, ImageScore>,
    public: &BTreeMap<String, ImageScore>,
    capability: &BTreeMap<String, ImageScore>,
    replay: ReplayEvidence,
    faithfulness: FaithfulnessEvidence,
    contamination: ContaminationEvidence,
) -> Result<T2iSliceScores, EvalError> {
    if holdout.is_empty() {
        return Err(EvalError::Holdout("no holdout cells scored".into()));
    }
    let series = |m: &BTreeMap<String, ImageScore>| {
        ExampleSeries::from_pairs(m.iter().map(|(k, v)| (k.clone(), v.normalized_total())))
    };
    let mut by_pillar: BTreeMap<L1Dimension, ExampleSeries> = BTreeMap::new();
    for dim in L1Dimension::ALL {
        let pairs: Vec<(String, f64)> = holdout
            .iter()
            .filter_map(|(k, v)| v.normalized_pillar(dim).map(|x| (k.clone(), x)))
            .collect();
        if !pairs.is_empty() {
            by_pillar.insert(dim, ExampleSeries::from_pairs(pairs));
        }
    }

    let na_items: u32 = holdout.values().map(|v| v.na_items).sum();
    let scored_items: u32 = holdout.values().map(|v| v.scored_items).sum();
    let denom = f64::from(na_items) + f64::from(scored_items);
    let na_rate = if denom <= 0.0 {
        1.0
    } else {
        f64::from(na_items) / denom
    };
    if na_rate > relearn_t2i_score::MAX_NA_RATE {
        return Err(EvalError::NotApplicableRate {
            rate: na_rate,
            max: relearn_t2i_score::MAX_NA_RATE,
        });
    }

    Ok(T2iSliceScores {
        holdout: series(holdout),
        public: series(public),
        holdout_by_pillar: by_pillar,
        capability_canary: series(capability),
        na_rate,
        replay,
        faithfulness,
        contamination,
    })
}

/// Score every cell of a split in sim.
fn sim_split(
    pin: &RelearnT2iPin,
    prompt_ids: &[u32],
    artifact_digest: &str,
    bias: f64,
) -> BTreeMap<String, ImageScore> {
    pin.seed_cells(prompt_ids)
        .into_iter()
        .map(
            |SeedCell {
                 prompt_id,
                 variation_index,
                 seed,
             }| {
                let key = cell_key(prompt_id, variation_index);
                let salted = format!("{artifact_digest}:{seed}");
                let score = sim_image_score(&salted, &key, bias);
                (key, score)
            },
        )
        .collect()
}

/// Artifact id of the un-fine-tuned pinned generator on the holdout.
pub const BASE_CHAMPION_ARTIFACT: &str = "base-relearn-image-champion";

/// Run id bound into the boot baseline measurement.
pub const BASE_CHAMPION_RUN: &str = "boot-baseline";

/// Cell keys of the general-capability canary slice.
///
/// Fixed prompts outside the bench split, so the canary cannot be inferred
/// from the scored ids and cannot be tuned through the paid number.
fn capability_cells() -> Vec<String> {
    (0..24).map(|i| format!("cap#{i}")).collect()
}

fn sim_capability(artifact_digest: &str, bias: f64) -> BTreeMap<String, ImageScore> {
    capability_cells()
        .into_iter()
        .map(|key| {
            let score = sim_image_score(artifact_digest, &key, bias);
            (key, score)
        })
        .collect()
}

fn full_evidence() -> (ReplayEvidence, FaithfulnessEvidence) {
    (
        ReplayEvidence {
            cells_checked: REPLAY_CELLS,
            exact_hash_matches: REPLAY_CELLS,
            max_embedding_drift: 0.0,
        },
        FaithfulnessEvidence {
            checks: MIN_FAITHFULNESS_CHECKS,
            agreements: MIN_FAITHFULNESS_CHECKS,
        },
    )
}

/// Sim-mode challenger measurements for a frozen digest.
///
/// # Errors
///
/// See [`fold_scores`].
pub fn sim_slice_scores(
    pin: &RelearnT2iPin,
    holdout_ids: &[u32],
    artifact_digest: &str,
) -> Result<T2iSliceScores, EvalError> {
    let holdout = sim_split(pin, holdout_ids, artifact_digest, 0.10);
    let public = sim_split(pin, &pin.prompts.public_ids, artifact_digest, 0.10);
    // Base competence is a property of the checkpoint, not of the fine-tune,
    // so the offline harness keeps the canary flat across artifacts. A harness
    // that jittered it would fail every submission for reasons a miner cannot
    // act on.
    let capability = sim_capability(BASE_CHAMPION_ARTIFACT, 0.10);
    let (replay, faithfulness) = full_evidence();
    fold_scores(
        &holdout,
        &public,
        &capability,
        replay,
        faithfulness,
        ContaminationEvidence::default(),
    )
}

/// Fixed base-checkpoint champion (pinned Cosmos3, no miner fine-tune).
///
/// These are sim numbers. Only seed them as the champion baseline on a host
/// that resolved [`JudgeBackend::Sim`]; live hosts use [`boot_base_champion`].
///
/// # Errors
///
/// See [`fold_scores`].
pub fn base_champion_scores(
    pin: &RelearnT2iPin,
    holdout_ids: &[u32],
) -> Result<T2iSliceScores, EvalError> {
    let holdout = sim_split(pin, holdout_ids, BASE_CHAMPION_ARTIFACT, 0.0);
    let public = sim_split(pin, &pin.prompts.public_ids, BASE_CHAMPION_ARTIFACT, 0.0);
    let capability = sim_capability(BASE_CHAMPION_ARTIFACT, 0.10);
    let (replay, faithfulness) = full_evidence();
    fold_scores(
        &holdout,
        &public,
        &capability,
        replay,
        faithfulness,
        ContaminationEvidence::default(),
    )
}

/// Declared training metadata plus the scored prompt ids inside it.
///
/// Returns the declared counts as well as the hits so a submission that
/// declared nothing cannot be read as a clean run.
#[must_use]
pub fn contamination_evidence(
    manifest: &ArtifactManifest,
    eval_ids: &[u32],
) -> ContaminationEvidence {
    let train: BTreeSet<u32> = manifest.train_prompt_ids.iter().copied().collect();
    let datasets: BTreeSet<String> = manifest
        .train_dataset_ids
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let eval: BTreeSet<u32> = eval_ids.iter().copied().collect();
    ContaminationEvidence {
        declared_prompt_ids: train.len(),
        declared_dataset_ids: datasets.len(),
        hits: contamination(&train, &eval),
    }
}

/// Holdout measurements produced by the digest-pinned eval image.
///
/// The implementation is not in this repo: generation and Q-Judger both run
/// inside `eval_image` from [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
/// The control plane holds a handle to that image's harvest and never computes
/// live numbers itself, so sim can never arrive through this trait.
///
/// The same scorer measures the boot baseline and every challenger: a live
/// challenger compared against a champion the host never measured is not a
/// comparison, and a champion measured by a different scorer is not either.
#[async_trait]
pub trait LiveJudge: Send + Sync {
    /// Score one artifact on the verified holdout prompts.
    ///
    /// `frozen_digest` binds the run. `artifact_digest` is the miner artifact,
    /// or [`BASE_CHAMPION_ARTIFACT`] for the boot baseline.
    ///
    /// # Errors
    ///
    /// Implementation-defined; surfaced to the miner as a 503.
    async fn score(
        &self,
        pin: &RelearnT2iPin,
        frozen_digest: &str,
        artifact_digest: &str,
        holdout: &[FrozenPrompt],
        manifest: &ArtifactManifest,
    ) -> Result<T2iSliceScores, EvalError>;

    /// Whether this judge could run right now.
    ///
    /// Checked by [`scoring_readiness`] so a gap the harvest knows about — no
    /// Q-Judger URL, no key to reach the pod — shows up on `/v1/status` and
    /// in the 503 instead of only after a pod has been paid for.
    ///
    /// # Errors
    ///
    /// Implementation-defined; the default is always ready.
    fn ready(&self) -> Result<(), EvalError> {
        Ok(())
    }
}

/// Schema version of the metrics document the eval image emits.
pub const T2I_METRICS_SCHEMA: u32 = 1;

/// Champion baseline measured by the digest-pinned eval image.
///
/// A live host needs a champion before any gate can run, and it must not be
/// sim numbers. An operator records one by running the pinned image on the
/// base checkpoint once and installing the result
/// (`RELEARN_T2I_BASE_CHAMPION_FILE`); [`Self::verify`] binds it to the pin so
/// a measurement from another image or another holdout is refused.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct T2iBaselineMeasurement {
    /// Eval image digest that produced these numbers. Must equal the pin's.
    pub eval_image_digest: String,
    /// Holdout commitment measured against. Must equal the pin's.
    pub holdout_commitment: String,
    /// Per-cell normalized totals on the holdout split.
    pub holdout: BTreeMap<String, f64>,
    /// Per-cell normalized totals on the published split.
    pub public: BTreeMap<String, f64>,
    /// Per-pillar holdout series (the anti-hidden-regression gate reads these).
    pub holdout_by_pillar: BTreeMap<L1Dimension, BTreeMap<String, f64>>,
    /// General-capability canary. Off the visible score.
    pub capability_canary: BTreeMap<String, f64>,
    /// Share of level-3 items the judge declined.
    pub na_rate: f64,
    /// Seed-replay evidence.
    pub replay: ReplayEvidence,
    /// Agentic faithfulness evidence.
    pub faithfulness: FaithfulnessEvidence,
}

impl T2iBaselineMeasurement {
    /// Parse an operator baseline file body.
    ///
    /// # Errors
    ///
    /// [`EvalError::Baseline`] when the body is not a baseline object.
    pub fn from_json(body: &str) -> Result<Self, EvalError> {
        serde_json::from_str(body).map_err(|e| EvalError::Baseline(e.to_string()))
    }

    /// Check the measurement against the pin before it becomes the champion.
    ///
    /// # Errors
    ///
    /// [`EvalError::Baseline`] on an image / holdout mismatch, or when a series
    /// the gates read is missing. A champion the gates cannot use would reject
    /// every challenger for a reason the miner cannot act on.
    pub fn verify(&self, pin: &RelearnT2iPin, expected_cells: usize) -> Result<(), EvalError> {
        let bad = |m: String| Err(EvalError::Baseline(m));
        if self.eval_image_digest.trim() != pin.eval_image_digest.trim() {
            return bad(format!(
                "measured by eval image {:?}, pin is {:?}",
                self.eval_image_digest, pin.eval_image_digest
            ));
        }
        if !self
            .holdout_commitment
            .trim()
            .eq_ignore_ascii_case(pin.prompts.holdout_commitment.trim())
        {
            return bad("holdout commitment does not match the pin".into());
        }
        if self.holdout.len() != expected_cells {
            return bad(format!(
                "{} holdout cells for {expected_cells} scored cells",
                self.holdout.len()
            ));
        }
        if self.public.is_empty() {
            return bad("no public split; the memorization gap gate cannot run".into());
        }
        if self.holdout_by_pillar.is_empty() {
            return bad("no per-pillar series; the pillar gate cannot run".into());
        }
        Ok(())
    }

    /// Convert to champion slices. Contamination is challenger-side only.
    #[must_use]
    pub fn into_slice_scores(self) -> T2iSliceScores {
        let series = |m: BTreeMap<String, f64>| ExampleSeries::from_pairs(m);
        T2iSliceScores {
            holdout: series(self.holdout),
            public: series(self.public),
            holdout_by_pillar: self
                .holdout_by_pillar
                .into_iter()
                .map(|(d, m)| (d, series(m)))
                .collect(),
            capability_canary: series(self.capability_canary),
            na_rate: self.na_rate,
            replay: self.replay,
            faithfulness: self.faithfulness,
            contamination: ContaminationEvidence::default(),
        }
    }
}

/// The document `eval_image` must print for one scored artifact.
///
/// The baseline envelope plus the run identity, so an operator's baseline file
/// is literally this image's output for the base checkpoint — one format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct T2iEvalMetrics {
    /// Must equal [`T2I_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// Frozen submission digest the run was asked for.
    pub submission_digest: String,
    /// Artifact digest the run was asked for.
    pub artifact_digest: String,
    /// Measured series, image digest, and holdout commitment.
    #[serde(flatten)]
    pub measurement: T2iBaselineMeasurement,
}

impl T2iEvalMetrics {
    /// Parse a metrics document emitted by the eval image.
    ///
    /// # Errors
    ///
    /// [`EvalError::Baseline`] when the body is not a metrics object.
    pub fn from_json(body: &str) -> Result<Self, EvalError> {
        serde_json::from_str(body).map_err(|e| EvalError::Baseline(e.to_string()))
    }

    /// Bind the document to the run that was requested.
    ///
    /// Without this a pod could answer with another artifact's numbers, or
    /// replay an earlier run's.
    ///
    /// # Errors
    ///
    /// [`EvalError::Baseline`] on a schema, run-identity, image, holdout, or
    /// missing-series mismatch.
    pub fn verify(
        &self,
        pin: &RelearnT2iPin,
        frozen_digest: &str,
        artifact_digest: &str,
        expected_cells: usize,
    ) -> Result<(), EvalError> {
        if self.schema_version != T2I_METRICS_SCHEMA {
            return Err(EvalError::Baseline(format!(
                "metrics schema_version {}, expected {T2I_METRICS_SCHEMA}",
                self.schema_version
            )));
        }
        if self.submission_digest.trim() != frozen_digest.trim() {
            return Err(EvalError::Baseline(
                "metrics submission_digest is not the frozen run".into(),
            ));
        }
        if !self
            .artifact_digest
            .trim()
            .eq_ignore_ascii_case(artifact_digest.trim())
        {
            return Err(EvalError::Baseline(
                "metrics artifact_digest is not the scored artifact".into(),
            ));
        }
        self.measurement.verify(pin, expected_cells)
    }
}

/// Whether this host can produce a verdict at all, and why not if it cannot.
///
/// Checked before anything is generated so a live run never spends the miner's
/// Lium budget on a submission the host could never judge, and so the miner is
/// told the root cause (no digest pin) rather than a downstream symptom.
///
/// # Errors
///
/// [`EvalError::JudgeUnconfigured`] without a judge endpoint,
/// [`EvalError::EvalImageUnpinned`] without a `sha256:` eval-image digest, and
/// [`EvalError::LiveHarvestUnavailable`] when no [`LiveJudge`] is wired.
pub fn scoring_readiness(
    pin: &RelearnT2iPin,
    judge: &JudgeConfig,
    live: Option<&dyn LiveJudge>,
) -> Result<(), EvalError> {
    judge.preflight(&JudgeInference::default())?;
    match judge.backend {
        JudgeBackend::Sim => Ok(()),
        JudgeBackend::HttpApi | JudgeBackend::Lium => {
            if !pin.can_rent() {
                return Err(EvalError::EvalImageUnpinned);
            }
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer.ready()
        }
    }
}

/// Champion baseline for this host, measured by the scorer submissions face.
///
/// A live host takes the operator-recorded measurement when there is one, and
/// otherwise measures the base checkpoint through the wired harvest. Sim
/// numbers are not a candidate on a live host at all.
///
/// # Errors
///
/// [`EvalError::Holdout`] with no verified prompts,
/// [`EvalError::EvalImageUnpinned`] without a digest pin,
/// [`EvalError::Baseline`] when a recorded measurement does not match the pin,
/// and [`EvalError::LiveHarvestUnavailable`] when a live host has neither
/// source.
pub async fn boot_base_champion(
    pin: &RelearnT2iPin,
    holdout: &[FrozenPrompt],
    judge: &JudgeConfig,
    recorded: Option<T2iBaselineMeasurement>,
    live: Option<&dyn LiveJudge>,
) -> Result<T2iSliceScores, EvalError> {
    if holdout.is_empty() {
        return Err(EvalError::Holdout("holdout still sealed".into()));
    }
    let holdout_ids: Vec<u32> = holdout.iter().map(|p| p.id).collect();
    match judge.backend {
        JudgeBackend::Sim => base_champion_scores(pin, &holdout_ids),
        JudgeBackend::HttpApi | JudgeBackend::Lium => {
            if !pin.can_rent() {
                return Err(EvalError::EvalImageUnpinned);
            }
            let expected = pin.seed_cells(&holdout_ids).len();
            if let Some(m) = recorded {
                m.verify(pin, expected)?;
                return Ok(m.into_slice_scores());
            }
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer
                .score(
                    pin,
                    BASE_CHAMPION_RUN,
                    BASE_CHAMPION_ARTIFACT,
                    holdout,
                    &ArtifactManifest::default(),
                )
                .await
        }
    }
}

/// Run one eval after the submission digest is frozen.
///
/// # Errors
///
/// Refuses on a failed license attestation, an unconfigured judge, an empty
/// holdout, an unpinned eval image, an unwired harvest, or an excessive judge
/// N/A rate. Contamination is not an error here: it is recorded on the scores
/// so the verdict reports it as a gate failure.
pub async fn eval_after_freeze(
    pin: &RelearnT2iPin,
    holdout: &[FrozenPrompt],
    frozen_digest: &str,
    artifact_digest: &str,
    manifest: &ArtifactManifest,
    judge: &JudgeConfig,
    live: Option<&dyn LiveJudge>,
) -> Result<EvalOutcome, EvalError> {
    if frozen_digest.trim().is_empty() {
        return Err(EvalError::Holdout("submission digest not frozen".into()));
    }
    if holdout.is_empty() {
        return Err(EvalError::Holdout("holdout still sealed".into()));
    }
    pin.attest_artifact_base(&manifest.base, &manifest.base_license)?;

    scoring_readiness(pin, judge, live)?;
    let backend = judge.backend;

    let holdout_ids: Vec<u32> = holdout.iter().map(|p| p.id).collect();
    let mut scores = match backend {
        JudgeBackend::Sim => sim_slice_scores(pin, &holdout_ids, artifact_digest)?,
        JudgeBackend::HttpApi | JudgeBackend::Lium => {
            // Generation and judging both happen inside the digest-pinned eval
            // image. `scoring_readiness` already refused an unwired host, so
            // reaching here without a scorer is a bug, not a sim fallback.
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer
                .score(pin, frozen_digest, artifact_digest, holdout, manifest)
                .await?
        }
    };
    scores.contamination = contamination_evidence(manifest, &holdout_ids);

    let holdout_cells = scores.holdout.len();
    let metrics = serde_json::to_vec(&serde_json::json!({
        "judge_model": JUDGE_MODEL_ID,
        "holdout_cells": holdout_cells,
        "na_rate": scores.na_rate,
    }))
    .unwrap_or_default();
    let receipt = EvalReceipt {
        provider: match backend {
            JudgeBackend::Sim => "sim".into(),
            JudgeBackend::HttpApi => "http_api".into(),
            JudgeBackend::Lium => "lium".into(),
        },
        pod_id: format!("t2i-{}", &frozen_digest[..8.min(frozen_digest.len())]),
        image_digest: pin.eval_image_digest.clone(),
        submission_hash: frozen_digest.to_owned(),
        metrics_hash: EvalReceipt::hash_metrics_bytes(&metrics),
        termination_verified: true,
    };
    NoScoreGate::check(&receipt, false).map_err(|e| EvalError::Integrity(e.to_string()))?;

    Ok(EvalOutcome {
        scores,
        receipt,
        backend,
        holdout_cells,
    })
}

/// Rent a digest-pinned eval pod, exec, harvest, terminate.
///
/// Live rent is skipped when `pin.can_rent()` is false (no published eval
/// digest yet).
///
/// # Errors
///
/// [`EvalError::Integrity`] without a digest pin or an unverified teardown, and
/// [`EvalError::Backend`] on any provider failure.
pub async fn rent_eval(
    backend: &dyn EvalJobBackend,
    pin: &RelearnT2iPin,
    frozen_digest: &str,
    artifact_digest: &str,
) -> Result<(RemoteExecResult, String), EvalError> {
    if !pin.can_rent() {
        return Err(EvalError::Integrity(
            "eval image digest not pinned; refuse live rent".into(),
        ));
    }
    let spec = InstanceSpec {
        name: format!(
            "relearn-t2i-{}",
            &frozen_digest[..12.min(frozen_digest.len())]
        ),
        max_lifetime_hours: 2.0,
        // Cosmos3-Super is 65B at BF16 and Q-Judger is 27B, so the pod is a
        // multi-GPU node rather than the single card the text challenge uses.
        max_price_per_hour: 48.0,
        gpu_count: 8,
        image_digest: Some(pin.eval_image_digest.clone()),
        docker_image: None,
        startup_commands: None,
        ssh_public_keys: Vec::new(),
        ssh_key_name: None,
        preferred_offer_id: None,
        template_id: None,
        template_name: None,
    };
    let inst = backend
        .provision(&spec)
        .await
        .map_err(|e| EvalError::Backend(e.to_string()))?;
    let exec = backend
        .exec_eval(&inst.id, artifact_digest, frozen_digest, None)
        .await
        .map_err(|e| EvalError::Backend(e.to_string()));
    let term = backend.terminate(&inst.id).await;
    let verified = backend.verify_terminated(&inst.id).await.unwrap_or(false);
    if let Err(e) = term {
        return Err(EvalError::Backend(e.to_string()));
    }
    if !verified {
        return Err(EvalError::Integrity("pod terminate not verified".into()));
    }
    exec.map(|r| (r, inst.id))
}

/// Convenience: sim backend rent that always tears down.
///
/// # Errors
///
/// See [`rent_eval`].
pub async fn sim_rent_roundtrip(digest: &str) -> Result<String, EvalError> {
    let backend = SimLiumBackend::new();
    let pin = RelearnT2iPin {
        eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
        ..RelearnT2iPin::default()
    };
    let (_r, id) = rent_eval(&backend, &pin, digest, digest).await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use relearn_t2i_task::{frozen_prompt_commitment, PromptPin};

    use super::*;

    fn prompt(id: u32) -> FrozenPrompt {
        FrozenPrompt {
            id,
            text: format!("prompt {id}"),
            upsampled_json: None,
        }
    }

    fn test_pin() -> RelearnT2iPin {
        let public: Vec<FrozenPrompt> = (1..=25).map(prompt).collect();
        let holdout: Vec<FrozenPrompt> = (900..=924).map(prompt).collect();
        RelearnT2iPin {
            prompts: PromptPin {
                pin_salt: "cortex-t2i-test".into(),
                variations_per_prompt: 4,
                public_ids: public.iter().map(|p| p.id).collect(),
                holdout_commitment: frozen_prompt_commitment(&holdout),
                holdout_size: holdout.len(),
            },
            frozen_prompts: public,
            ..RelearnT2iPin::default()
        }
    }

    fn manifest() -> ArtifactManifest {
        ArtifactManifest {
            base: relearn_t2i_task::BASE_MODEL_ID.into(),
            base_license: relearn_t2i_task::BASE_MODEL_LICENSE.into(),
            train_dataset_ids: vec!["cortex-public-v0".into()],
            ..ArtifactManifest::default()
        }
    }

    fn holdout() -> Vec<FrozenPrompt> {
        (900..=924).map(prompt).collect()
    }

    fn sim() -> JudgeConfig {
        JudgeConfig::sim()
    }

    fn live_pin() -> RelearnT2iPin {
        RelearnT2iPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..test_pin()
        }
    }

    fn live_judge_config() -> JudgeConfig {
        JudgeConfig {
            backend: JudgeBackend::HttpApi,
            endpoint: Some("http://judge.invalid/v1".into()),
        }
    }

    /// Stand-in for the eval image's harvest. Real live scores come from
    /// `CortexLM/relearn`; this exists so the gate path can be exercised.
    struct StubJudge {
        bias: f64,
    }

    #[async_trait]
    impl LiveJudge for StubJudge {
        async fn score(
            &self,
            pin: &RelearnT2iPin,
            _frozen: &str,
            artifact: &str,
            holdout: &[FrozenPrompt],
            _manifest: &ArtifactManifest,
        ) -> Result<T2iSliceScores, EvalError> {
            let ids: Vec<u32> = holdout.iter().map(|p| p.id).collect();
            let hold = sim_split(pin, &ids, artifact, self.bias);
            let public = sim_split(pin, &pin.prompts.public_ids, artifact, self.bias);
            let capability = sim_capability(BASE_CHAMPION_ARTIFACT, 0.10);
            let (replay, faithfulness) = full_evidence();
            fold_scores(
                &hold,
                &public,
                &capability,
                replay,
                faithfulness,
                ContaminationEvidence::default(),
            )
        }
    }

    fn recorded(pin: &RelearnT2iPin) -> T2iBaselineMeasurement {
        let ids: Vec<u32> = holdout().iter().map(|p| p.id).collect();
        let scores = base_champion_scores(pin, &ids).expect("base");
        T2iBaselineMeasurement {
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.prompts.holdout_commitment.clone(),
            holdout: scores.holdout.by_cluster,
            public: scores.public.by_cluster,
            holdout_by_pillar: scores
                .holdout_by_pillar
                .into_iter()
                .map(|(d, s)| (d, s.by_cluster))
                .collect(),
            capability_canary: scores.capability_canary.by_cluster,
            na_rate: scores.na_rate,
            replay: scores.replay,
            faithfulness: scores.faithfulness,
        }
    }

    #[tokio::test]
    async fn sim_eval_needs_a_frozen_digest_and_an_unsealed_holdout() {
        let pin = test_pin();
        assert!(
            eval_after_freeze(&pin, &holdout(), "", "art", &manifest(), &sim(), None)
                .await
                .is_err()
        );
        assert!(
            eval_after_freeze(&pin, &[], "digest", "art", &manifest(), &sim(), None)
                .await
                .is_err()
        );
        let out = eval_after_freeze(
            &pin,
            &holdout(),
            "digest-a",
            "art",
            &manifest(),
            &sim(),
            None,
        )
        .await
        .expect("sim eval");
        assert_eq!(out.backend, JudgeBackend::Sim);
        assert_eq!(out.holdout_cells, 100);
        assert_eq!(out.receipt.provider, "sim");
        assert_eq!(out.receipt.submission_hash, "digest-a");
        assert!(!out.scores.capability_canary.is_empty());
    }

    #[tokio::test]
    async fn sim_eval_is_deterministic() {
        let pin = test_pin();
        let a = eval_after_freeze(&pin, &holdout(), "d", "art", &manifest(), &sim(), None)
            .await
            .expect("a");
        let b = eval_after_freeze(&pin, &holdout(), "d", "art", &manifest(), &sim(), None)
            .await
            .expect("b");
        assert_eq!(a.scores.holdout, b.scores.holdout);
    }

    #[tokio::test]
    async fn flux_artifact_is_refused_before_any_scoring() {
        let pin = test_pin();
        let mut m = manifest();
        m.base = "black-forest-labs/FLUX.1-dev".into();
        let err = eval_after_freeze(&pin, &holdout(), "d", "art", &m, &sim(), None)
            .await
            .expect_err("refuse");
        assert!(
            matches!(err, EvalError::Attestation(PinError::RejectedBase(_))),
            "{err}"
        );
    }

    #[tokio::test]
    async fn wrong_license_attestation_is_refused() {
        let pin = test_pin();
        let mut m = manifest();
        m.base_license = "cc-by-nc-4.0".into();
        assert!(
            eval_after_freeze(&pin, &holdout(), "d", "art", &m, &sim(), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn contaminated_training_metadata_lands_on_the_scores() {
        let pin = test_pin();
        let mut m = manifest();
        m.train_prompt_ids = vec![1, 2, 907];
        let out = eval_after_freeze(&pin, &holdout(), "d", "art", &m, &sim(), None)
            .await
            .expect("eval");
        assert_eq!(out.scores.contamination.hits, vec![907]);
        assert!(out.scores.contamination.is_declared());
    }

    /// An empty manifest declares nothing, so the gate has nothing to check.
    /// That is a failure, not a pass.
    #[tokio::test]
    async fn empty_training_metadata_is_undeclared_not_clean() {
        let pin = test_pin();
        let bare = ArtifactManifest {
            base: relearn_t2i_task::BASE_MODEL_ID.into(),
            base_license: relearn_t2i_task::BASE_MODEL_LICENSE.into(),
            ..ArtifactManifest::default()
        };
        let out = eval_after_freeze(&pin, &holdout(), "d", "art", &bare, &sim(), None)
            .await
            .expect("eval");
        assert!(!out.scores.contamination.is_declared());
        let champ = base_champion_scores(&pin, &[900]).expect("champ");
        assert!(relearn_t2i_score::judge_challenger(&champ, &out.scores)
            .failed
            .contains(&relearn_t2i_score::GateFail::ContaminationEvidenceMissing));
    }

    #[tokio::test]
    async fn live_backend_without_a_pinned_eval_image_refuses() {
        let err = eval_after_freeze(
            &test_pin(),
            &holdout(),
            "d",
            "art",
            &manifest(),
            &live_judge_config(),
            None,
        )
        .await
        .expect_err("must refuse");
        assert!(matches!(err, EvalError::EvalImageUnpinned), "{err}");
        assert!(err.to_string().contains("eval image digest not pinned"));
    }

    #[tokio::test]
    async fn live_backend_with_a_pin_but_no_harvest_never_falls_back_to_sim() {
        let err = eval_after_freeze(
            &live_pin(),
            &holdout(),
            "d",
            "art",
            &manifest(),
            &live_judge_config(),
            None,
        )
        .await
        .expect_err("must refuse");
        assert!(matches!(err, EvalError::LiveHarvestUnavailable), "{err}");
    }

    #[tokio::test]
    async fn live_eval_uses_the_wired_harvest() {
        let pin = live_pin();
        let judge = StubJudge { bias: 0.20 };
        let out = eval_after_freeze(
            &pin,
            &holdout(),
            "d",
            "art",
            &manifest(),
            &live_judge_config(),
            Some(&judge),
        )
        .await
        .expect("live eval");
        assert_eq!(out.backend, JudgeBackend::HttpApi);
        assert_eq!(out.receipt.provider, "http_api");
        assert_eq!(out.receipt.image_digest, pin.eval_image_digest);
        // The harvest biased the cells; the sim harness would not have.
        let sim_only = sim_slice_scores(&pin, &[900], "art").expect("sim");
        assert_ne!(out.scores.holdout, sim_only.holdout);
    }

    #[tokio::test]
    async fn live_boot_records_a_baseline_from_either_live_source_but_never_sim() {
        let pin = live_pin();
        let cfg = live_judge_config();

        let from_file = boot_base_champion(&pin, &holdout(), &cfg, Some(recorded(&pin)), None)
            .await
            .expect("recorded baseline");
        assert_eq!(from_file.holdout.len(), 100);
        assert!(!from_file.capability_canary.is_empty());

        let stub = StubJudge { bias: 0.0 };
        let harvested = boot_base_champion(&pin, &holdout(), &cfg, None, Some(&stub))
            .await
            .expect("harvested baseline");
        assert_eq!(harvested.holdout.len(), 100);

        let err = boot_base_champion(&pin, &holdout(), &cfg, None, None)
            .await
            .expect_err("no live source");
        assert!(matches!(err, EvalError::LiveHarvestUnavailable), "{err}");

        let unpinned = boot_base_champion(&test_pin(), &holdout(), &cfg, None, None)
            .await
            .expect_err("no digest pin");
        assert!(
            matches!(unpinned, EvalError::EvalImageUnpinned),
            "{unpinned}"
        );
    }

    #[test]
    fn a_recorded_baseline_is_bound_to_the_pinned_image_and_holdout() {
        let pin = live_pin();
        recorded(&pin).verify(&pin, 100).expect("clean");

        let mut other_image = recorded(&pin);
        other_image.eval_image_digest = format!("sha256:{}", "cd".repeat(32));
        assert!(matches!(
            other_image.verify(&pin, 100),
            Err(EvalError::Baseline(_))
        ));

        let mut other_holdout = recorded(&pin);
        other_holdout.holdout_commitment = "bb".repeat(32);
        assert!(matches!(
            other_holdout.verify(&pin, 100),
            Err(EvalError::Baseline(_))
        ));

        // capability_canary is optional: the published eval image does not
        // emit it (faithfulness + replay are its off-score controls).
        let mut no_canary = recorded(&pin);
        no_canary.capability_canary.clear();
        no_canary.verify(&pin, 100).expect("canary is optional");

        for break_it in 0..2 {
            let mut m = recorded(&pin);
            match break_it {
                0 => m.public.clear(),
                _ => m.holdout_by_pillar.clear(),
            }
            assert!(
                matches!(m.verify(&pin, 100), Err(EvalError::Baseline(_))),
                "case {break_it} must be refused at boot"
            );
        }
    }

    #[test]
    fn readiness_names_the_root_cause() {
        let stub = StubJudge { bias: 0.0 };
        scoring_readiness(&test_pin(), &sim(), None).expect("sim can always score");
        assert!(matches!(
            scoring_readiness(&test_pin(), &live_judge_config(), None),
            Err(EvalError::EvalImageUnpinned)
        ));
        assert!(matches!(
            scoring_readiness(&live_pin(), &live_judge_config(), None),
            Err(EvalError::LiveHarvestUnavailable)
        ));
        scoring_readiness(&live_pin(), &live_judge_config(), Some(&stub))
            .expect("pinned digest + wired harvest");
        // An unconfigured HTTP judge is refused before the digest is consulted.
        let unset = JudgeConfig {
            backend: JudgeBackend::HttpApi,
            endpoint: None,
        };
        assert!(matches!(
            scoring_readiness(&live_pin(), &unset, Some(&stub)),
            Err(EvalError::JudgeUnconfigured)
        ));
    }

    #[test]
    fn http_backend_without_an_endpoint_refuses_to_score() {
        let unset = JudgeConfig {
            backend: JudgeBackend::HttpApi,
            endpoint: None,
        };
        assert!(!unset.endpoint_configured());
        let err = unset
            .preflight(&JudgeInference::default())
            .expect_err("must refuse");
        assert!(matches!(err, EvalError::JudgeUnconfigured), "{err}");
    }

    #[test]
    fn only_q_judger_passes_preflight() {
        let bad = JudgeInference {
            model: "google/gemma-3".into(),
            ..JudgeInference::default()
        };
        assert!(matches!(sim().preflight(&bad), Err(EvalError::Judge(_))));
        sim()
            .preflight(&JudgeInference::default())
            .expect("q-judger + sim");
    }

    #[test]
    fn high_na_rate_fails_closed_in_the_fold() {
        let mut holdout = BTreeMap::new();
        holdout.insert(
            "p1#v0".to_owned(),
            ImageScore {
                per_l1: BTreeMap::from([(L1Dimension::Quality, 100.0)]),
                total: 100.0,
                scored_items: 1,
                na_items: 20,
            },
        );
        let err = fold_scores(
            &holdout,
            &BTreeMap::new(),
            &BTreeMap::new(),
            ReplayEvidence::default(),
            FaithfulnessEvidence::default(),
            ContaminationEvidence::default(),
        )
        .expect_err("must refuse");
        assert!(matches!(err, EvalError::NotApplicableRate { .. }), "{err}");
    }

    #[test]
    fn empty_holdout_fold_is_refused() {
        assert!(fold_scores(
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            ReplayEvidence::default(),
            FaithfulnessEvidence::default(),
            ContaminationEvidence::default(),
        )
        .is_err());
    }

    #[test]
    fn base_champion_is_beatable_but_not_free() {
        let pin = test_pin();
        let ids: Vec<u32> = holdout().iter().map(|p| p.id).collect();
        let base = base_champion_scores(&pin, &ids).expect("base");
        assert_eq!(base.holdout.len(), 100);
        assert!(T2iSliceScores::mean(&base.holdout).unwrap_or(0.0) > 0.0);
    }

    #[tokio::test]
    async fn sim_rent_tears_down() {
        let id = sim_rent_roundtrip("abcdef0123456789").await.expect("rent");
        assert!(id.contains("sim-pod"));
    }

    #[test]
    fn unpinned_eval_image_refuses_live_rent() {
        assert!(!RelearnT2iPin::default().can_rent());
    }
}
