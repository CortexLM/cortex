//! Relearn eval loop: freeze digest → unseal holdout → rent/sim → harvest.
//!
//! Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). The control plane
//! only ever boots a digest-pinned eval image. Teacher HTTP is judge-only
//! and never serves miner weights as the scored artifact.

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

use prism_competition::ExampleSeries;
use prism_lium::{EvalJobBackend, SimLiumBackend};
use prism_lium_types::{EvalReceipt, InstanceSpec, NoScoreGate, RemoteExecResult};
use relearn_challenge_task::{
    default_teacher_backend, is_configured_teacher_model, HoldoutItem, HoldoutTask, TeacherBackend,
    BASE_MODEL_ID, MIN_HOLDOUT_ITEMS, TEACHER_MODEL_ID, TEACHER_NVFP4_ID,
};
use relearn_score::{ShuffleEvidence, SliceScores, MIN_SHUFFLE_DROP};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Pins Cortex stores for the split `CortexLM/relearn` repo + eval image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelearnPin {
    /// Language / VLM base. Do not recale here; the pin owner owns the id.
    pub base_model: String,
    /// HTTP teacher wire id (`kimi-k3` default; GLM optional override).
    pub teacher_model: String,
    /// Optional NVFP4 id for Lium serving.
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
}

/// One finished eval.
#[derive(Debug, Clone)]
pub struct EvalOutcome {
    /// Challenger measurements.
    pub scores: SliceScores,
    /// Integrity receipt.
    pub receipt: EvalReceipt,
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

/// Deterministic sim scores from a frozen digest + verified holdout records.
///
/// Public and general-canary series are produced from salts that do **not**
/// include the holdout prompts, so they cannot reconstruct the private split.
#[must_use]
pub fn sim_slice_scores(artifact_digest: &str, holdout: &[HoldoutItem]) -> SliceScores {
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
        holdout: series_ids("h", &hold_ids, artifact_digest, "hold", 0.15),
        public: series_ids("p", &public_ids, artifact_digest, "public", 0.0),
        perturbed: series_ids("x", &hold_ids, artifact_digest, "pert", -0.02),
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
        contaminated_fingerprints: Vec::new(),
    }
}

/// Baseline champion on the verified holdout (no miner adapter).
#[must_use]
pub fn base_champion_scores(holdout: &[HoldoutItem]) -> SliceScores {
    sim_slice_scores("base-relearn-champion", holdout)
}

/// Score only after the submission digest is frozen and holdout records exist.
pub fn eval_after_freeze(
    frozen_digest: &str,
    artifact_digest: &str,
    holdout: &[HoldoutItem],
) -> Result<EvalOutcome, EvalError> {
    if frozen_digest.trim().is_empty() {
        return Err(EvalError::HoldoutSealed);
    }
    if holdout.is_empty() {
        return Err(EvalError::HoldoutSealed);
    }
    let scores = sim_slice_scores(artifact_digest, holdout);
    let metrics = serde_json::to_vec(&serde_json::json!({
        "holdout_n": scores.holdout.len(),
        "agent_trace": scores.agent_trace,
    }))
    .unwrap_or_default();
    let receipt = EvalReceipt {
        provider: "sim".into(),
        pod_id: format!("sim-{}", &frozen_digest[..8.min(frozen_digest.len())]),
        image_digest: String::new(),
        submission_hash: frozen_digest.to_owned(),
        metrics_hash: EvalReceipt::hash_metrics_bytes(&metrics),
        termination_verified: true,
    };
    NoScoreGate::check(&receipt, false).map_err(|e| EvalError::Integrity(e.to_string()))?;
    Ok(EvalOutcome {
        scores,
        receipt,
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
    let force_sim = matches!(
        std::env::var("RELEARN_FORCE_SIM")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    );
    if force_sim {
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
        return Err(EvalError::Integrity(
            "eval image digest not pinned; refuse live rent".into(),
        ));
    }
    let spec = InstanceSpec {
        name: format!("relearn-{}", &frozen_digest[..12.min(frozen_digest.len())]),
        max_lifetime_hours: 1.0,
        max_price_per_hour: 8.0,
        gpu_count: 1,
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

    #[test]
    fn scoring_happens_only_after_freeze_and_unseal() {
        let hold = recs(120);
        assert!(eval_after_freeze("", "art", &hold).is_err());
        assert!(eval_after_freeze("digest-a", "art", &[]).is_err());
        let out = eval_after_freeze("digest-a", "art", &hold).expect("eval");
        assert_eq!(out.receipt.submission_hash, "digest-a");
        assert_eq!(out.holdout_items, 120);
        assert!(out.scores.holdout.len() >= 100);
        assert!(!out.scores.general_canary.is_empty());
        assert_eq!(out.scores.vision_shuffle.len(), 4);
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
base_model = "Qwen/Qwen3.8-Flash-Next"
teacher_model = "kimi-k3"
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
        let p = RelearnPin::from_toml("base_model = \"Qwen/Qwen3.8-Flash-Next\"\n").expect("parse");
        assert!(matches!(p.validate(), Err(PinError::BadHoldoutCommitment)));
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
