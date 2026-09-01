//! Relearn Agent eval loop: freeze digest → unseal episodes → replay → judge.
//!
//! The control plane only ever boots a digest-pinned eval image, and the miner
//! pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). Nothing in this repo can run
//! an episode: the tool environment, the trace replay, and the ablation arms
//! all live inside `eval_image` from
//! [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
//!
//! Backend resolution is fail-closed. Without `RELEARN_AGENT_FORCE_SIM=1` a
//! host needs a `sha256:` eval-image pin **and** a wired harvest, and refuses
//! to score until it has both. The deterministic offline harness exists for CI
//! and local development, is reported on `/v1/status`, and is never a
//! fallback.
//!
//! A live host also needs a champion measured by the scorer submissions face
//! ([`boot_base_champion`]). Every gate is a comparison against the champion,
//! so judging a live challenger against simulated champion numbers would let
//! an artifact displace a champion nobody ever measured.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use prism_competition::ExampleSeries;
use prism_lium_types::{EvalReceipt, NoScoreGate};
use relearn_agent_score::{
    AblationEvidence, AgentSliceScores, ContaminationEvidence, MIN_ABLATION_DROP, MIN_SHUFFLE_DROP,
};
use relearn_agent_store::ArtifactManifest;
use relearn_agent_task::{contamination, AgentEpisode, RelearnAgentPin};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Where a Relearn Agent eval actually runs.
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
        std::env::var("RELEARN_AGENT_FORCE_SIM")
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
    /// Episodes were requested before the digest freeze, or none are loaded.
    #[error("episodes still sealed")]
    EpisodesSealed,
    /// Integrity gate failed.
    #[error("integrity: {0}")]
    Integrity(String),
    /// Provider / eval-image failure.
    #[error("backend: {0}")]
    Backend(String),
    /// A live run was asked for without a digest-pinned eval image.
    #[error(
        "eval image digest not pinned; refuse live scoring (RELEARN_AGENT_FORCE_SIM=1 is CI only)"
    )]
    EvalImageUnpinned,
    /// A live run reached the in-process scorer. It must not silently sim.
    #[error("live episode replay is driven by the digest-pinned eval image; no in-process sim")]
    LiveHarvestUnavailable,
    /// The operator-recorded champion baseline does not match the pin.
    #[error("recorded baseline: {0}")]
    Baseline(String),
}

/// Artifact id of the un-post-trained base model on the episode set.
pub const BASE_CHAMPION_ARTIFACT: &str = "base-relearn-agent-champion";

/// Run id bound into the boot baseline measurement.
pub const BASE_CHAMPION_RUN: &str = "boot-baseline";

/// Schema version of the metrics document the eval image emits.
pub const AGENT_METRICS_SCHEMA: u32 = 1;

/// Episode measurements produced by the digest-pinned eval image.
///
/// The implementation is not in this repo. The control plane holds a handle to
/// the eval image's harvest and never computes live numbers itself, so sim can
/// never arrive through this trait.
///
/// The same scorer measures the boot baseline and every challenger: a live
/// challenger compared against a champion the host never measured is not a
/// comparison, and a champion measured by a different scorer is not either.
#[async_trait]
pub trait LiveScorer: Send + Sync {
    /// Score one artifact on the verified episode set.
    ///
    /// `frozen_digest` binds the run. `artifact_digest` is the miner artifact,
    /// or [`BASE_CHAMPION_ARTIFACT`] for the boot baseline.
    ///
    /// # Errors
    ///
    /// Implementation-defined; surfaced to the miner as a 503.
    async fn score(
        &self,
        pin: &RelearnAgentPin,
        frozen_digest: &str,
        artifact_digest: &str,
        episodes: &[AgentEpisode],
    ) -> Result<AgentSliceScores, EvalError>;
}

/// Champion baseline measured by the digest-pinned eval image.
///
/// An operator records one by running the pinned image on the base checkpoint
/// once and installing the result (`RELEARN_AGENT_BASE_CHAMPION_FILE`);
/// [`Self::verify`] binds it to the pin so a measurement from another image or
/// another episode set is refused.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BaselineMeasurement {
    /// Eval image digest that produced these numbers. Must equal the pin's.
    pub eval_image_digest: String,
    /// Episode commitment measured against. Must equal the pin's.
    pub holdout_commitment: String,
    /// Per-episode task success on the holdout.
    pub holdout: BTreeMap<String, f64>,
    /// Per-episode task success on the published split.
    pub public: BTreeMap<String, f64>,
    /// Per-episode action score from the replay (`tool_call` on the wire).
    #[serde(alias = "tool_call")]
    pub trace_valid: BTreeMap<String, f64>,
    /// Shipped canary slice (`canaries` on the wire). Off the visible score.
    #[serde(alias = "canaries")]
    pub capability_canary: BTreeMap<String, f64>,
    /// Success with observations withheld (`tool_blind` on the wire).
    #[serde(alias = "tool_blind")]
    pub tool_ablation: AblationEvidence,
    /// Success with screenshot pixels shuffled. The image nests this under
    /// `{"image": …}`; [`Self::from_json`] unwraps that.
    pub observation_shuffle: AblationEvidence,
}

impl BaselineMeasurement {
    /// Parse an operator baseline file body.
    ///
    /// # Errors
    ///
    /// [`EvalError::Baseline`] when the body is not a baseline object.
    pub fn from_json(body: &str) -> Result<Self, EvalError> {
        let mut value: serde_json::Value =
            serde_json::from_str(body).map_err(|e| EvalError::Baseline(e.to_string()))?;
        unwrap_observation_shuffle(&mut value);
        serde_json::from_value(value).map_err(|e| EvalError::Baseline(e.to_string()))
    }

    /// Check the measurement against the pin before it becomes the champion.
    ///
    /// # Errors
    ///
    /// [`EvalError::Baseline`] on an image / commitment mismatch, or when a
    /// series the gates read is missing. A champion the gates cannot use would
    /// reject every challenger for a reason the miner cannot act on.
    pub fn verify(
        &self,
        pin: &RelearnAgentPin,
        episodes: &[AgentEpisode],
    ) -> Result<(), EvalError> {
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
            return bad("episode commitment does not match the pin".into());
        }
        if self.holdout.len() != episodes.len() {
            return bad(format!(
                "{} episode scores for {} verified episodes",
                self.holdout.len(),
                episodes.len()
            ));
        }
        if self.public.is_empty() {
            return bad("no public split; the memorization gap gate cannot run".into());
        }
        if self.capability_canary.is_empty() {
            return bad("no capability canary; every challenger would fail closed".into());
        }
        if self.trace_valid.is_empty() {
            return bad("no trace replay; the grounding gate cannot run".into());
        }
        Ok(())
    }

    /// Convert to champion slices. Contamination is challenger-side only.
    #[must_use]
    pub fn into_slice_scores(self) -> AgentSliceScores {
        let series = |m: BTreeMap<String, f64>| ExampleSeries::from_pairs(m);
        AgentSliceScores {
            holdout: series(self.holdout),
            public: series(self.public),
            trace_valid: series(self.trace_valid),
            capability_canary: series(self.capability_canary),
            tool_ablation: self.tool_ablation,
            observation_shuffle: self.observation_shuffle,
            contamination: ContaminationEvidence::default(),
        }
    }
}

/// The document `eval_image` must print for one scored artifact.
///
/// The baseline envelope plus the run identity, so an operator's baseline file
/// is literally this image's output for the base checkpoint — one format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvalMetrics {
    /// Must equal [`AGENT_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// Frozen submission digest the run was asked for.
    pub submission_digest: String,
    /// Artifact digest the run was asked for.
    pub artifact_digest: String,
    /// Measured series, image digest, and episode commitment.
    #[serde(flatten)]
    pub measurement: BaselineMeasurement,
}

impl AgentEvalMetrics {
    /// Parse a metrics document emitted by the eval image.
    ///
    /// # Errors
    ///
    /// [`EvalError::Baseline`] when the body is not a metrics object.
    pub fn from_json(body: &str) -> Result<Self, EvalError> {
        let mut value: serde_json::Value =
            serde_json::from_str(body).map_err(|e| EvalError::Baseline(e.to_string()))?;
        unwrap_observation_shuffle(&mut value);
        serde_json::from_value(value).map_err(|e| EvalError::Baseline(e.to_string()))
    }

    /// Bind the document to the run that was requested.
    ///
    /// Without this a pod could answer with another artifact's numbers, or
    /// replay an earlier run's.
    ///
    /// # Errors
    ///
    /// [`EvalError::Baseline`] on a schema, run-identity, image, commitment, or
    /// missing-series mismatch.
    pub fn verify(
        &self,
        pin: &RelearnAgentPin,
        frozen_digest: &str,
        artifact_digest: &str,
        episodes: &[AgentEpisode],
    ) -> Result<(), EvalError> {
        if self.schema_version != AGENT_METRICS_SCHEMA {
            return Err(EvalError::Baseline(format!(
                "metrics schema_version {}, expected {AGENT_METRICS_SCHEMA}",
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
        self.measurement.verify(pin, episodes)
    }
}

/// The published image nests shuffle evidence under `{"image": …}`.
fn unwrap_observation_shuffle(value: &mut serde_json::Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    if let Some(os) = obj.get_mut("observation_shuffle") {
        if let Some(image) = os.get("image").cloned() {
            *os = image;
        }
    }
}

/// Whether this host can produce a verdict at all, and why not if it cannot.
///
/// Checked before any episode is replayed so a live run never spends the
/// miner's Lium budget on a submission the host could never judge, and so the
/// miner is told the root cause (unloaded holdout, no digest pin) rather
/// than a symptom.
///
/// # Errors
///
/// [`EvalError::EpisodesSealed`] when the holdout is not verified loaded,
/// [`EvalError::EvalImageUnpinned`] without a `sha256:` eval-image digest, and
/// [`EvalError::LiveHarvestUnavailable`] when no [`LiveScorer`] is wired.
pub fn scoring_readiness(
    pin: &RelearnAgentPin,
    backend: EvalBackend,
    live: Option<&dyn LiveScorer>,
    holdout_loaded: bool,
) -> Result<(), EvalError> {
    // Root cause first: a host with no verified episodes cannot score, even
    // in sim. Status must not report `can_score` for that host.
    if !holdout_loaded {
        return Err(EvalError::EpisodesSealed);
    }
    match backend {
        EvalBackend::Sim => Ok(()),
        EvalBackend::Lium => {
            if !pin.can_rent() {
                return Err(EvalError::EvalImageUnpinned);
            }
            if live.is_none() {
                return Err(EvalError::LiveHarvestUnavailable);
            }
            Ok(())
        }
    }
}

/// One finished eval.
#[derive(Debug, Clone)]
pub struct EvalOutcome {
    /// Challenger measurements.
    pub scores: AgentSliceScores,
    /// Integrity receipt.
    pub receipt: EvalReceipt,
    /// Backend that produced the scores.
    pub backend: EvalBackend,
    /// Episode count that was scored (ids stay off the HTTP row).
    pub episodes: usize,
}

fn unit(parts: &[&str], index: u32) -> f64 {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0xff]);
    }
    h.update(index.to_le_bytes());
    f64::from(h.finalize()[0]) / 255.0
}

/// Per-episode noise band around the sim skill level.
const SIM_NOISE: f64 = 0.10;

/// How far sim skill moves the mean episode success.
const SIM_SKILL_SPAN: f64 = 0.30;

/// Sim skill of the base champion, in `[0, 1]`.
pub const BASE_CHAMPION_SKILL: f64 = 0.40;

/// Deterministic sim skill of an artifact, in `[0, 1]`.
#[must_use]
pub fn sim_artifact_skill(artifact_digest: &str) -> f64 {
    unit(&[artifact_digest, "skill"], 0)
}

fn skill_series(prefix: &str, ids: &[u32], digest: &str, salt: &str, skill: f64) -> ExampleSeries {
    ExampleSeries::from_pairs(ids.iter().map(|id| {
        let v = unit(&[digest, salt, &id.to_string()], *id);
        (
            format!("{prefix}{id}"),
            (0.30 + SIM_SKILL_SPAN * skill + SIM_NOISE * v).clamp(0.0, 1.0),
        )
    }))
}

/// A series that sits just under 1.0, for the arms the offline harness must
/// keep clear of their floors (trace validity, capability canary).
fn healthy_series(prefix: &str, ids: &[u32], digest: &str, salt: &str) -> ExampleSeries {
    ExampleSeries::from_pairs(ids.iter().map(|id| {
        let v = unit(&[digest, salt, &id.to_string()], *id);
        (format!("{prefix}{id}"), (0.90 + 0.09 * v).clamp(0.0, 1.0))
    }))
}

/// Deterministic sim scores from a frozen digest + verified episodes.
///
/// The ablation arms are derived from the holdout draw rather than sampled
/// independently: they are statements about the *same* model, so a harness
/// whose arms disagree with each other would fail every gate no matter what
/// the submission did.
#[must_use]
pub fn sim_slice_scores_at_skill(
    artifact_digest: &str,
    episodes: &[AgentEpisode],
    skill: f64,
) -> AgentSliceScores {
    let skill = skill.clamp(0.0, 1.0);
    let ids: Vec<u32> = episodes.iter().map(|e| e.id).collect();
    let public_ids: Vec<u32> = (1..=40).collect();
    let holdout = skill_series("e", &ids, artifact_digest, "hold", skill);
    let mean = AgentSliceScores::mean(&holdout).unwrap_or(0.0);
    let arm = |ablated: f64| AblationEvidence {
        episodes: u32::try_from(episodes.len()).unwrap_or(u32::MAX),
        score: mean,
        ablated_score: (mean - ablated).max(0.0),
    };
    AgentSliceScores {
        holdout,
        public: skill_series("p", &public_ids, artifact_digest, "public", skill),
        trace_valid: healthy_series("e", &ids, artifact_digest, "trace"),
        // Base competence is a property of the checkpoint, not of the
        // post-train, so the offline harness keeps the canary flat across
        // artifacts. A harness that jittered it would fail every submission
        // for a reason a miner cannot act on.
        capability_canary: healthy_series("c", &(0..40).collect::<Vec<_>>(), "canary", "offpath"),
        // Comfortably past the floors: a grounded agent loses most of its
        // score when the tools or the observation go away.
        tool_ablation: arm(2.0 * MIN_ABLATION_DROP),
        observation_shuffle: arm(2.0 * MIN_SHUFFLE_DROP),
        contamination: ContaminationEvidence::default(),
    }
}

/// Deterministic sim scores at the artifact's derived skill.
#[must_use]
pub fn sim_slice_scores(artifact_digest: &str, episodes: &[AgentEpisode]) -> AgentSliceScores {
    sim_slice_scores_at_skill(
        artifact_digest,
        episodes,
        sim_artifact_skill(artifact_digest),
    )
}

/// Baseline champion on the verified episodes (no miner post-train).
///
/// These are sim numbers. Only seed them on a host that resolved
/// [`EvalBackend::Sim`]; live hosts use [`boot_base_champion`].
#[must_use]
pub fn base_champion_scores(episodes: &[AgentEpisode]) -> AgentSliceScores {
    sim_slice_scores_at_skill(BASE_CHAMPION_ARTIFACT, episodes, BASE_CHAMPION_SKILL)
}

/// Champion baseline for this host, measured by the scorer submissions face.
///
/// # Errors
///
/// [`EvalError::EpisodesSealed`] with no verified episodes,
/// [`EvalError::EvalImageUnpinned`] without a digest pin,
/// [`EvalError::Baseline`] when a recorded measurement does not match the pin,
/// and [`EvalError::LiveHarvestUnavailable`] when a live host has neither
/// source.
pub async fn boot_base_champion(
    pin: &RelearnAgentPin,
    episodes: &[AgentEpisode],
    backend: EvalBackend,
    recorded: Option<BaselineMeasurement>,
    live: Option<&dyn LiveScorer>,
) -> Result<AgentSliceScores, EvalError> {
    if episodes.is_empty() {
        return Err(EvalError::EpisodesSealed);
    }
    match backend {
        EvalBackend::Sim => Ok(base_champion_scores(episodes)),
        EvalBackend::Lium => {
            if !pin.can_rent() {
                return Err(EvalError::EvalImageUnpinned);
            }
            if let Some(m) = recorded {
                m.verify(pin, episodes)?;
                return Ok(m.into_slice_scores());
            }
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer
                .score(pin, BASE_CHAMPION_RUN, BASE_CHAMPION_ARTIFACT, episodes)
                .await
        }
    }
}

/// Declared training metadata plus the holdout fingerprints inside it.
///
/// Returns the declared counts as well as the hits so a submission that
/// declared nothing cannot be read as a clean run.
#[must_use]
pub fn contamination_evidence(
    manifest: &ArtifactManifest,
    episodes: &[AgentEpisode],
) -> ContaminationEvidence {
    let ids: BTreeSet<u32> = manifest.train_episode_ids.iter().copied().collect();
    let observations: BTreeSet<String> = manifest
        .train_observation_hashes
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let environments: BTreeSet<String> = manifest
        .train_environment_ids
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    ContaminationEvidence {
        declared_episode_ids: ids.len(),
        declared_observation_hashes: observations.len(),
        declared_environment_ids: environments.len(),
        hits: contamination(&ids, &observations, episodes),
    }
}

/// Score only after the submission digest is frozen and episodes exist.
///
/// On [`EvalBackend::Lium`] this refuses rather than falling back: without a
/// `sha256:` eval-image pin the answer is [`EvalError::EvalImageUnpinned`],
/// and without a wired [`LiveScorer`] it is
/// [`EvalError::LiveHarvestUnavailable`]. Sim is never substituted.
///
/// # Errors
///
/// [`EvalError::EpisodesSealed`] before the freeze / without verified
/// episodes, the readiness errors above on a live host, and
/// [`EvalError::Integrity`] when the receipt gate fails.
pub async fn eval_after_freeze(
    pin: &RelearnAgentPin,
    frozen_digest: &str,
    artifact_digest: &str,
    episodes: &[AgentEpisode],
    backend: EvalBackend,
    live: Option<&dyn LiveScorer>,
) -> Result<EvalOutcome, EvalError> {
    if frozen_digest.trim().is_empty() || episodes.is_empty() {
        return Err(EvalError::EpisodesSealed);
    }
    scoring_readiness(pin, backend, live, !episodes.is_empty())?;
    let scores = match backend {
        EvalBackend::Sim => sim_slice_scores(artifact_digest, episodes),
        EvalBackend::Lium => {
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer
                .score(pin, frozen_digest, artifact_digest, episodes)
                .await?
        }
    };
    let metrics = serde_json::to_vec(&serde_json::json!({
        "episodes": scores.holdout.len(),
        "trace_valid": AgentSliceScores::mean(&scores.trace_valid),
        "tool_ablation_drop": scores.tool_ablation.drop(),
    }))
    .unwrap_or_default();
    let receipt = EvalReceipt {
        provider: match backend {
            EvalBackend::Sim => "sim".into(),
            EvalBackend::Lium => "lium".into(),
        },
        pod_id: format!("agent-{}", &frozen_digest[..8.min(frozen_digest.len())]),
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
        episodes: episodes.len(),
    })
}

#[cfg(test)]
mod tests {
    use relearn_agent_score::MIN_TRACE_VALIDITY;
    use relearn_agent_task::episode_commitment;

    use super::*;

    fn episodes(n: u32) -> Vec<AgentEpisode> {
        (1..=n)
            .map(|i| {
                AgentEpisode::synthetic(
                    800 + i,
                    format!("episode {i} asks for a figure buried in the ledger"),
                )
            })
            .collect()
    }

    fn live_pin(eps: &[AgentEpisode]) -> RelearnAgentPin {
        RelearnAgentPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            holdout_commitment: episode_commitment(eps),
            holdout_size: eps.len(),
            ..RelearnAgentPin::default()
        }
    }

    struct Harvest;

    #[async_trait]
    impl LiveScorer for Harvest {
        async fn score(
            &self,
            _pin: &RelearnAgentPin,
            _frozen: &str,
            artifact: &str,
            eps: &[AgentEpisode],
        ) -> Result<AgentSliceScores, EvalError> {
            Ok(sim_slice_scores_at_skill(artifact, eps, 0.5))
        }
    }

    fn measurement(pin: &RelearnAgentPin, eps: &[AgentEpisode]) -> BaselineMeasurement {
        let s = base_champion_scores(eps);
        BaselineMeasurement {
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.holdout_commitment.clone(),
            holdout: s.holdout.by_cluster,
            public: s.public.by_cluster,
            trace_valid: s.trace_valid.by_cluster,
            capability_canary: s.capability_canary.by_cluster,
            tool_ablation: s.tool_ablation,
            observation_shuffle: s.observation_shuffle,
        }
    }

    #[test]
    fn sim_is_opt_in_only() {
        assert!(!force_sim());
        assert_eq!(resolve_eval_backend(), EvalBackend::Lium);
    }

    #[tokio::test]
    async fn scoring_happens_only_after_freeze_and_unseal() {
        let eps = episodes(120);
        let pin = RelearnAgentPin::default();
        assert!(
            eval_after_freeze(&pin, "", "art", &eps, EvalBackend::Sim, None)
                .await
                .is_err()
        );
        assert!(
            eval_after_freeze(&pin, "digest-a", "art", &[], EvalBackend::Sim, None)
                .await
                .is_err()
        );
        let out = eval_after_freeze(&pin, "digest-a", "art", &eps, EvalBackend::Sim, None)
            .await
            .expect("sim eval");
        assert_eq!(out.backend, EvalBackend::Sim);
        assert_eq!(out.episodes, 120);
        assert_eq!(out.receipt.provider, "sim");
        assert!(out.scores.tool_ablation.shows_dependence(MIN_ABLATION_DROP));
        assert!(!out.scores.capability_canary.is_empty());
    }

    #[tokio::test]
    async fn a_live_host_refuses_rather_than_simming() {
        let eps = episodes(120);
        let unpinned = eval_after_freeze(
            &RelearnAgentPin::default(),
            "d",
            "art",
            &eps,
            EvalBackend::Lium,
            None,
        )
        .await
        .expect_err("no digest pin");
        assert!(
            matches!(unpinned, EvalError::EvalImageUnpinned),
            "{unpinned}"
        );

        let unwired = eval_after_freeze(&live_pin(&eps), "d", "art", &eps, EvalBackend::Lium, None)
            .await
            .expect_err("no harvest");
        assert!(
            matches!(unwired, EvalError::LiveHarvestUnavailable),
            "{unwired}"
        );
    }

    #[tokio::test]
    async fn a_live_host_uses_the_wired_harvest() {
        let eps = episodes(120);
        let pin = live_pin(&eps);
        let out = eval_after_freeze(&pin, "d", "art", &eps, EvalBackend::Lium, Some(&Harvest))
            .await
            .expect("live eval");
        assert_eq!(out.backend, EvalBackend::Lium);
        assert_eq!(out.receipt.provider, "lium");
        assert_eq!(out.receipt.image_digest, pin.eval_image_digest);
        // The harvest pinned skill 0.5; the sim harness would have used the
        // digest-derived skill, so these must differ.
        assert_ne!(out.scores.holdout, sim_slice_scores("art", &eps).holdout);
    }

    #[tokio::test]
    async fn live_boot_takes_either_live_source_and_never_the_sim_baseline() {
        let eps = episodes(120);
        let pin = live_pin(&eps);

        let from_file = boot_base_champion(
            &pin,
            &eps,
            EvalBackend::Lium,
            Some(measurement(&pin, &eps)),
            None,
        )
        .await
        .expect("recorded baseline");
        assert_eq!(from_file.holdout.len(), 120);

        let harvested = boot_base_champion(&pin, &eps, EvalBackend::Lium, None, Some(&Harvest))
            .await
            .expect("harvested baseline");
        assert_eq!(harvested.holdout.len(), 120);

        assert!(matches!(
            boot_base_champion(&pin, &eps, EvalBackend::Lium, None, None).await,
            Err(EvalError::LiveHarvestUnavailable)
        ));

        // Sim hosts still get the sim baseline, and it is not the live one.
        let sim = boot_base_champion(&pin, &eps, EvalBackend::Sim, None, None)
            .await
            .expect("sim");
        assert_eq!(sim.holdout, base_champion_scores(&eps).holdout);
    }

    #[test]
    fn a_recorded_baseline_is_bound_to_the_pin_and_must_carry_every_series() {
        let eps = episodes(120);
        let pin = live_pin(&eps);
        measurement(&pin, &eps).verify(&pin, &eps).expect("clean");

        let mut other_image = measurement(&pin, &eps);
        other_image.eval_image_digest = format!("sha256:{}", "cd".repeat(32));
        assert!(matches!(
            other_image.verify(&pin, &eps),
            Err(EvalError::Baseline(_))
        ));

        let mut other_set = measurement(&pin, &eps);
        other_set.holdout_commitment = "bb".repeat(32);
        assert!(matches!(
            other_set.verify(&pin, &eps),
            Err(EvalError::Baseline(_))
        ));

        for break_it in 0..3 {
            let mut m = measurement(&pin, &eps);
            match break_it {
                0 => m.capability_canary.clear(),
                1 => m.public.clear(),
                _ => m.trace_valid.clear(),
            }
            assert!(
                matches!(m.verify(&pin, &eps), Err(EvalError::Baseline(_))),
                "case {break_it} must be refused at boot"
            );
        }
    }

    #[test]
    fn readiness_names_the_root_cause() {
        let eps = episodes(120);
        let live = live_pin(&eps);
        scoring_readiness(&RelearnAgentPin::default(), EvalBackend::Sim, None, true).expect("sim");
        assert!(matches!(
            scoring_readiness(&live, EvalBackend::Sim, None, false),
            Err(EvalError::EpisodesSealed)
        ));
        assert!(matches!(
            scoring_readiness(&RelearnAgentPin::default(), EvalBackend::Lium, None, true),
            Err(EvalError::EvalImageUnpinned)
        ));
        assert!(matches!(
            scoring_readiness(&live, EvalBackend::Lium, None, true),
            Err(EvalError::LiveHarvestUnavailable)
        ));
        scoring_readiness(&live, EvalBackend::Lium, Some(&Harvest), true).expect("ready");
    }

    #[test]
    fn contamination_evidence_separates_undeclared_from_clean() {
        let eps = episodes(120);
        let bare = contamination_evidence(&ArtifactManifest::default(), &eps);
        assert!(!bare.is_declared());
        assert!(bare.hits.is_empty());

        let declared = contamination_evidence(
            &ArtifactManifest {
                train_environment_ids: vec!["cortex-public-envs-v0".into()],
                ..ArtifactManifest::default()
            },
            &eps,
        );
        assert!(declared.is_declared());
        assert!(declared.hits.is_empty());

        let dirty = contamination_evidence(
            &ArtifactManifest {
                train_episode_ids: vec![eps[0].id],
                ..ArtifactManifest::default()
            },
            &eps,
        );
        assert!(dirty.hits.iter().any(|h| h == &eps[0].fingerprint()));
    }

    /// The offline harness has to be able to express both verdicts, or no
    /// local run could ever reach `awaiting_admin`.
    #[test]
    fn the_sim_harness_can_express_a_win_and_a_loss() {
        let eps = episodes(120);
        let champ = base_champion_scores(&eps);
        let declared = ContaminationEvidence {
            declared_environment_ids: 1,
            ..ContaminationEvidence::default()
        };

        let mut strong = sim_slice_scores_at_skill("art", &eps, BASE_CHAMPION_SKILL + 0.35);
        strong.contamination = declared.clone();
        let win = relearn_agent_score::judge_challenger(&champ, &strong);
        assert!(win.eligible, "failed={:?}", win.failed);
        assert!(win.lattice > 0);

        let mut weak = sim_slice_scores_at_skill("art", &eps, BASE_CHAMPION_SKILL - 0.35);
        weak.contamination = declared;
        let lose = relearn_agent_score::judge_challenger(&champ, &weak);
        assert!(!lose.eligible);
        assert_eq!(lose.lattice, 0);
    }

    /// The harness must clear its own grounding floors, otherwise every local
    /// submission fails for a reason the miner cannot act on.
    #[test]
    fn the_sim_harness_is_internally_consistent() {
        let eps = episodes(120);
        let s = sim_slice_scores_at_skill("art", &eps, 0.8);
        assert!(AgentSliceScores::mean(&s.trace_valid).unwrap_or(0.0) >= MIN_TRACE_VALIDITY);
        assert!(s.tool_ablation.shows_dependence(MIN_ABLATION_DROP));
        assert!(s.observation_shuffle.shows_dependence(MIN_SHUFFLE_DROP));
    }
}
