//! Relearn eval loop: freeze digest → unseal holdout → rent/sim → harvest.
//!
//! Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). The control plane
//! only ever boots a digest-pinned eval image. Teacher HTTP is judge-only
//! and never serves miner weights as the scored artifact.
//!
//! The deterministic sim scorer is **not** a fallback. It runs only when the
//! operator sets `RELEARN_FORCE_SIM=1`; otherwise a host without a
//! `sha256:` eval-image pin refuses to score at all rather than shipping
//! sim numbers to the lattice.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::must_use_candidate,
    clippy::significant_drop_tightening
)]

use std::collections::BTreeMap;

use async_trait::async_trait;
use prism_competition::ExampleSeries;
use prism_lium::{EvalJobBackend, SimLiumBackend};
use prism_lium_types::{EvalReceipt, InstanceSpec, NoScoreGate, RemoteExecResult};
use relearn_challenge_task::{
    default_teacher_backend, is_configured_teacher_model, HoldoutItem, HoldoutTask, TeacherBackend,
    BASE_MODEL_ID, MIN_HOLDOUT_ITEMS, TEACHER_MODEL_ID, TEACHER_NVFP4_ID,
};
use relearn_score::{
    ContaminationEvidence, ShuffleEvidence, SliceScores, MAX_PERTURB_DROP, MAX_PUBLIC_PRIVATE_GAP,
    MIN_SHUFFLE_DROP,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Pins Cortex stores for the split `CortexLM/relearn` repo + eval image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelearnPin {
    /// Language / VLM base. Do not recale here; the pin owner owns the id.
    pub base_model: String,
    /// HTTP teacher wire id (`glm-5.3` default).
    pub teacher_model: String,
    /// NVFP4 weights id to download. Serve from `RELEARN_TEACHER_LOCAL_DIR`.
    pub teacher_nvfp4: String,
    /// `lium_nvfp4` | `http_api` | `sim`.
    pub teacher_backend: TeacherBackend,
    /// Eval image reference (no floating tag in prod).
    pub eval_image: String,
    /// `sha256:…` digest. Empty until the first green relearn CI image.
    pub eval_image_digest: String,
    /// `https://github.com/CortexLM/relearn`.
    pub relearn_git: String,
    /// Pinned git SHA of CortexLM/relearn (empty until first push).
    pub relearn_git_sha: String,
    /// Commitment over the operator holdout file. Required.
    pub holdout_commitment: String,
    /// Expected holdout record count.
    pub holdout_size: usize,
    /// Published item ids. Miners may train on these.
    pub public_ids: Vec<u32>,
}

impl Default for RelearnPin {
    fn default() -> Self {
        Self {
            base_model: BASE_MODEL_ID.into(),
            teacher_model: TEACHER_MODEL_ID.into(),
            teacher_nvfp4: TEACHER_NVFP4_ID.into(),
            teacher_backend: TeacherBackend::HttpApi,
            eval_image: "ghcr.io/cortexlm/relearn-eval".into(),
            eval_image_digest: String::new(),
            relearn_git: relearn_challenge_task::RELEARN_GIT_URL.into(),
            relearn_git_sha: String::new(),
            holdout_commitment: String::new(),
            holdout_size: 0,
            public_ids: Vec::new(),
        }
    }
}

/// Why a pin was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PinError {
    /// TOML did not parse.
    #[error("parse relearn pin: {0}")]
    Parse(String),
    /// Holdout commitment is not a 64-hex digest.
    #[error("holdout_commitment must be 64 hex chars")]
    BadHoldoutCommitment,
    /// Holdout split is too thin for a verdict.
    #[error("holdout_size {got} is below the {min} floor")]
    TooFewHoldout {
        /// Count the pin declared.
        got: usize,
        /// Required floor.
        min: usize,
    },
}

impl RelearnPin {
    /// Load from `config/relearn-pin.toml`.
    ///
    /// # Errors
    ///
    /// [`PinError::Parse`] on malformed TOML. Call [`validate`] before boot.
    pub fn from_toml(body: &str) -> Result<Self, PinError> {
        toml::from_str(body).map_err(|e| PinError::Parse(e.to_string()))
    }

    /// Enforce holdout commitment / size. Model ids are not rewritten here.
    ///
    /// # Errors
    ///
    /// [`PinError::BadHoldoutCommitment`] or [`PinError::TooFewHoldout`].
    pub fn validate(&self) -> Result<(), PinError> {
        let commitment = self.holdout_commitment.trim();
        if commitment.len() != 64 || !commitment.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(PinError::BadHoldoutCommitment);
        }
        if self.holdout_size < MIN_HOLDOUT_ITEMS {
            return Err(PinError::TooFewHoldout {
                got: self.holdout_size,
                min: MIN_HOLDOUT_ITEMS,
            });
        }
        Ok(())
    }

    /// True when a live rent is allowed (real digest pin present).
    #[must_use]
    pub fn can_rent(&self) -> bool {
        self.eval_image_digest.starts_with("sha256:") && self.eval_image_digest.len() >= 71
    }
}

/// Where a Relearn eval actually runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalBackend {
    /// Digest-pinned eval image on a Lium pod (production default).
    Lium,
    /// Deterministic offline scorer. CI / local opt-in only, never a fallback.
    Sim,
}

/// True when the operator explicitly opted into sim scoring.
#[must_use]
pub fn force_sim() -> bool {
    matches!(
        std::env::var("RELEARN_FORCE_SIM")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

/// Resolve the scoring backend for this host. Sim is never implicit.
#[must_use]
pub fn resolve_eval_backend() -> EvalBackend {
    if force_sim() {
        EvalBackend::Sim
    } else {
        EvalBackend::Lium
    }
}

/// Eval errors.
#[derive(Debug, Error)]
pub enum EvalError {
    /// Holdout was requested before the digest freeze.
    #[error("holdout still sealed")]
    HoldoutSealed,
    /// Integrity gate failed.
    #[error("integrity: {0}")]
    Integrity(String),
    /// Lium / backend failure.
    #[error("backend: {0}")]
    Backend(String),
    /// Teacher API is not allowed to receive miner weights.
    #[error("teacher API refused miner-weight payload")]
    TeacherMinerWeights,
    /// A live run was asked for without a digest-pinned eval image.
    #[error("eval image digest not pinned; refuse live scoring (RELEARN_FORCE_SIM=1 is CI only)")]
    EvalImageUnpinned,
    /// A live run reached the in-process scorer. It must not silently sim.
    #[error("live holdout harvest is driven by the digest-pinned eval image; no in-process sim")]
    LiveHarvestUnavailable,
    /// The operator-recorded champion baseline does not match the pin.
    #[error("recorded baseline: {0}")]
    Baseline(String),
}

/// Champion baseline measured by the digest-pinned eval image and installed by
/// the operator (`RELEARN_BASE_CHAMPION_FILE`).
///
/// A live host needs a champion baseline before any gate can run, and it must
/// not be sim numbers. Until the eval-image harvest is wired into the control
/// plane, this is how the operator records one: run the pinned image on the
/// base model once, install the result. [`Self::verify`] binds it to the pin,
/// so a measurement from a different image or a different holdout is refused.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BaselineMeasurement {
    /// Eval image digest that produced these numbers. Must equal the pin's.
    pub eval_image_digest: String,
    /// Holdout commitment measured against. Must equal the pin's.
    pub holdout_commitment: String,
    /// Per-item holdout scores. The only series that may enter the lattice.
    pub holdout: BTreeMap<String, f64>,
    /// Public / training-adjacent split.
    pub public: BTreeMap<String, f64>,
    /// Holdout items after the pinned perturbation.
    pub perturbed: BTreeMap<String, f64>,
    /// Known-answer base-competence canaries.
    pub canaries: BTreeMap<String, f64>,
    /// General benches (MMLU / MMMU / …), off the visible score.
    pub general_canary: BTreeMap<String, f64>,
    /// Agent-trace quality in `[0, 1]`.
    pub agent_trace: f64,
    /// Pixel-shuffle control per vision family.
    pub vision_shuffle: BTreeMap<HoldoutTask, ShuffleEvidence>,
}

impl BaselineMeasurement {
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
    /// the gates need is missing. A champion the gates cannot use would reject
    /// every challenger for reasons the miner cannot act on.
    pub fn verify(&self, pin: &RelearnPin, holdout: &[HoldoutItem]) -> Result<(), EvalError> {
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
            .eq_ignore_ascii_case(pin.holdout_commitment.trim())
        {
            return bad("holdout commitment does not match the pin".into());
        }
        if self.holdout.len() != holdout.len() {
            return bad(format!(
                "{} holdout scores for {} verified items",
                self.holdout.len(),
                holdout.len()
            ));
        }
        // Each of these is a gate the challenger is measured on by the same
        // image. A champion missing one means the image does not emit it at
        // all, so every challenger would fail closed for a reason the miner
        // cannot act on. Refuse at boot, where the operator can fix it.
        for (series, why) in [
            (
                &self.general_canary,
                "no general-bench canary; every challenger would fail closed",
            ),
            (
                &self.public,
                "no public split; the memorization gap gate cannot run",
            ),
            (
                &self.perturbed,
                "no perturbed rerun; the brittleness gate cannot run",
            ),
            (
                &self.canaries,
                "no known-answer canaries; the base-competence gate cannot run",
            ),
        ] {
            if series.is_empty() {
                return bad(why.into());
            }
        }
        if !self.agent_trace.is_finite() || !(0.0..=1.0).contains(&self.agent_trace) {
            return bad(format!("agent_trace {} outside [0, 1]", self.agent_trace));
        }
        Ok(())
    }

    /// Convert to champion slices. Contamination is challenger-side only.
    #[must_use]
    pub fn into_slice_scores(self) -> SliceScores {
        let series = |m: BTreeMap<String, f64>| ExampleSeries::from_pairs(m);
        SliceScores {
            holdout: series(self.holdout),
            public: series(self.public),
            perturbed: series(self.perturbed),
            canaries: series(self.canaries),
            general_canary: series(self.general_canary),
            agent_trace: self.agent_trace,
            vision_shuffle: self.vision_shuffle,
            contamination: ContaminationEvidence::default(),
        }
    }
}

/// Holdout measurements produced by the digest-pinned eval image.
///
/// The implementation is not in this repo. The control plane holds a handle to
/// the eval image's harvest and never computes live numbers itself, so sim can
/// never arrive through this trait.
///
/// The same scorer measures the boot baseline and every challenger: a live
/// challenger compared against a champion the host never measured is not a
/// comparison, and a champion measured by a different scorer is not either.
///
/// The Lium implementation is `relearn-lium-harvest`.
#[async_trait]
pub trait LiveScorer: Send + Sync {
    /// Score one artifact on the verified holdout.
    ///
    /// `frozen_digest` binds the run. `artifact_digest` is the miner artifact,
    /// or [`BASE_CHAMPION_ARTIFACT`] for the boot baseline.
    ///
    /// # Errors
    ///
    /// Implementation-defined; surfaced to the miner as a 503.
    async fn score(
        &self,
        pin: &RelearnPin,
        frozen_digest: &str,
        artifact_digest: &str,
        holdout: &[HoldoutItem],
    ) -> Result<SliceScores, EvalError>;

    /// Whether this scorer could run right now.
    ///
    /// Checked by [`scoring_readiness`] so a gap the scorer knows about — no
    /// judge configured, no key to reach the pod — shows up on `/v1/status`
    /// and in the 503 instead of only after a pod has been paid for.
    ///
    /// # Errors
    ///
    /// Implementation-defined; the default is always ready.
    fn ready(&self) -> Result<(), EvalError> {
        Ok(())
    }

    /// Whether the harvest will hand the pod a backbone (dir or explicit pull).
    fn base_weights_primed(&self) -> bool {
        true
    }

    /// Which priming var is set. Name only; never a filesystem path.
    fn base_weights_via(&self) -> Option<&'static str> {
        None
    }
}

/// Schema version of the metrics document the eval image emits.
pub const RELEARN_METRICS_SCHEMA: u32 = 1;

/// The document `eval_image` must print for one scored artifact.
///
/// This is the image contract (see `docs/RELEARN.md` § Eval image contract).
/// It is the [`BaselineMeasurement`] envelope plus the run identity, so the
/// operator's baseline file is literally this image's output for the base
/// model — one format, not two.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelearnEvalMetrics {
    /// Must equal [`RELEARN_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// Frozen submission digest the run was asked for.
    pub submission_digest: String,
    /// Artifact digest the run was asked for.
    pub artifact_digest: String,
    /// Measured series, image digest, and holdout commitment.
    #[serde(flatten)]
    pub measurement: BaselineMeasurement,
}

impl RelearnEvalMetrics {
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
        pin: &RelearnPin,
        frozen_digest: &str,
        artifact_digest: &str,
        holdout: &[HoldoutItem],
    ) -> Result<(), EvalError> {
        if self.schema_version != RELEARN_METRICS_SCHEMA {
            return Err(EvalError::Baseline(format!(
                "metrics schema_version {}, expected {RELEARN_METRICS_SCHEMA}",
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
        self.measurement.verify(pin, holdout)
    }
}

/// Whether this host can produce a verdict at all, and why not if it cannot.
///
/// Checked before the holdout is scored so a live run never spends the miner's
/// Lium budget on a submission the host could never judge, and so the miner is
/// told the root cause (no digest pin) rather than a downstream symptom.
///
/// # Errors
///
/// [`EvalError::EvalImageUnpinned`] without a `sha256:` eval-image digest, and
/// [`EvalError::LiveHarvestUnavailable`] when no [`LiveScorer`] is wired.
pub fn scoring_readiness(
    pin: &RelearnPin,
    backend: EvalBackend,
    live: Option<&dyn LiveScorer>,
) -> Result<(), EvalError> {
    match backend {
        EvalBackend::Sim => Ok(()),
        EvalBackend::Lium => {
            if !pin.can_rent() {
                return Err(EvalError::EvalImageUnpinned);
            }
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer.ready()
        }
    }
}

/// Artifact id of the un-post-trained base model on the holdout.
pub const BASE_CHAMPION_ARTIFACT: &str = "base-relearn-champion";

/// Run id bound into the boot baseline measurement.
pub const BASE_CHAMPION_RUN: &str = "boot-baseline";

/// One finished eval.
#[derive(Debug, Clone)]
pub struct EvalOutcome {
    /// Challenger measurements.
    pub scores: SliceScores,
    /// Integrity receipt.
    pub receipt: EvalReceipt,
    /// Backend that produced the scores.
    pub backend: EvalBackend,
    /// Holdout item count that was scored (ids stay off the HTTP row).
    pub holdout_items: usize,
}

fn unit(parts: &[&str], index: u32) -> f64 {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0xff]);
    }
    h.update(index.to_le_bytes());
    let d = h.finalize();
    f64::from(d[0]) / 255.0
}

fn series_ids(prefix: &str, ids: &[u32], digest: &str, salt: &str, bias: f64) -> ExampleSeries {
    ExampleSeries::from_pairs(ids.iter().map(|id| {
        let v = unit(&[digest, salt, &id.to_string()], *id);
        (
            format!("{prefix}{id}"),
            (0.45 + 0.4 * v + bias).clamp(0.0, 1.0),
        )
    }))
}

/// Per-item noise band around the sim skill level.
///
/// Smaller than [`SIM_SKILL_SPAN`] so a skill gap survives the noise and the
/// paired test can actually decide; larger than `DEADZONE` so items are not
/// all ties.
const SIM_NOISE: f64 = 0.10;

/// How far sim skill moves the mean holdout score.
const SIM_SKILL_SPAN: f64 = 0.30;

/// Sim skill of the base champion, in `[0, 1]`.
///
/// Fixed rather than digest-derived so a harness can aim above or below it.
pub const BASE_CHAMPION_SKILL: f64 = 0.40;

/// Deterministic sim skill of an artifact, in `[0, 1]`.
///
/// A digest above [`BASE_CHAMPION_SKILL`] by more than the noise band beats
/// the sim champion; one below it loses. That is the point: the offline
/// harness has to be able to produce both verdicts.
#[must_use]
pub fn sim_artifact_skill(artifact_digest: &str) -> f64 {
    unit(&[artifact_digest, "skill"], 0)
}

fn skill_series(
    prefix: &str,
    ids: &[u32],
    digest: &str,
    salt: &str,
    skill: f64,
    bias: f64,
) -> ExampleSeries {
    ExampleSeries::from_pairs(ids.iter().map(|id| {
        let v = unit(&[digest, salt, &id.to_string()], *id);
        (
            format!("{prefix}{id}"),
            (0.45 + SIM_SKILL_SPAN * skill + SIM_NOISE * v + bias).clamp(0.0, 1.0),
        )
    }))
}

/// Deterministic sim scores from a frozen digest + verified holdout records.
///
/// Public and general-canary series are produced from salts that do **not**
/// include the holdout prompts, so they cannot reconstruct the private split.
///
/// Skill comes from [`sim_artifact_skill`]. Use
/// [`sim_slice_scores_at_skill`] to pin it.
#[must_use]
pub fn sim_slice_scores(artifact_digest: &str, holdout: &[HoldoutItem]) -> SliceScores {
    sim_slice_scores_at_skill(
        artifact_digest,
        holdout,
        sim_artifact_skill(artifact_digest),
    )
}

/// Sim scores at an explicit skill level in `[0, 1]`.
///
/// The retention series are derived from the holdout series rather than drawn
/// independently: perturbation, public gap, and canaries are gates on the
/// *same* model, so an offline harness whose slices disagree with each other
/// would fail every gate no matter what the submission did.
#[must_use]
pub fn sim_slice_scores_at_skill(
    artifact_digest: &str,
    holdout: &[HoldoutItem],
    skill: f64,
) -> SliceScores {
    let skill = skill.clamp(0.0, 1.0);
    let hold_ids: Vec<u32> = holdout.iter().map(|r| r.id).collect();
    let public_ids: Vec<u32> = (1..=40).collect();
    let mut vision_shuffle = BTreeMap::new();
    for task in HoldoutTask::VISION {
        let n = holdout.iter().filter(|r| r.task == task).count();
        if n == 0 {
            continue;
        }
        let score =
            (0.55 + 0.2 * unit(&[artifact_digest, task.as_str()], n as u32)).clamp(0.0, 1.0);
        vision_shuffle.insert(
            task,
            ShuffleEvidence {
                items: u32::try_from(n).unwrap_or(u32::MAX),
                score,
                shuffled_score: (score - 2.0 * MIN_SHUFFLE_DROP).max(0.0),
            },
        );
    }
    SliceScores {
        holdout: skill_series("h", &hold_ids, artifact_digest, "hold", skill, 0.0),
        // Train-adjacent, so a little easier — but inside the memorization gap.
        public: skill_series(
            "p",
            &public_ids,
            artifact_digest,
            "public",
            skill,
            0.5 * MAX_PUBLIC_PRIVATE_GAP,
        ),
        // A perturbed rerun of the same model: the "hold" draw again, small drop.
        perturbed: skill_series(
            "x",
            &hold_ids,
            artifact_digest,
            "hold",
            skill,
            -0.5 * MAX_PERTURB_DROP,
        ),
        canaries: series_ids("c", &(0..40).collect::<Vec<_>>(), "canary", "base", 0.45),
        general_canary: series_ids(
            "g",
            &(0..40).collect::<Vec<_>>(),
            "mmlu-mmmu",
            "offpath",
            0.50,
        ),
        agent_trace: 0.85,
        vision_shuffle,
        contamination: ContaminationEvidence::default(),
    }
}

/// Baseline champion on the verified holdout (no miner adapter).
///
/// These are sim numbers. Only seed them as the champion baseline on a host
/// that resolved [`EvalBackend::Sim`]; a live challenger must never be judged
/// against a simulated champion. Live hosts use [`boot_base_champion`].
#[must_use]
pub fn base_champion_scores(holdout: &[HoldoutItem]) -> SliceScores {
    sim_slice_scores_at_skill(BASE_CHAMPION_ARTIFACT, holdout, BASE_CHAMPION_SKILL)
}

/// Champion baseline for this host, measured by the scorer submissions face.
///
/// A live host records the base model through the eval image; it does not fall
/// back to [`base_champion_scores`]. Without a baseline the gates never run —
/// contamination, public-holdout gap, and pixel-shuffle all need a champion to
/// compare against — so this is called at boot and its failure is the reason
/// submissions refuse, reported on `/v1/status`.
///
/// A live host takes the operator-recorded measurement when there is one, and
/// otherwise measures through the wired harvest. Sim numbers are not a
/// candidate on a live host at all.
///
/// # Errors
///
/// [`EvalError::HoldoutSealed`] with no verified records,
/// [`EvalError::EvalImageUnpinned`] without a digest pin,
/// [`EvalError::Baseline`] when a recorded measurement does not match the pin,
/// and [`EvalError::LiveHarvestUnavailable`] when a live host has neither
/// source.
pub async fn boot_base_champion(
    pin: &RelearnPin,
    holdout: &[HoldoutItem],
    backend: EvalBackend,
    recorded: Option<BaselineMeasurement>,
    live: Option<&dyn LiveScorer>,
) -> Result<SliceScores, EvalError> {
    if holdout.is_empty() {
        return Err(EvalError::HoldoutSealed);
    }
    match backend {
        EvalBackend::Sim => Ok(base_champion_scores(holdout)),
        EvalBackend::Lium => {
            if !pin.can_rent() {
                return Err(EvalError::EvalImageUnpinned);
            }
            if let Some(m) = recorded {
                m.verify(pin, holdout)?;
                return Ok(m.into_slice_scores());
            }
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer
                .score(pin, BASE_CHAMPION_RUN, BASE_CHAMPION_ARTIFACT, holdout)
                .await
        }
    }
}

/// Score only after the submission digest is frozen and holdout records exist.
///
/// `backend` comes from [`resolve_eval_backend`] and is the only thing that
/// can select sim. On [`EvalBackend::Lium`] this refuses rather than falling
/// back: without a `sha256:` eval-image pin the answer is
/// [`EvalError::EvalImageUnpinned`], and without a wired [`LiveScorer`] it is
/// [`EvalError::LiveHarvestUnavailable`]. Sim is never substituted.
///
/// # Errors
///
/// [`EvalError::HoldoutSealed`] before the freeze / without verified records,
/// [`EvalError::EvalImageUnpinned`] or [`EvalError::LiveHarvestUnavailable`]
/// on a live host, and [`EvalError::Integrity`] when the receipt gate fails.
pub async fn eval_after_freeze(
    pin: &RelearnPin,
    frozen_digest: &str,
    artifact_digest: &str,
    holdout: &[HoldoutItem],
    backend: EvalBackend,
    live: Option<&dyn LiveScorer>,
) -> Result<EvalOutcome, EvalError> {
    if frozen_digest.trim().is_empty() {
        return Err(EvalError::HoldoutSealed);
    }
    if holdout.is_empty() {
        return Err(EvalError::HoldoutSealed);
    }
    let scores = match backend {
        EvalBackend::Sim => sim_slice_scores(artifact_digest, holdout),
        EvalBackend::Lium => {
            if !pin.can_rent() {
                return Err(EvalError::EvalImageUnpinned);
            }
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer
                .score(pin, frozen_digest, artifact_digest, holdout)
                .await?
        }
    };
    let metrics = serde_json::to_vec(&serde_json::json!({
        "holdout_n": scores.holdout.len(),
        "agent_trace": scores.agent_trace,
    }))
    .unwrap_or_default();
    let receipt = EvalReceipt {
        provider: match backend {
            EvalBackend::Sim => "sim".into(),
            EvalBackend::Lium => "lium".into(),
        },
        pod_id: format!("sim-{}", &frozen_digest[..8.min(frozen_digest.len())]),
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
        holdout_items: holdout.len(),
    })
}

/// Teacher request: prompts only. Rejects miner-weight bodies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeacherJudgeRequest {
    /// Prompt / completion pair to judge.
    pub prompt: String,
    /// Candidate text.
    pub candidate: String,
    /// Must be the teacher model id, never a miner artifact.
    pub model: String,
}

/// Refuse any attempt to send miner weights through the teacher API.
pub fn teacher_judge_guard(req: &TeacherJudgeRequest, pin: &RelearnPin) -> Result<(), EvalError> {
    let model = req.model.trim();
    let looks_like_digest = model.len() == 64 && model.chars().all(|c| c.is_ascii_hexdigit());
    if looks_like_digest {
        return Err(EvalError::TeacherMinerWeights);
    }
    if model != pin.teacher_model && !is_configured_teacher_model(model) {
        return Err(EvalError::TeacherMinerWeights);
    }
    let lower = req.candidate.to_ascii_lowercase();
    if lower.contains("safetensors") || lower.contains("gguf") || lower.contains("nvfp4") {
        return Err(EvalError::TeacherMinerWeights);
    }
    Ok(())
}

/// Resolve the v0 teacher backend. HTTP API is the default. Sim when
/// `RELEARN_FORCE_SIM` is set or `RELEARN_TEACHER_BACKEND=sim`. Lium NVFP4
/// only when the operator sets `RELEARN_TEACHER_BACKEND=lium`.
/// Miner weights are never the served model.
#[must_use]
pub fn resolve_teacher_backend() -> TeacherBackend {
    if force_sim() {
        return default_teacher_backend(true);
    }
    TeacherBackend::from_env()
}

/// Rent a digest-pinned eval pod, exec, harvest, terminate.
///
/// `api_key` is used only to construct the backend the caller already built.
/// This function never logs it. Live rent is skipped when `pin.can_rent()` is
/// false (no published eval digest yet).
pub async fn rent_eval(
    backend: &dyn EvalJobBackend,
    pin: &RelearnPin,
    frozen_digest: &str,
    artifact_digest: &str,
) -> Result<(RemoteExecResult, String), EvalError> {
    if !pin.can_rent() {
        return Err(EvalError::EvalImageUnpinned);
    }
    let spec = InstanceSpec {
        name: format!("relearn-{}", &frozen_digest[..12.min(frozen_digest.len())]),
        max_lifetime_hours: 1.0,
        max_price_per_hour: 8.0,
        gpu_count: 1,
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
pub async fn sim_rent_roundtrip(digest: &str) -> Result<String, EvalError> {
    let backend = SimLiumBackend::new();
    let pin = RelearnPin {
        eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
        ..RelearnPin::default()
    };
    let (_r, id) = rent_eval(&backend, &pin, digest, digest).await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recs(n: u32) -> Vec<HoldoutItem> {
        (1..=n)
            .map(|id| {
                let vision = id % 5;
                let (task, image_hash) = match vision {
                    1 => (HoldoutTask::Captioning, format!("{id:064x}")),
                    2 => (HoldoutTask::Vqa, format!("{:064x}", id + 1000)),
                    3 => (HoldoutTask::Ocr, format!("{:064x}", id + 2000)),
                    4 => (HoldoutTask::Spatial, format!("{:064x}", id + 3000)),
                    _ => (HoldoutTask::Text, String::new()),
                };
                HoldoutItem {
                    id: 800 + id,
                    prompt: format!("holdout item {id} with enough words for a trigram"),
                    dataset_id: "dev".into(),
                    task,
                    image_hash,
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn scoring_happens_only_after_freeze_and_unseal() {
        let hold = recs(120);
        let pin = RelearnPin::default();
        assert!(
            eval_after_freeze(&pin, "", "art", &hold, EvalBackend::Sim, None)
                .await
                .is_err()
        );
        assert!(
            eval_after_freeze(&pin, "digest-a", "art", &[], EvalBackend::Sim, None)
                .await
                .is_err()
        );
        let out = eval_after_freeze(&pin, "digest-a", "art", &hold, EvalBackend::Sim, None)
            .await
            .expect("eval");
        assert_eq!(out.receipt.submission_hash, "digest-a");
        assert_eq!(out.backend, EvalBackend::Sim);
        assert_eq!(out.holdout_items, 120);
        assert!(out.scores.holdout.len() >= 100);
        assert!(!out.scores.general_canary.is_empty());
        assert_eq!(out.scores.vision_shuffle.len(), 4);
    }

    #[tokio::test]
    async fn live_eval_without_a_digest_pin_refuses_instead_of_simming() {
        let hold = recs(120);
        let err = eval_after_freeze(
            &RelearnPin::default(),
            "digest-a",
            "art",
            &hold,
            EvalBackend::Lium,
            None,
        )
        .await
        .expect_err("live eval must refuse without a digest pin");
        assert!(matches!(err, EvalError::EvalImageUnpinned), "{err}");
        assert!(err.to_string().contains("eval image digest not pinned"));
    }

    #[tokio::test]
    async fn live_eval_with_a_digest_pin_never_falls_back_to_sim() {
        let pin = RelearnPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..RelearnPin::default()
        };
        let err = eval_after_freeze(&pin, "digest-a", "art", &recs(120), EvalBackend::Lium, None)
            .await
            .expect_err("live scores must come from the eval image");
        assert!(matches!(err, EvalError::LiveHarvestUnavailable), "{err}");
    }

    #[tokio::test]
    async fn sim_receipt_declares_the_sim_provider() {
        let out = eval_after_freeze(
            &RelearnPin::default(),
            "digest-a",
            "art",
            &recs(120),
            EvalBackend::Sim,
            None,
        )
        .await
        .expect("sim eval");
        assert_eq!(out.receipt.provider, "sim");
        assert!(out.receipt.image_digest.is_empty());
    }

    fn live_pin() -> RelearnPin {
        RelearnPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            holdout_commitment: "aa".repeat(32),
            ..RelearnPin::default()
        }
    }

    fn measurement(pin: &RelearnPin, hold: &[HoldoutItem]) -> BaselineMeasurement {
        let s = sim_slice_scores_at_skill(BASE_CHAMPION_ARTIFACT, hold, BASE_CHAMPION_SKILL);
        BaselineMeasurement {
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.holdout_commitment.clone(),
            holdout: s.holdout.by_cluster,
            public: s.public.by_cluster,
            perturbed: s.perturbed.by_cluster,
            canaries: s.canaries.by_cluster,
            general_canary: s.general_canary.by_cluster,
            agent_trace: s.agent_trace,
            vision_shuffle: s.vision_shuffle,
        }
    }

    struct Harvest;

    #[async_trait]
    impl LiveScorer for Harvest {
        async fn score(
            &self,
            _pin: &RelearnPin,
            _frozen: &str,
            artifact: &str,
            holdout: &[HoldoutItem],
        ) -> Result<SliceScores, EvalError> {
            Ok(sim_slice_scores_at_skill(artifact, holdout, 0.5))
        }
    }

    struct Unready;

    #[async_trait]
    impl LiveScorer for Unready {
        async fn score(
            &self,
            _pin: &RelearnPin,
            _frozen: &str,
            _artifact: &str,
            _holdout: &[HoldoutItem],
        ) -> Result<SliceScores, EvalError> {
            Err(EvalError::Backend("must not score".into()))
        }
        fn ready(&self) -> Result<(), EvalError> {
            Err(EvalError::Backend(
                "RELEARN_TEACHER_API_URL not set on this host".into(),
            ))
        }
    }

    #[tokio::test]
    async fn live_boot_records_a_baseline_from_either_live_source() {
        let hold = recs(120);
        let pin = live_pin();
        let recorded = boot_base_champion(
            &pin,
            &hold,
            EvalBackend::Lium,
            Some(measurement(&pin, &hold)),
            None,
        )
        .await
        .expect("recorded baseline");
        assert_eq!(recorded.holdout.len(), 120);
        let harvested = boot_base_champion(&pin, &hold, EvalBackend::Lium, None, Some(&Harvest))
            .await
            .expect("harvested baseline");
        assert_eq!(harvested.holdout.len(), 120);
    }

    #[tokio::test]
    async fn live_boot_never_falls_back_to_the_sim_baseline() {
        let hold = recs(120);
        let pin = live_pin();
        assert!(matches!(
            boot_base_champion(&pin, &hold, EvalBackend::Lium, None, None).await,
            Err(EvalError::LiveHarvestUnavailable)
        ));
        assert!(matches!(
            boot_base_champion(&RelearnPin::default(), &hold, EvalBackend::Lium, None, None).await,
            Err(EvalError::EvalImageUnpinned)
        ));
        // Sim hosts still get the sim baseline, and it is not the live one.
        let sim = boot_base_champion(&pin, &hold, EvalBackend::Sim, None, None)
            .await
            .expect("sim");
        assert_eq!(sim.holdout, base_champion_scores(&hold).holdout);
    }

    #[test]
    fn recorded_baseline_is_bound_to_the_pinned_image_and_holdout() {
        let hold = recs(120);
        let pin = live_pin();
        measurement(&pin, &hold).verify(&pin, &hold).expect("clean");

        let mut other_image = measurement(&pin, &hold);
        other_image.eval_image_digest = format!("sha256:{}", "cd".repeat(32));
        assert!(matches!(
            other_image.verify(&pin, &hold),
            Err(EvalError::Baseline(_))
        ));

        let mut other_holdout = measurement(&pin, &hold);
        other_holdout.holdout_commitment = "bb".repeat(32);
        assert!(matches!(
            other_holdout.verify(&pin, &hold),
            Err(EvalError::Baseline(_))
        ));

        let mut short = measurement(&pin, &hold);
        short.holdout.pop_last();
        assert!(matches!(
            short.verify(&pin, &hold),
            Err(EvalError::Baseline(_))
        ));
    }

    #[test]
    fn recorded_baseline_must_carry_the_series_the_gates_read() {
        let hold = recs(120);
        let pin = live_pin();
        for break_it in [0_u8, 1, 2, 3, 4] {
            let mut m = measurement(&pin, &hold);
            match break_it {
                0 => m.general_canary.clear(),
                1 => m.public.clear(),
                2 => m.perturbed.clear(),
                3 => m.canaries.clear(),
                _ => m.agent_trace = 7.0,
            }
            assert!(
                matches!(m.verify(&pin, &hold), Err(EvalError::Baseline(_))),
                "case {break_it} must be refused at boot, not silently reject every challenger"
            );
        }
    }

    #[test]
    fn recorded_baseline_round_trips_through_json() {
        let hold = recs(120);
        let pin = live_pin();
        let body = serde_json::to_string(&measurement(&pin, &hold)).expect("json");
        let back = BaselineMeasurement::from_json(&body).expect("parse");
        back.verify(&pin, &hold).expect("verifies");
        assert!(BaselineMeasurement::from_json("not json").is_err());
    }

    #[test]
    fn readiness_names_the_root_cause() {
        let live = live_pin();
        assert!(scoring_readiness(&RelearnPin::default(), EvalBackend::Sim, None).is_ok());
        assert!(matches!(
            scoring_readiness(&RelearnPin::default(), EvalBackend::Lium, None),
            Err(EvalError::EvalImageUnpinned)
        ));
        assert!(matches!(
            scoring_readiness(&live, EvalBackend::Lium, None),
            Err(EvalError::LiveHarvestUnavailable)
        ));
        assert!(scoring_readiness(&live, EvalBackend::Lium, Some(&Harvest)).is_ok());
        let err = scoring_readiness(&live, EvalBackend::Lium, Some(&Unready))
            .expect_err("unready scorer");
        assert!(err.to_string().contains("RELEARN_TEACHER_API_URL"), "{err}");
    }

    #[tokio::test]
    async fn live_eval_uses_the_wired_harvest_not_the_sim_harness() {
        let hold = recs(120);
        let pin = live_pin();
        let out = eval_after_freeze(
            &pin,
            "digest-a",
            "art",
            &hold,
            EvalBackend::Lium,
            Some(&Harvest),
        )
        .await
        .expect("live eval");
        assert_eq!(out.backend, EvalBackend::Lium);
        assert_eq!(out.receipt.provider, "lium");
        assert_eq!(out.receipt.image_digest, pin.eval_image_digest);
        // The harvest pinned skill 0.5; the sim harness would have used the
        // digest-derived skill, so these must differ.
        assert_ne!(out.scores.holdout, sim_slice_scores("art", &hold).holdout);
    }

    #[test]
    fn sim_is_opt_in_only() {
        // Nothing in this process sets RELEARN_FORCE_SIM, so a default host
        // resolves the live backend.
        assert!(!force_sim());
        assert_eq!(resolve_eval_backend(), EvalBackend::Lium);
    }

    #[test]
    fn teacher_guard_rejects_miner_weight_payload() {
        let pin = RelearnPin::default();
        let bad = TeacherJudgeRequest {
            prompt: "score".into(),
            candidate: "here is a safetensors blob".into(),
            model: TEACHER_MODEL_ID.into(),
        };
        assert!(teacher_judge_guard(&bad, &pin).is_err());
        let good = TeacherJudgeRequest {
            prompt: "score".into(),
            candidate: "the capital is paris".into(),
            model: TEACHER_MODEL_ID.into(),
        };
        assert!(teacher_judge_guard(&good, &pin).is_ok());
        let glm = TeacherJudgeRequest {
            prompt: "score".into(),
            candidate: "ok".into(),
            model: relearn_challenge_task::TEACHER_GLM_MODEL_ID.into(),
        };
        assert!(teacher_judge_guard(&glm, &pin).is_ok());
        let digest = TeacherJudgeRequest {
            prompt: "score".into(),
            candidate: "ok".into(),
            model: "ab".repeat(32),
        };
        assert!(teacher_judge_guard(&digest, &pin).is_err());
    }

    #[test]
    fn pin_refuses_rent_without_digest() {
        assert!(!RelearnPin::default().can_rent());
        let p = RelearnPin {
            eval_image_digest: format!("sha256:{}", "00".repeat(32)),
            ..RelearnPin::default()
        };
        assert!(p.can_rent());
    }

    #[tokio::test]
    async fn sim_rent_tears_down() {
        let id = sim_rent_roundtrip("abcdef0123456789").await.expect("rent");
        assert!(id.contains("sim-pod"));
    }

    #[test]
    fn toml_pin_roundtrip() {
        let body = r#"
base_model = "Qwen/Qwen3.8-27B"
teacher_model = "glm-5.3"
teacher_backend = "http_api"
holdout_commitment = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
holdout_size = 120
public_ids = [1, 2, 3]
"#;
        let p = RelearnPin::from_toml(body).expect("parse");
        assert_eq!(p.base_model, BASE_MODEL_ID);
        assert_eq!(p.teacher_model, TEACHER_MODEL_ID);
        assert_eq!(p.teacher_backend, TeacherBackend::HttpApi);
        assert_eq!(p.holdout_size, 120);
        assert_eq!(p.public_ids, vec![1, 2, 3]);
        p.validate().expect("validates");
    }

    #[test]
    fn pin_without_holdout_commitment_fails_validate() {
        let p = RelearnPin::from_toml("base_model = \"Qwen/Qwen3.8-27B\"\n").expect("parse");
        assert!(matches!(p.validate(), Err(PinError::BadHoldoutCommitment)));
    }

    #[test]
    fn sim_harness_can_express_a_win_and_a_loss() {
        let hold = recs(120);
        let champ = base_champion_scores(&hold);
        let strong = sim_slice_scores_at_skill("art", &hold, BASE_CHAMPION_SKILL + 0.35);
        let weak = sim_slice_scores_at_skill("art", &hold, BASE_CHAMPION_SKILL - 0.35);

        let mut win = strong;
        win.contamination = relearn_score::ContaminationEvidence {
            declared_dataset_ids: 1,
            ..relearn_score::ContaminationEvidence::default()
        };
        let v = relearn_score::judge_challenger(&champ, &win);
        assert!(v.eligible, "failed={:?}", v.failed);
        assert!(v.lattice > 0);

        let mut lose = weak;
        lose.contamination = win.contamination.clone();
        let v = relearn_score::judge_challenger(&champ, &lose);
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn sim_retention_slices_agree_with_the_holdout_draw() {
        // Perturbation and public-gap gates are about the *same* model, so a
        // harness whose slices are drawn independently would fail them for
        // every submission and no sim run could ever reach awaiting_admin.
        let hold = recs(120);
        let s = sim_slice_scores_at_skill("art", &hold, 0.8);
        let h = SliceScores::mean(&s.holdout).expect("holdout mean");
        let p = SliceScores::mean(&s.perturbed).expect("perturbed mean");
        let pub_m = SliceScores::mean(&s.public).expect("public mean");
        assert!(h - p <= MAX_PERTURB_DROP, "perturb drop {}", h - p);
        assert!(h - p > 0.0, "perturbation must cost something");
        assert!(pub_m - h <= MAX_PUBLIC_PRIVATE_GAP, "gap {}", pub_m - h);
    }

    #[test]
    fn sim_skill_is_deterministic_per_digest() {
        assert!((sim_artifact_skill("art") - sim_artifact_skill("art")).abs() < f64::EPSILON);
        assert!((sim_artifact_skill("art") - sim_artifact_skill("other")).abs() > f64::EPSILON);
        let a = sim_slice_scores("art", &recs(120));
        let b = sim_slice_scores("art", &recs(120));
        assert_eq!(a.holdout, b.holdout);
    }

    #[test]
    fn sim_shuffle_covers_every_vision_family_in_the_holdout() {
        let scores = sim_slice_scores("art", &recs(120));
        for task in HoldoutTask::VISION {
            let ev = scores.vision_shuffle.get(&task).expect("family");
            assert!(ev.uses_the_image(), "{task:?}");
        }
        assert!(SliceScores::mean(&scores.general_canary).unwrap_or(0.0) > 0.9);
    }
}
