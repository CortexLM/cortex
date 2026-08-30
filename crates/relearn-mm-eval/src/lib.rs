//! Relearn Multimodal eval loop.
//!
//! Order matters: the text-intact rerun happens first, because it is the gate
//! that can zero the submission, and running it first means a damaged language
//! model is caught before any GPU time is spent on vision benchmarks.
//!
//! Backend resolution is fail-closed, the same way the T2I challenge does it. A
//! host with no eval endpoint and no explicit sim opt-in refuses to score
//! rather than producing a deterministic placeholder that reads like a pass.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;

use prism_competition::ExampleSeries;
use prism_lium::{EvalJobBackend, SimLiumBackend};
use prism_lium_types::{EvalReceipt, InstanceSpec, NoScoreGate, RemoteExecResult};
use relearn_mm_score::{AgenticEvidence, MmSliceScores, MIN_SHUFFLE_DROP};
use relearn_mm_store::EncoderManifest;
use relearn_mm_task::{PinError, RelearnMmPin, VisionTask};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Where the multimodal eval runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalBackend {
    /// Digest-pinned eval image on a Lium pod (production default).
    Lium,
    /// Deterministic offline eval (CI / local only).
    Sim,
}

/// True when the operator explicitly opted into sim.
#[must_use]
pub fn force_sim() -> bool {
    matches!(
        std::env::var("RELEARN_MM_FORCE_SIM")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

/// Resolve the eval backend for this host.
#[must_use]
pub fn resolve_backend() -> EvalBackend {
    if force_sim() {
        EvalBackend::Sim
    } else {
        EvalBackend::Lium
    }
}

/// Eval errors.
#[derive(Debug, Error)]
pub enum EvalError {
    /// The submission digest was not frozen before scoring.
    #[error("submission digest not frozen")]
    NotFrozen,
    /// Integrity gate failed.
    #[error("integrity: {0}")]
    Integrity(String),
    /// Lium / backend failure.
    #[error("backend: {0}")]
    Backend(String),
    /// The encoder license is not permissive.
    #[error("encoder attestation: {0}")]
    Attestation(#[from] PinError),
    /// The pinned eval image has no digest and sim was not opted into.
    #[error("eval image digest not pinned; set RELEARN_MM_FORCE_SIM=1 for CI")]
    EvalImageUnpinned,
}

/// One finished eval.
#[derive(Debug, Clone)]
pub struct EvalOutcome {
    /// Challenger measurements.
    pub scores: MmSliceScores,
    /// Integrity receipt.
    pub receipt: EvalReceipt,
    /// Backend that produced the scores.
    pub backend: EvalBackend,
    /// Text holdout items rerun for gate 1.
    pub text_items: usize,
    /// Vision holdout items scored for gate 2.
    pub vision_items: usize,
}

fn unit(parts: &[&str], index: usize) -> f64 {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0xff]);
    }
    h.update(u32::try_from(index).unwrap_or(0).to_le_bytes());
    let d = h.finalize();
    f64::from(d[0]) / 255.0
}

fn sim_series(prefix: &str, salt: &[&str], n: usize, base: f64) -> ExampleSeries {
    ExampleSeries::from_pairs((0..n).map(|i| {
        (
            format!("{prefix}{i}"),
            (base + 0.25 * unit(salt, i)).clamp(0.0, 1.0),
        )
    }))
}

/// Deterministic sim measurements for a frozen digest.
///
/// `text_base` is separate from `vision_base` on purpose: the sim harness has
/// to be able to produce a submission that wins on vision while regressing the
/// text side, because that is the case gate 1 exists to reject.
#[must_use]
pub fn sim_slice_scores(
    pin: &RelearnMmPin,
    artifact_digest: &str,
    manifest: &EncoderManifest,
    text_base: f64,
    vision_base: f64,
) -> MmSliceScores {
    let salt = [artifact_digest, "mm"];
    let mut vision_by_task = BTreeMap::new();
    for task in VisionTask::ALL {
        vision_by_task.insert(
            task,
            sim_series(
                task.as_str(),
                &[artifact_digest, task.as_str()],
                pin.vision_items_per_task,
                vision_base,
            ),
        );
    }
    let pooled: Vec<(String, f64)> = vision_by_task
        .iter()
        .flat_map(|(task, series)| {
            series
                .by_cluster
                .iter()
                .map(move |(k, v)| (format!("{}/{k}", task.as_str()), *v))
        })
        .collect();

    let agentic_series = sim_series(
        "agentic",
        &[artifact_digest, "agentic"],
        pin.agentic_traces.max(1),
        vision_base,
    );
    let agentic_mean = MmSliceScores::mean(&agentic_series).unwrap_or(0.0);

    MmSliceScores {
        text_holdout: sim_series("t", &salt, pin.text_holdout_items, text_base),
        vision_holdout: ExampleSeries::from_pairs(pooled),
        vision_by_task,
        agentic: AgenticEvidence {
            traces: u32::try_from(pin.agentic_traces).unwrap_or(u32::MAX),
            score: agentic_mean,
            // A model that reads the image loses most of the signal when the
            // pixels are destroyed; sim models that.
            shuffled_score: (agentic_mean - 2.0 * MIN_SHUFFLE_DROP).max(0.0),
        },
        agentic_series,
        vision_public: sim_series("vp", &[artifact_digest, "public"], 120, vision_base),
        lm_weights_hash: manifest.lm_weights_hash.trim().to_ascii_lowercase(),
        kind: manifest.kind,
    }
}

/// Baseline champion: pinned encoder on the champion LM, no miner training.
#[must_use]
pub fn base_champion_scores(pin: &RelearnMmPin, champion_lm_hash: &str) -> MmSliceScores {
    let manifest = EncoderManifest {
        encoder_model: pin.encoder_model.clone(),
        encoder_license: pin.encoder_license.clone(),
        lm_weights_hash: champion_lm_hash.to_owned(),
        ..EncoderManifest::default()
    };
    sim_slice_scores(pin, "relearn-mm-baseline", &manifest, 0.70, 0.45)
}

/// Run one eval after the submission digest is frozen.
///
/// Gate 1's text rerun is produced before the vision splits, so a submission
/// that damaged the language model is refused early.
///
/// # Errors
///
/// [`EvalError::NotFrozen`], [`EvalError::Attestation`] for a non-permissive
/// encoder license, and [`EvalError::EvalImageUnpinned`] when a live run was
/// requested without a digest-pinned eval image.
pub fn eval_after_freeze(
    pin: &RelearnMmPin,
    frozen_digest: &str,
    artifact_digest: &str,
    manifest: &EncoderManifest,
    backend: EvalBackend,
) -> Result<EvalOutcome, EvalError> {
    if frozen_digest.trim().is_empty() {
        return Err(EvalError::NotFrozen);
    }
    pin.attest_encoder(&manifest.encoder_model, &manifest.encoder_license)?;

    let scores = match backend {
        EvalBackend::Sim => sim_slice_scores(pin, artifact_digest, manifest, 0.72, 0.62),
        EvalBackend::Lium => {
            if !pin.can_rent() {
                return Err(EvalError::EvalImageUnpinned);
            }
            return Err(EvalError::Integrity(
                "live multimodal harvest is driven by the eval image; no in-process fallback"
                    .into(),
            ));
        }
    };

    let text_items = scores.text_holdout.len();
    let vision_items = scores.vision_holdout.len();
    let metrics = serde_json::to_vec(&serde_json::json!({
        "lm_base_model": pin.lm_base_model,
        "encoder_model": manifest.encoder_model,
        "text_items": text_items,
        "vision_items": vision_items,
        "shuffle_drop": scores.agentic.shuffle_drop(),
    }))
    .unwrap_or_default();
    let receipt = EvalReceipt {
        provider: match backend {
            EvalBackend::Sim => "sim".into(),
            EvalBackend::Lium => "lium".into(),
        },
        pod_id: format!("mm-{}", &frozen_digest[..8.min(frozen_digest.len())]),
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
        text_items,
        vision_items,
    })
}

/// Rent a digest-pinned eval pod, exec, harvest, terminate.
///
/// # Errors
///
/// [`EvalError::EvalImageUnpinned`] without a digest pin,
/// [`EvalError::Integrity`] on an unverified teardown, and
/// [`EvalError::Backend`] on any provider failure.
pub async fn rent_eval(
    backend: &dyn EvalJobBackend,
    pin: &RelearnMmPin,
    frozen_digest: &str,
    artifact_digest: &str,
) -> Result<(RemoteExecResult, String), EvalError> {
    if !pin.can_rent() {
        return Err(EvalError::EvalImageUnpinned);
    }
    let spec = InstanceSpec {
        name: format!(
            "relearn-mm-{}",
            &frozen_digest[..12.min(frozen_digest.len())]
        ),
        max_lifetime_hours: 2.0,
        max_price_per_hour: 16.0,
        gpu_count: 2,
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
    let lium = SimLiumBackend::new();
    let pin = RelearnMmPin {
        eval_image_digest: format!("sha256:{}", "cd".repeat(32)),
        ..RelearnMmPin::default()
    };
    let (_r, id) = rent_eval(&lium, &pin, digest, digest).await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use relearn_mm_task::SubmissionKind;

    use super::*;

    fn manifest() -> EncoderManifest {
        EncoderManifest {
            encoder_model: relearn_mm_task::ENCODER_MODEL_ID.into(),
            encoder_license: "apache-2.0".into(),
            projector: "2-layer MLP".into(),
            kind: SubmissionKind::EncoderOnly,
            lm_weights_hash: "aaaa1111".into(),
        }
    }

    #[test]
    fn sim_eval_needs_a_frozen_digest() {
        let pin = RelearnMmPin::default();
        assert!(matches!(
            eval_after_freeze(&pin, "", "art", &manifest(), EvalBackend::Sim),
            Err(EvalError::NotFrozen)
        ));
    }

    #[test]
    fn sim_eval_fills_both_gates_and_is_deterministic() {
        let pin = RelearnMmPin::default();
        let a = eval_after_freeze(&pin, "d", "art", &manifest(), EvalBackend::Sim).expect("a");
        let b = eval_after_freeze(&pin, "d", "art", &manifest(), EvalBackend::Sim).expect("b");
        assert_eq!(a.scores.text_holdout, b.scores.text_holdout);
        assert_eq!(a.text_items, pin.text_holdout_items);
        assert_eq!(a.vision_items, pin.vision_items_per_task * 4);
        assert_eq!(a.scores.vision_by_task.len(), 4);
        assert_eq!(a.receipt.provider, "sim");
        assert!(a.scores.agentic.uses_the_image());
    }

    #[test]
    fn non_permissive_encoder_is_refused_before_any_scoring() {
        let pin = RelearnMmPin::default();
        let mut m = manifest();
        m.encoder_license = "creativeml-openrail-m".into();
        let err = eval_after_freeze(&pin, "d", "art", &m, EvalBackend::Sim).expect_err("refuse");
        assert!(
            matches!(err, EvalError::Attestation(PinError::EncoderLicense(_))),
            "{err}"
        );
    }

    #[test]
    fn live_backend_without_a_pinned_image_refuses() {
        let pin = RelearnMmPin::default();
        assert!(!pin.can_rent());
        assert!(matches!(
            eval_after_freeze(&pin, "d", "art", &manifest(), EvalBackend::Lium),
            Err(EvalError::EvalImageUnpinned)
        ));
    }

    #[test]
    fn sim_harness_can_express_a_vision_win_with_a_text_regression() {
        let pin = RelearnMmPin::default();
        let damaged = sim_slice_scores(&pin, "art", &manifest(), 0.30, 0.90);
        let champion = base_champion_scores(&pin, "aaaa1111");
        assert!(
            MmSliceScores::mean(&damaged.text_holdout).unwrap_or(0.0)
                < MmSliceScores::mean(&champion.text_holdout).unwrap_or(1.0)
        );
        assert!(
            MmSliceScores::mean(&damaged.vision_holdout).unwrap_or(0.0)
                > MmSliceScores::mean(&champion.vision_holdout).unwrap_or(1.0)
        );
    }

    #[test]
    fn baseline_champion_carries_the_champion_lm_hash_canonicalized() {
        let pin = RelearnMmPin::default();
        let base = base_champion_scores(&pin, "AAAA1111");
        assert_eq!(
            base.lm_weights_hash, "aaaa1111",
            "hashes are canonicalized so a case difference is not a mismatch"
        );
        assert_eq!(base.text_holdout.len(), pin.text_holdout_items);
    }

    #[test]
    fn backend_defaults_to_lium_without_the_sim_opt_in() {
        assert_eq!(resolve_backend(), EvalBackend::Lium);
        assert!(!force_sim());
    }

    #[tokio::test]
    async fn sim_rent_tears_down() {
        let id = sim_rent_roundtrip("abcdef0123456789").await.expect("rent");
        assert!(id.contains("sim-pod"));
    }
}
