//! Relearn T2I eval loop: freeze digest → unseal prompts → generate → judge.
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

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};

use prism_competition::ExampleSeries;
use prism_lium::{EvalJobBackend, SimLiumBackend};
use prism_lium_types::{EvalReceipt, InstanceSpec, NoScoreGate, RemoteExecResult};
use relearn_t2i_judge::{assert_judge_model, ImageScore, JudgeError, JudgeInference};
use relearn_t2i_score::{
    contamination, FaithfulnessEvidence, ReplayEvidence, T2iSliceScores, MIN_FAITHFULNESS_CHECKS,
    REPLAY_CELLS,
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
    replay: ReplayEvidence,
    faithfulness: FaithfulnessEvidence,
    contaminated_prompt_ids: Vec<u32>,
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
        na_rate,
        replay,
        faithfulness,
        contaminated_prompt_ids,
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
    fold_scores(
        &holdout,
        &public,
        ReplayEvidence {
            cells_checked: REPLAY_CELLS,
            exact_hash_matches: REPLAY_CELLS,
            max_embedding_drift: 0.0,
        },
        FaithfulnessEvidence {
            checks: MIN_FAITHFULNESS_CHECKS,
            agreements: MIN_FAITHFULNESS_CHECKS,
        },
        Vec::new(),
    )
}

/// Fixed base-checkpoint champion (pinned Cosmos3, no miner fine-tune).
///
/// # Errors
///
/// See [`fold_scores`].
pub fn base_champion_scores(
    pin: &RelearnT2iPin,
    holdout_ids: &[u32],
) -> Result<T2iSliceScores, EvalError> {
    let holdout = sim_split(pin, holdout_ids, "cosmos3-super-text2image-base", 0.0);
    let public = sim_split(
        pin,
        &pin.prompts.public_ids,
        "cosmos3-super-text2image-base",
        0.0,
    );
    fold_scores(
        &holdout,
        &public,
        ReplayEvidence {
            cells_checked: REPLAY_CELLS,
            exact_hash_matches: REPLAY_CELLS,
            max_embedding_drift: 0.0,
        },
        FaithfulnessEvidence {
            checks: MIN_FAITHFULNESS_CHECKS,
            agreements: MIN_FAITHFULNESS_CHECKS,
        },
        Vec::new(),
    )
}

/// Bench prompt ids a submission admits to having trained on that are also
/// scored. Non-empty means the contamination gate rejects the submission.
#[must_use]
pub fn contaminated_ids(manifest: &ArtifactManifest, eval_ids: &[u32]) -> Vec<u32> {
    let train: BTreeSet<u32> = manifest.train_prompt_ids.iter().copied().collect();
    let eval: BTreeSet<u32> = eval_ids.iter().copied().collect();
    contamination(&train, &eval)
}

/// Run one eval after the submission digest is frozen.
///
/// # Errors
///
/// Refuses on a failed license attestation, an unconfigured judge, an empty
/// holdout, or an excessive judge N/A rate. Contamination is not an error here:
/// it is recorded on the scores so the verdict reports it as a gate failure.
pub fn eval_after_freeze(
    pin: &RelearnT2iPin,
    holdout: &[FrozenPrompt],
    frozen_digest: &str,
    artifact_digest: &str,
    manifest: &ArtifactManifest,
    judge: &JudgeConfig,
) -> Result<EvalOutcome, EvalError> {
    if frozen_digest.trim().is_empty() {
        return Err(EvalError::Holdout("submission digest not frozen".into()));
    }
    if holdout.is_empty() {
        return Err(EvalError::Holdout("holdout still sealed".into()));
    }
    pin.attest_artifact_base(&manifest.base, &manifest.base_license)?;

    judge.preflight(&JudgeInference::default())?;
    let backend = judge.backend;

    let holdout_ids: Vec<u32> = holdout.iter().map(|p| p.id).collect();
    let mut scores = match backend {
        JudgeBackend::Sim => sim_slice_scores(pin, &holdout_ids, artifact_digest)?,
        JudgeBackend::HttpApi | JudgeBackend::Lium => {
            // Live generation and judging happen inside the digest-pinned eval
            // image; this control-plane path refuses rather than inventing
            // scores when that image has not been pinned yet.
            if !pin.can_rent() {
                return Err(EvalError::Integrity(
                    "eval image digest not pinned; refuse live judge".into(),
                ));
            }
            return Err(EvalError::Integrity(
                "live Q-Judger harvest is driven by the eval image; no in-process fallback".into(),
            ));
        }
    };
    scores.contaminated_prompt_ids = contaminated_ids(manifest, &holdout_ids);

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
            ..ArtifactManifest::default()
        }
    }

    fn holdout() -> Vec<FrozenPrompt> {
        (900..=924).map(prompt).collect()
    }

    fn sim() -> JudgeConfig {
        JudgeConfig::sim()
    }

    #[test]
    fn sim_eval_needs_a_frozen_digest_and_an_unsealed_holdout() {
        let pin = test_pin();
        assert!(eval_after_freeze(&pin, &holdout(), "", "art", &manifest(), &sim()).is_err());
        assert!(eval_after_freeze(&pin, &[], "digest", "art", &manifest(), &sim()).is_err());
        let out = eval_after_freeze(&pin, &holdout(), "digest-a", "art", &manifest(), &sim())
            .expect("sim eval");
        assert_eq!(out.backend, JudgeBackend::Sim);
        assert_eq!(out.holdout_cells, 100);
        assert_eq!(out.receipt.provider, "sim");
        assert_eq!(out.receipt.submission_hash, "digest-a");
    }

    #[test]
    fn sim_eval_is_deterministic() {
        let pin = test_pin();
        let a = eval_after_freeze(&pin, &holdout(), "d", "art", &manifest(), &sim()).expect("a");
        let b = eval_after_freeze(&pin, &holdout(), "d", "art", &manifest(), &sim()).expect("b");
        assert_eq!(a.scores.holdout, b.scores.holdout);
    }

    #[test]
    fn flux_artifact_is_refused_before_any_scoring() {
        let pin = test_pin();
        let mut m = manifest();
        m.base = "black-forest-labs/FLUX.1-dev".into();
        let err = eval_after_freeze(&pin, &holdout(), "d", "art", &m, &sim()).expect_err("refuse");
        assert!(
            matches!(err, EvalError::Attestation(PinError::RejectedBase(_))),
            "{err}"
        );
    }

    #[test]
    fn wrong_license_attestation_is_refused() {
        let pin = test_pin();
        let mut m = manifest();
        m.base_license = "cc-by-nc-4.0".into();
        assert!(eval_after_freeze(&pin, &holdout(), "d", "art", &m, &sim()).is_err());
    }

    #[test]
    fn contaminated_training_metadata_lands_on_the_scores() {
        let pin = test_pin();
        let mut m = manifest();
        m.train_prompt_ids = vec![1, 2, 907];
        let out = eval_after_freeze(&pin, &holdout(), "d", "art", &m, &sim()).expect("eval");
        assert_eq!(out.scores.contaminated_prompt_ids, vec![907]);
    }

    #[test]
    fn live_backend_without_a_pinned_eval_image_refuses() {
        let pin = test_pin();
        let live = JudgeConfig {
            backend: JudgeBackend::HttpApi,
            endpoint: Some("http://judge.invalid/v1".into()),
        };
        let err = eval_after_freeze(&pin, &holdout(), "d", "art", &manifest(), &live)
            .expect_err("must refuse");
        assert!(matches!(err, EvalError::Integrity(_)), "{err}");
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
            ReplayEvidence::default(),
            FaithfulnessEvidence::default(),
            Vec::new(),
        )
        .expect_err("must refuse");
        assert!(matches!(err, EvalError::NotApplicableRate { .. }), "{err}");
    }

    #[test]
    fn empty_holdout_fold_is_refused() {
        assert!(fold_scores(
            &BTreeMap::new(),
            &BTreeMap::new(),
            ReplayEvidence::default(),
            FaithfulnessEvidence::default(),
            Vec::new(),
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
