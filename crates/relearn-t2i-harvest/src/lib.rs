//! Relearn Image live harvest: score an artifact on a digest-pinned eval image.
//!
//! This is the control-plane client for the [`LiveJudge`] seam. Neither the
//! Cosmos3 generation nor the Q-Judger pass happens here — both ship inside
//! `eval_image` from [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
//! This crate boots that image on a Lium pod, hands it the run request, reads
//! back the metrics document it printed, verifies the document against the pin
//! and the run identity, and tears the pod down.
//!
//! Nothing here computes a score. There is no sim fallback: a pod that does
//! not return a well-formed, correctly bound metrics document is an error, and
//! the submission answers 503.
//!
//! Image contract: `docs/RELEARN-IMAGE.md` § Eval image contract.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::doc_markdown)]

use std::sync::Arc;

use async_trait::async_trait;
use harvest_pod::{EvalPod, PodProgram};
use prism_lium_types::InstanceSpec;
use relearn_t2i_eval::{EvalError, LiveJudge, T2iEvalMetrics, T2I_METRICS_SCHEMA};
use relearn_t2i_score::T2iSliceScores;
use relearn_t2i_task::{FrozenPrompt, RelearnT2iPin, SeedCell};
use serde::{Deserialize, Serialize};

/// Prefix the eval image prints before its metrics document.
pub const METRICS_MARKER: &str = "RELEARN_IMAGE_METRICS=";

/// Marker the eval image prints on a completed run.
pub const OK_MARKER: &str = "RELEARN_IMAGE_EVAL_OK";

/// Directory the request and metrics sidecar live in, on the pod.
pub const POD_WORKDIR: &str = "/tmp/relearn_image_eval";

/// Lium SSH key name the harvest registers its public key under.
pub const SSH_KEY_NAME: &str = "relearn-image-eval-worker";

/// Image contract for the Relearn Image eval entrypoint.
pub const PROGRAM: PodProgram = PodProgram {
    workdir: POD_WORKDIR,
    entrypoint: "relearn-image-eval score",
    metrics_marker: METRICS_MARKER,
    ok_marker: OK_MARKER,
};

/// One scored `(prompt_id, variation_index)` cell, with the prompt verbatim.
///
/// The pod receives the frozen prompt string rather than an id it would have
/// to look up, so a stale bench snapshot on the image cannot silently change
/// what was scored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestCell {
    /// Bench prompt id.
    pub prompt_id: u32,
    /// Variation index within the prompt.
    pub variation_index: u32,
    /// Derived generation seed for this cell.
    pub seed: u64,
    /// `p{prompt_id}#v{variation_index}`.
    pub cell_key: String,
    /// Prompt string, replayed verbatim (never upsampled on the scored split).
    pub prompt: String,
}

/// What the eval image is asked to score.
///
/// This carries the **holdout prompts**, so the pod sees the private split for
/// the duration of the run. That is inherent to scoring on rented hardware: a
/// generator cannot be scored on prompts it is not shown. The mitigations are
/// the digest-pinned image, delivery into [`POD_WORKDIR`] rather than any
/// persisted path, the post-run scrub, and verified termination. Rotate the
/// holdout (salt + bench snapshot, then re-sign) if a pod is ever suspected of
/// exfiltration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestRequest {
    /// Must equal [`T2I_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// Frozen submission digest. Echoed back in the metrics document.
    pub submission_digest: String,
    /// Artifact to score. Echoed back in the metrics document.
    pub artifact_digest: String,
    /// Pinned generator checkpoint the artifact fine-tuned.
    pub base_model: String,
    /// Q-Judger id. The image must refuse any other judge.
    pub judge_model: String,
    /// Eval image digest, so the image can stamp its own provenance.
    pub eval_image_digest: String,
    /// Commitment the holdout prompts below must hash to.
    pub holdout_commitment: String,
    /// Frozen sampler configuration.
    pub sampler: relearn_t2i_task::SamplerConfig,
    /// Private split cells to score.
    pub holdout_cells: Vec<RequestCell>,
    /// Published split cells (informational, but measured on the same run so
    /// the public–holdout gap gate compares like with like).
    pub public_cells: Vec<RequestCell>,
}

/// Cost and shape guardrails for one harvest pod.
///
/// Cosmos3-Super is 65B at BF16 and Q-Judger is 27B, so this is a multi-GPU
/// node rather than the single card the text challenge uses.
#[derive(Debug, Clone, Copy)]
pub struct HarvestLimits {
    /// Hard ceiling on hourly price.
    pub max_price_per_hour: f64,
    /// GPUs per pod.
    pub gpu_count: u32,
    /// Pod lifetime ceiling; the provider reaps past this.
    pub max_lifetime_hours: f64,
}

impl Default for HarvestLimits {
    fn default() -> Self {
        Self {
            max_price_per_hour: 48.0,
            gpu_count: 8,
            max_lifetime_hours: 2.0,
        }
    }
}

/// [`LiveJudge`] over a digest-pinned eval image on a Lium pod.
pub struct LiumImageHarvest {
    pod: Arc<dyn EvalPod>,
    limits: HarvestLimits,
    /// Master's SSH public key(s). The pod is unreachable without one, so the
    /// request could not be delivered and no metrics could be read back.
    ssh_public_keys: Vec<String>,
}

impl LiumImageHarvest {
    /// Wrap a pod transport.
    #[must_use]
    pub fn new(pod: Arc<dyn EvalPod>, limits: HarvestLimits, ssh_public_keys: Vec<String>) -> Self {
        Self {
            pod,
            limits,
            ssh_public_keys,
        }
    }

    fn spec(&self, pin: &RelearnT2iPin, frozen_digest: &str) -> InstanceSpec {
        InstanceSpec {
            name: format!(
                "relearn-image-{}",
                &frozen_digest[..12.min(frozen_digest.len())]
            ),
            max_lifetime_hours: self.limits.max_lifetime_hours,
            max_price_per_hour: self.limits.max_price_per_hour,
            gpu_count: self.limits.gpu_count,
            image_digest: Some(pin.eval_image_digest.clone()),
            ssh_public_keys: self.ssh_public_keys.clone(),
            ssh_key_name: Some(SSH_KEY_NAME.to_owned()),
            preferred_offer_id: None,
            template_id: None,
            template_name: None,
        }
    }
}

/// Expand a split into request cells, prompts verbatim.
fn cells(pin: &RelearnT2iPin, prompts: &[FrozenPrompt]) -> Vec<RequestCell> {
    let ids: Vec<u32> = prompts.iter().map(|p| p.id).collect();
    pin.seed_cells(&ids)
        .into_iter()
        .filter_map(
            |SeedCell {
                 prompt_id,
                 variation_index,
                 seed,
             }| {
                let record = prompts.iter().find(|p| p.id == prompt_id)?;
                Some(RequestCell {
                    prompt_id,
                    variation_index,
                    seed,
                    cell_key: relearn_t2i_task::cell_key(prompt_id, variation_index),
                    prompt: record.generator_input().to_owned(),
                })
            },
        )
        .collect()
}

/// Pull the metrics document out of the image's stdout.
pub fn extract_metrics(stdout: &str) -> Result<T2iEvalMetrics, EvalError> {
    let body = PROGRAM.extract_document(stdout).ok_or_else(|| {
        EvalError::Backend(format!("eval image printed no {METRICS_MARKER} document"))
    })?;
    T2iEvalMetrics::from_json(body)
}

#[async_trait]
impl LiveJudge for LiumImageHarvest {
    async fn score(
        &self,
        pin: &RelearnT2iPin,
        frozen_digest: &str,
        artifact_digest: &str,
        holdout: &[FrozenPrompt],
    ) -> Result<T2iSliceScores, EvalError> {
        if !pin.can_rent() {
            return Err(EvalError::EvalImageUnpinned);
        }
        if holdout.is_empty() {
            return Err(EvalError::Holdout("holdout still sealed".into()));
        }
        if self.ssh_public_keys.iter().all(|k| k.trim().is_empty()) {
            return Err(EvalError::Backend(
                "no master SSH public key; the pod would be unreachable".into(),
            ));
        }
        let holdout_cells = cells(pin, holdout);
        let expected = holdout_cells.len();
        let request = HarvestRequest {
            schema_version: T2I_METRICS_SCHEMA,
            submission_digest: frozen_digest.to_owned(),
            artifact_digest: artifact_digest.to_owned(),
            base_model: pin.base.clone(),
            judge_model: pin.judge_model.clone(),
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.prompts.holdout_commitment.clone(),
            sampler: pin.sampler.clone(),
            holdout_cells,
            public_cells: cells(pin, &pin.frozen_prompts),
        };
        let body = serde_json::to_vec(&request)
            .map_err(|e| EvalError::Backend(format!("encode request: {e}")))?;

        let instance = self
            .pod
            .boot(&self.spec(pin, frozen_digest))
            .await
            .map_err(EvalError::Backend)?;
        let run = self.pod.run(&instance, &body).await;
        let shutdown = self.pod.shutdown(&instance).await;

        // Teardown outranks the run: an orphan pod keeps spending the miner's
        // money whatever the numbers said.
        match shutdown {
            Ok(true) => {}
            Ok(false) => {
                return Err(EvalError::Integrity(format!(
                    "pod {instance} terminate not verified"
                )))
            }
            Err(e) => return Err(EvalError::Backend(e)),
        }

        let stdout = run.map_err(EvalError::Backend)?;
        if !PROGRAM.ran_to_completion(&stdout) {
            tracing::warn!(
                instance,
                "eval image did not print {OK_MARKER}; refusing the run"
            );
            return Err(EvalError::Backend(format!(
                "eval image did not print {OK_MARKER}"
            )));
        }
        let metrics = extract_metrics(&stdout)?;
        metrics.verify(pin, frozen_digest, artifact_digest, expected)?;
        Ok(metrics.measurement.into_slice_scores())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use relearn_t2i_eval::T2iBaselineMeasurement;
    use relearn_t2i_task::{frozen_prompt_commitment, L1Dimension, PromptPin};

    use super::*;

    fn prompt(id: u32) -> FrozenPrompt {
        FrozenPrompt {
            id,
            text: format!("prompt {id}"),
            upsampled_json: None,
        }
    }

    fn holdout() -> Vec<FrozenPrompt> {
        (900..=924).map(prompt).collect()
    }

    fn pin() -> RelearnT2iPin {
        let public: Vec<FrozenPrompt> = (1..=25).map(prompt).collect();
        RelearnT2iPin {
            prompts: PromptPin {
                pin_salt: "cortex-image-test".into(),
                variations_per_prompt: 4,
                public_ids: public.iter().map(|p| p.id).collect(),
                holdout_commitment: frozen_prompt_commitment(&holdout()),
                holdout_size: 25,
            },
            frozen_prompts: public,
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..RelearnT2iPin::default()
        }
    }

    /// A metrics document exactly as the eval image is contracted to print it.
    /// Fixture data, not a harness: this crate must never produce a score.
    fn document(p: &RelearnT2iPin, frozen: &str, artifact: &str, level: f64) -> String {
        let flat = |prefix: &str, n: usize, v: f64| {
            (0..n)
                .map(|i| (format!("{prefix}{i}"), v))
                .collect::<BTreeMap<String, f64>>()
        };
        let hold: BTreeMap<String, f64> = cells(p, &holdout())
            .into_iter()
            .map(|c| (c.cell_key, level))
            .collect();
        let m = T2iEvalMetrics {
            schema_version: T2I_METRICS_SCHEMA,
            submission_digest: frozen.to_owned(),
            artifact_digest: artifact.to_owned(),
            measurement: T2iBaselineMeasurement {
                eval_image_digest: p.eval_image_digest.clone(),
                holdout_commitment: p.prompts.holdout_commitment.clone(),
                holdout: hold.clone(),
                public: flat("p", 40, level + 0.02),
                holdout_by_pillar: L1Dimension::ALL
                    .into_iter()
                    .map(|d| (d, hold.clone()))
                    .collect(),
                capability_canary: flat("cap#", 24, 0.90),
                na_rate: 0.05,
                replay: relearn_t2i_score::ReplayEvidence {
                    cells_checked: relearn_t2i_score::REPLAY_CELLS,
                    exact_hash_matches: relearn_t2i_score::REPLAY_CELLS,
                    max_embedding_drift: 0.0,
                },
                faithfulness: relearn_t2i_score::FaithfulnessEvidence {
                    checks: relearn_t2i_score::MIN_FAITHFULNESS_CHECKS,
                    agreements: relearn_t2i_score::MIN_FAITHFULNESS_CHECKS,
                },
            },
        };
        format!(
            "boot ok\n{METRICS_MARKER}{}\n{OK_MARKER}\n",
            serde_json::to_string(&m).unwrap_or_default()
        )
    }

    #[derive(Default)]
    struct Recorder {
        booted: Vec<InstanceSpec>,
        requests: Vec<HarvestRequest>,
        shutdowns: Vec<String>,
    }

    struct FakePod {
        stdout: Result<String, String>,
        verified: bool,
        log: Mutex<Recorder>,
    }

    impl FakePod {
        fn ok(stdout: String) -> Self {
            Self {
                stdout: Ok(stdout),
                verified: true,
                log: Mutex::new(Recorder::default()),
            }
        }

        fn log(&self) -> std::sync::MutexGuard<'_, Recorder> {
            self.log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    #[async_trait]
    impl EvalPod for FakePod {
        async fn boot(&self, spec: &InstanceSpec) -> Result<String, String> {
            self.log().booted.push(spec.clone());
            Ok("pod-1".into())
        }

        async fn run(&self, _instance_id: &str, request: &[u8]) -> Result<String, String> {
            let parsed: HarvestRequest = serde_json::from_slice(request).expect("request json");
            self.log().requests.push(parsed);
            self.stdout.clone()
        }

        async fn shutdown(&self, instance_id: &str) -> Result<bool, String> {
            self.log().shutdowns.push(instance_id.to_owned());
            Ok(self.verified)
        }
    }

    fn harvest(pod: Arc<FakePod>) -> LiumImageHarvest {
        LiumImageHarvest::new(
            pod,
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAmaster".into()],
        )
    }

    #[tokio::test]
    async fn boots_the_pinned_digest_and_returns_the_image_numbers() {
        let p = pin();
        let pod = Arc::new(FakePod::ok(document(&p, "frozen-1", "artifact-1", 0.61)));
        let scores = harvest(Arc::clone(&pod))
            .score(&p, "frozen-1", "artifact-1", &holdout())
            .await
            .expect("harvest");

        assert_eq!(scores.holdout.len(), 100);
        assert!((T2iSliceScores::mean(&scores.holdout).unwrap_or(0.0) - 0.61).abs() < 1e-9);
        assert!(!scores.capability_canary.is_empty());

        let log = pod.log();
        assert_eq!(
            log.booted[0].image_digest.as_deref(),
            Some(p.eval_image_digest.as_str())
        );
        assert_eq!(log.booted[0].ssh_key_name.as_deref(), Some(SSH_KEY_NAME));
        assert_eq!(log.requests[0].holdout_cells.len(), 100);
        assert_eq!(log.requests[0].artifact_digest, "artifact-1");
        assert_eq!(log.shutdowns, vec!["pod-1".to_owned()]);
    }

    /// Miners never bring an upsampler to the scored split, so the pod must be
    /// handed the frozen strings and the derived seeds, not ids to resolve.
    #[tokio::test]
    async fn the_request_carries_frozen_prompts_and_derived_seeds() {
        let p = pin();
        let pod = Arc::new(FakePod::ok(document(&p, "f", "a", 0.5)));
        harvest(Arc::clone(&pod))
            .score(&p, "f", "a", &holdout())
            .await
            .expect("harvest");
        let log = pod.log();
        let cell = &log.requests[0].holdout_cells[0];
        assert!(cell.seed > 0);
        assert!(cell.prompt.starts_with("prompt 9"));
        assert_eq!(
            cell.cell_key,
            relearn_t2i_task::cell_key(cell.prompt_id, cell.variation_index)
        );
        assert!(!log.requests[0].public_cells.is_empty());
    }

    #[tokio::test]
    async fn refuses_without_a_digest_pin_or_a_master_key_and_never_boots() {
        let p = pin();
        let unpinned = RelearnT2iPin {
            eval_image_digest: String::new(),
            ..p.clone()
        };
        let pod = Arc::new(FakePod::ok(String::new()));
        let err = harvest(Arc::clone(&pod))
            .score(&unpinned, "f", "a", &holdout())
            .await
            .expect_err("unpinned");
        assert!(matches!(err, EvalError::EvalImageUnpinned), "{err}");
        assert!(pod.log().booted.is_empty());

        let keyless = LiumImageHarvest::new(
            Arc::clone(&pod) as Arc<dyn EvalPod>,
            HarvestLimits::default(),
            Vec::new(),
        );
        assert!(keyless.score(&p, "f", "a", &holdout()).await.is_err());
        assert!(pod.log().booted.is_empty());
    }

    #[tokio::test]
    async fn always_tears_the_pod_down_and_refuses_an_orphan() {
        let p = pin();
        let failed = Arc::new(FakePod {
            stdout: Err("cuda oom".into()),
            verified: true,
            log: Mutex::new(Recorder::default()),
        });
        assert!(harvest(Arc::clone(&failed))
            .score(&p, "f", "a", &holdout())
            .await
            .is_err());
        assert_eq!(failed.log().shutdowns, vec!["pod-1".to_owned()]);

        let orphan = Arc::new(FakePod {
            stdout: Ok(document(&p, "f", "a", 0.6)),
            verified: false,
            log: Mutex::new(Recorder::default()),
        });
        let err = harvest(orphan)
            .score(&p, "f", "a", &holdout())
            .await
            .expect_err("orphan pod");
        assert!(matches!(err, EvalError::Integrity(_)), "{err}");
    }

    #[tokio::test]
    async fn a_document_for_another_run_is_refused() {
        let p = pin();
        for (frozen, artifact) in [("frozen-1", "someone-else"), ("an-earlier-run", "artifact-1")] {
            let pod = Arc::new(FakePod::ok(document(&p, frozen, artifact, 0.9)));
            let err = harvest(pod)
                .score(&p, "frozen-1", "artifact-1", &holdout())
                .await
                .expect_err("run identity mismatch");
            assert!(matches!(err, EvalError::Baseline(_)), "{err}");
        }
    }

    #[tokio::test]
    async fn silence_from_the_pod_is_never_a_score() {
        let p = pin();
        let full = document(&p, "f", "a", 0.6);
        for body in [
            String::new(),
            "boot ok\nsegfault\n".to_owned(),
            format!("{OK_MARKER}\n"),
            full.replace(OK_MARKER, ""),
        ] {
            let pod = Arc::new(FakePod::ok(body.clone()));
            assert!(
                harvest(pod).score(&p, "f", "a", &holdout()).await.is_err(),
                "body {body:?} must not score"
            );
        }
    }
}
