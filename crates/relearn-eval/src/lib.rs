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

use prism_competition::ExampleSeries;
use prism_lium::{EvalJobBackend, SimLiumBackend};
use prism_lium_types::{EvalReceipt, InstanceSpec, NoScoreGate, RemoteExecResult};
use relearn_challenge_task::{
    default_teacher_backend, TeacherBackend, BASE_MODEL_ID, TEACHER_MODEL_ID, TEACHER_NVFP4_ID,
};
use relearn_score::SliceScores;
use relearn_store::{unseal_holdout, Holdout};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Pins Cortex stores for the split `CortexLM/relearn` repo + eval image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelearnPin {
    /// `Qwen/Qwen3.8-Flash-Next`.
    pub base_model: String,
    /// `zai-org/GLM-5.3`.
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
}

impl Default for RelearnPin {
    fn default() -> Self {
        Self {
            base_model: BASE_MODEL_ID.into(),
            teacher_model: TEACHER_MODEL_ID.into(),
            teacher_nvfp4: TEACHER_NVFP4_ID.into(),
            teacher_backend: TeacherBackend::Sim,
            eval_image: "ghcr.io/cortexlm/relearn-eval".into(),
            eval_image_digest: String::new(),
            relearn_git: relearn_challenge_task::RELEARN_GIT_URL.into(),
            relearn_git_sha: String::new(),
        }
    }
}

impl RelearnPin {
    /// Load from `config/relearn-pin.toml` (best-effort key=value / toml-ish).
    #[must_use]
    pub fn from_toml(body: &str) -> Self {
        let mut pin = Self::default();
        for raw in body.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim();
            let val = v.trim().trim_matches('"').to_owned();
            match key {
                "base_model" => pin.base_model = val,
                "teacher_model" => pin.teacher_model = val,
                "teacher_nvfp4" => pin.teacher_nvfp4 = val,
                "eval_image" => pin.eval_image = val,
                "eval_image_digest" => pin.eval_image_digest = val,
                "relearn_git" => pin.relearn_git = val,
                "relearn_git_sha" => pin.relearn_git_sha = val,
                "teacher_backend" => {
                    pin.teacher_backend = match val.as_str() {
                        "lium_nvfp4" => TeacherBackend::LiumNvfp4,
                        "http_api" => TeacherBackend::HttpApi,
                        _ => TeacherBackend::Sim,
                    };
                }
                _ => {}
            }
        }
        pin
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
    /// Holdout after unseal (seed visible only here).
    pub holdout: Holdout,
}

/// Deterministic sim scores from a frozen digest + holdout seed.
#[must_use]
pub fn sim_slice_scores(artifact_digest: &str, holdout_seed: &str) -> SliceScores {
    let holdout = series_from("h", artifact_digest, holdout_seed, 120, 0.15);
    let public = series_from("p", artifact_digest, holdout_seed, 120, 0.0);
    let perturbed = series_from(
        "x",
        artifact_digest,
        &format!("{holdout_seed}-p"),
        120,
        -0.02,
    );
    let canaries = series_from("c", "canary", holdout_seed, 40, 0.45);
    SliceScores {
        holdout,
        public,
        perturbed,
        canaries,
        agent_trace: 0.85,
    }
}

/// Fixed base-model champion (Qwen3.8-Flash-Next, no miner adapter).
#[must_use]
pub fn base_champion_scores() -> SliceScores {
    sim_slice_scores("base-qwen-3.8-flash-next", "base-seed")
}

fn series_from(prefix: &str, digest: &str, seed: &str, n: usize, bias: f64) -> ExampleSeries {
    let mut h = Sha256::new();
    h.update(digest.as_bytes());
    h.update([0xff]);
    h.update(seed.as_bytes());
    let root = h.finalize();
    let pairs = (0..n).map(|i| {
        let v = f64::from(root[i % 32]) / 255.0;
        let score = (0.45 + 0.4 * v + bias).clamp(0.0, 1.0);
        (format!("{prefix}{i}"), score)
    });
    ExampleSeries::from_pairs(pairs)
}

/// Unseal holdout only after `frozen_digest` is recorded, then score.
pub fn eval_after_freeze(
    pending: &Holdout,
    frozen_digest: &str,
    artifact_digest: &str,
) -> Result<EvalOutcome, EvalError> {
    let holdout = unseal_holdout(pending, frozen_digest).ok_or(EvalError::HoldoutSealed)?;
    if !holdout.unsealed {
        return Err(EvalError::HoldoutSealed);
    }
    let scores = sim_slice_scores(artifact_digest, &holdout.seed_hex);
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
        holdout,
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
    if req.model != pin.teacher_model && req.model != TEACHER_MODEL_ID {
        return Err(EvalError::TeacherMinerWeights);
    }
    let lower = req.candidate.to_ascii_lowercase();
    if lower.contains("safetensors") || lower.contains("gguf") || lower.contains("nvfp4") {
        return Err(EvalError::TeacherMinerWeights);
    }
    Ok(())
}

/// Resolve the v0 teacher backend. NVFP4-on-Lium is preferred when the
/// operator sets `RELEARN_TEACHER_BACKEND=lium` **and** can rent an 8×
/// Blackwell host; otherwise HTTP API (if `RELEARN_TEACHER_API_URL` is set)
/// or Sim. Miner weights are never the served model.
#[must_use]
pub fn resolve_teacher_backend() -> TeacherBackend {
    let env = TeacherBackend::from_env();
    if env == TeacherBackend::LiumNvfp4 {
        return TeacherBackend::LiumNvfp4;
    }
    let api = std::env::var("RELEARN_TEACHER_API_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    if env == TeacherBackend::HttpApi || api.is_some() {
        return default_teacher_backend(api.is_some());
    }
    TeacherBackend::Sim
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
    use relearn_store::sealed_holdout;

    #[test]
    fn unseal_happens_only_after_freeze() {
        let pending = sealed_holdout(1, "digest-a");
        assert!(eval_after_freeze(&pending, "", "art").is_err());
        let out = eval_after_freeze(&pending, "digest-a", "art").expect("eval");
        assert!(out.holdout.unsealed);
        assert_eq!(out.receipt.submission_hash, "digest-a");
        assert!(out.scores.holdout.len() >= 100);
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
teacher_model = "zai-org/GLM-5.3"
teacher_backend = "http_api"
"#;
        let p = RelearnPin::from_toml(body);
        assert_eq!(p.base_model, BASE_MODEL_ID);
        assert_eq!(p.teacher_backend, TeacherBackend::HttpApi);
    }
}
