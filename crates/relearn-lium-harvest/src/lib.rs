//! Relearn live harvest: score an artifact on a digest-pinned eval image.
//!
//! This is the control-plane client for the [`LiveScorer`] seam. The scoring
//! code itself is not here — it ships inside `eval_image` from
//! [`CortexLM/relearn`](https://github.com/CortexLM/relearn). This crate boots
//! that image on a Lium pod, hands it the run request, reads back the metrics
//! document it printed, verifies the document against the pin and the run
//! identity, and tears the pod down.
//!
//! Nothing here computes a score. There is no sim fallback: a pod that does
//! not return a well-formed, correctly bound metrics document is an error, and
//! the submission answers 503.
//!
//! Image contract: `docs/RELEARN.md` § Eval image contract.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::doc_markdown)]

mod pod;

use std::sync::Arc;

use async_trait::async_trait;
use prism_lium_types::InstanceSpec;
use relearn_challenge_task::HoldoutItem;
use relearn_eval::{EvalError, LiveScorer, RelearnEvalMetrics, RelearnPin, RELEARN_METRICS_SCHEMA};
use relearn_score::SliceScores;
use serde::{Deserialize, Serialize};

pub use pod::LiumEvalPod;

/// Prefix the eval image prints before its metrics document.
///
/// Same shape as the Prism harness (`METRICS_JSON=`): the document is one
/// line and can be far larger than any log tail, so it is grepped by prefix
/// rather than scraped from the end of a log.
pub const METRICS_MARKER: &str = "RELEARN_METRICS=";

/// Marker the eval image prints on a completed run.
pub const OK_MARKER: &str = "RELEARN_EVAL_OK";

/// Directory the request and metrics sidecar live in, on the pod.
pub const POD_WORKDIR: &str = "/tmp/relearn_eval";

/// What the eval image is asked to score.
///
/// This carries the **holdout items**, so the pod sees the private split for
/// the duration of the run. That is inherent to scoring on rented hardware:
/// the model has to see the prompts. The mitigations are that the pod runs a
/// digest-pinned image, the request lands under [`POD_WORKDIR`] and is not
/// persisted, and the pod is terminated with verification after every run.
/// Rotate the holdout (salt + catalog, then re-sign) if a pod is ever
/// suspected of exfiltration — see `docs/RELEARN.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestRequest {
    /// Must equal [`RELEARN_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// Frozen submission digest. Echoed back in the metrics document.
    pub submission_digest: String,
    /// Artifact to score. Echoed back in the metrics document.
    pub artifact_digest: String,
    /// Base model the artifact post-trained.
    pub base_model: String,
    /// Teacher wire id for judge calls the image makes.
    pub teacher_model: String,
    /// Eval image digest, so the image can stamp its own provenance.
    pub eval_image_digest: String,
    /// Commitment the holdout below must hash to.
    pub holdout_commitment: String,
    /// The verified holdout records to score on.
    pub holdout: Vec<HoldoutItem>,
}

/// Cost and shape guardrails for one harvest pod.
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
            max_price_per_hour: 8.0,
            gpu_count: 1,
            max_lifetime_hours: 1.0,
        }
    }
}

/// One pod's lifecycle for one harvest.
///
/// Split from [`LiumHarvest`] so the lifecycle, teardown, and document
/// verification are testable without a Lium account. [`LiumEvalPod`] is the
/// real transport.
#[async_trait]
pub trait EvalPod: Send + Sync {
    /// Boot the digest-pinned image and return the instance id.
    async fn boot(&self, spec: &InstanceSpec) -> Result<String, EvalError>;

    /// Deliver `request`, run the image, return its stdout.
    async fn run(&self, instance_id: &str, request: &HarvestRequest) -> Result<String, EvalError>;

    /// Terminate. `Ok(true)` only when the provider confirms the pod is gone.
    async fn shutdown(&self, instance_id: &str) -> Result<bool, EvalError>;
}

/// Lium SSH key name the harvest registers its public key under.
pub const SSH_KEY_NAME: &str = "relearn-eval-worker";

/// [`LiveScorer`] over a digest-pinned eval image on a Lium pod.
pub struct LiumHarvest {
    pod: Arc<dyn EvalPod>,
    limits: HarvestLimits,
    /// Master's SSH public key(s). The pod is unreachable without one, so the
    /// request could not be delivered and no metrics could be read back.
    ssh_public_keys: Vec<String>,
}

impl LiumHarvest {
    /// Wrap a pod transport.
    #[must_use]
    pub fn new(pod: Arc<dyn EvalPod>, limits: HarvestLimits, ssh_public_keys: Vec<String>) -> Self {
        Self {
            pod,
            limits,
            ssh_public_keys,
        }
    }

    fn spec(&self, pin: &RelearnPin, frozen_digest: &str) -> InstanceSpec {
        InstanceSpec {
            name: format!("relearn-{}", &frozen_digest[..12.min(frozen_digest.len())]),
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

/// Pull the metrics document out of the image's stdout.
///
/// Accepts a bare JSON body too, so a future image that writes only the
/// sidecar still works.
pub fn extract_metrics(stdout: &str) -> Result<RelearnEvalMetrics, EvalError> {
    if let Some(line) = stdout.lines().find(|l| l.starts_with(METRICS_MARKER)) {
        return RelearnEvalMetrics::from_json(&line[METRICS_MARKER.len()..]);
    }
    let trimmed = stdout.trim();
    if trimmed.starts_with('{') {
        return RelearnEvalMetrics::from_json(trimmed);
    }
    Err(EvalError::Backend(format!(
        "eval image printed no {METRICS_MARKER} document"
    )))
}

#[async_trait]
impl LiveScorer for LiumHarvest {
    async fn score(
        &self,
        pin: &RelearnPin,
        frozen_digest: &str,
        artifact_digest: &str,
        holdout: &[HoldoutItem],
    ) -> Result<SliceScores, EvalError> {
        if !pin.can_rent() {
            return Err(EvalError::EvalImageUnpinned);
        }
        if holdout.is_empty() {
            return Err(EvalError::HoldoutSealed);
        }
        if self.ssh_public_keys.iter().all(|k| k.trim().is_empty()) {
            return Err(EvalError::Backend(
                "no master SSH public key; the pod would be unreachable".into(),
            ));
        }
        let request = HarvestRequest {
            schema_version: RELEARN_METRICS_SCHEMA,
            submission_digest: frozen_digest.to_owned(),
            artifact_digest: artifact_digest.to_owned(),
            base_model: pin.base_model.clone(),
            teacher_model: pin.teacher_model.clone(),
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.holdout_commitment.clone(),
            holdout: holdout.to_vec(),
        };

        let instance = self.pod.boot(&self.spec(pin, frozen_digest)).await?;
        let run = self.pod.run(&instance, &request).await;
        let shutdown = self.pod.shutdown(&instance).await;

        // Teardown first, as `rent_eval` does: an orphan pod keeps spending the
        // miner's money, so it outranks whatever the run said.
        match shutdown {
            Ok(true) => {}
            Ok(false) => {
                return Err(EvalError::Integrity(format!(
                    "pod {instance} terminate not verified"
                )))
            }
            Err(e) => return Err(e),
        }

        let stdout = run?;
        if !stdout.lines().any(|l| l.trim_end() == OK_MARKER) {
            tracing::warn!(
                instance,
                "eval image did not print {OK_MARKER}; refusing the run"
            );
            return Err(EvalError::Backend(format!(
                "eval image did not print {OK_MARKER}"
            )));
        }
        let metrics = extract_metrics(&stdout)?;
        metrics.verify(pin, frozen_digest, artifact_digest, holdout)?;
        Ok(metrics.measurement.into_slice_scores())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relearn_challenge_task::{holdout_commitment, HoldoutTask};
    use relearn_eval::BaselineMeasurement;
    use std::sync::Mutex;

    fn recs(n: u32) -> Vec<HoldoutItem> {
        (1..=n)
            .map(|id| HoldoutItem {
                id: 800 + id,
                prompt: format!("holdout item {id} with enough words for a trigram"),
                dataset_id: "dev".into(),
                task: HoldoutTask::Text,
                image_hash: String::new(),
            })
            .collect()
    }

    fn pin(hold: &[HoldoutItem]) -> RelearnPin {
        RelearnPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            holdout_commitment: holdout_commitment(hold),
            holdout_size: hold.len(),
            ..RelearnPin::default()
        }
    }

    /// A metrics document exactly as the eval image is contracted to print it.
    /// The numbers are fixture data, not a sim harness: this crate must never
    /// be able to produce a score itself.
    fn document(
        pin: &RelearnPin,
        hold: &[HoldoutItem],
        frozen: &str,
        artifact: &str,
        level: f64,
    ) -> String {
        let flat = |prefix: &str, n: usize, v: f64| {
            (0..n)
                .map(|i| (format!("{prefix}{i}"), v))
                .collect::<std::collections::BTreeMap<String, f64>>()
        };
        let m = RelearnEvalMetrics {
            schema_version: RELEARN_METRICS_SCHEMA,
            submission_digest: frozen.to_owned(),
            artifact_digest: artifact.to_owned(),
            measurement: BaselineMeasurement {
                eval_image_digest: pin.eval_image_digest.clone(),
                holdout_commitment: pin.holdout_commitment.clone(),
                holdout: hold.iter().map(|r| (format!("h{}", r.id), level)).collect(),
                public: flat("p", 40, level + 0.02),
                perturbed: hold
                    .iter()
                    .map(|r| (format!("x{}", r.id), level - 0.01))
                    .collect(),
                canaries: flat("c", 40, 0.98),
                general_canary: flat("g", 40, 0.97),
                agent_trace: 0.85,
                vision_shuffle: std::collections::BTreeMap::new(),
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
        async fn boot(&self, spec: &InstanceSpec) -> Result<String, EvalError> {
            self.log().booted.push(spec.clone());
            Ok("pod-1".into())
        }

        async fn run(
            &self,
            _instance_id: &str,
            request: &HarvestRequest,
        ) -> Result<String, EvalError> {
            self.log().requests.push(request.clone());
            self.stdout.clone().map_err(EvalError::Backend)
        }

        async fn shutdown(&self, instance_id: &str) -> Result<bool, EvalError> {
            self.log().shutdowns.push(instance_id.to_owned());
            Ok(self.verified)
        }
    }

    fn harvest(pod: Arc<FakePod>) -> LiumHarvest {
        LiumHarvest::new(
            pod,
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAmaster".into()],
        )
    }

    #[tokio::test]
    async fn boots_the_pinned_digest_and_returns_the_image_numbers() {
        let hold = recs(120);
        let p = pin(&hold);
        let pod = Arc::new(FakePod::ok(document(
            &p,
            &hold,
            "frozen-1",
            "artifact-1",
            0.61,
        )));
        let scores = harvest(Arc::clone(&pod))
            .score(&p, "frozen-1", "artifact-1", &hold)
            .await
            .expect("harvest");

        assert_eq!(scores.holdout.len(), 120);
        assert!((SliceScores::mean(&scores.holdout).unwrap_or(0.0) - 0.61).abs() < 1e-9);
        let log = pod.log();
        assert_eq!(
            log.booted[0].image_digest.as_deref(),
            Some(p.eval_image_digest.as_str())
        );
        assert_eq!(log.requests[0].holdout.len(), 120);
        assert_eq!(log.requests[0].artifact_digest, "artifact-1");
        assert_eq!(log.shutdowns, vec!["pod-1".to_owned()]);
        assert_eq!(
            log.booted[0].ssh_public_keys,
            vec!["ssh-ed25519 AAAAmaster"]
        );
        assert_eq!(log.booted[0].ssh_key_name.as_deref(), Some(SSH_KEY_NAME));
    }

    #[tokio::test]
    async fn refuses_without_a_master_ssh_key_and_never_boots() {
        let hold = recs(120);
        let p = pin(&hold);
        let pod = Arc::new(FakePod::ok(document(&p, &hold, "f", "a", 0.6)));
        let transport: Arc<dyn EvalPod> = pod.clone();
        let err = LiumHarvest::new(transport, HarvestLimits::default(), Vec::new())
            .score(&p, "f", "a", &hold)
            .await
            .expect_err("unreachable pod");
        assert!(matches!(err, EvalError::Backend(_)), "{err}");
        assert!(
            pod.log().booted.is_empty(),
            "must not rent a pod it cannot reach"
        );
    }

    #[tokio::test]
    async fn refuses_without_a_digest_pin_and_never_boots() {
        let hold = recs(120);
        let pod = Arc::new(FakePod::ok(String::new()));
        let err = harvest(Arc::clone(&pod))
            .score(&RelearnPin::default(), "frozen-1", "a", &hold)
            .await
            .expect_err("unpinned");
        assert!(matches!(err, EvalError::EvalImageUnpinned), "{err}");
        assert!(pod.log().booted.is_empty(), "must not rent without a pin");
    }

    #[tokio::test]
    async fn always_tears_the_pod_down_even_when_the_run_fails() {
        let hold = recs(120);
        let p = pin(&hold);
        let pod = Arc::new(FakePod {
            stdout: Err("cuda oom".into()),
            verified: true,
            log: Mutex::new(Recorder::default()),
        });
        let err = harvest(Arc::clone(&pod))
            .score(&p, "frozen-1", "a", &hold)
            .await
            .expect_err("run failed");
        assert!(matches!(err, EvalError::Backend(_)), "{err}");
        assert_eq!(pod.log().shutdowns, vec!["pod-1".to_owned()]);
    }

    #[tokio::test]
    async fn unverified_teardown_is_an_integrity_failure() {
        let hold = recs(120);
        let p = pin(&hold);
        let pod = Arc::new(FakePod {
            stdout: Ok(document(&p, &hold, "frozen-1", "a", 0.6)),
            verified: false,
            log: Mutex::new(Recorder::default()),
        });
        let err = harvest(pod)
            .score(&p, "frozen-1", "a", &hold)
            .await
            .expect_err("orphan pod");
        assert!(matches!(err, EvalError::Integrity(_)), "{err}");
    }

    #[tokio::test]
    async fn a_document_for_another_run_is_refused() {
        let hold = recs(120);
        let p = pin(&hold);

        let wrong_artifact = Arc::new(FakePod::ok(document(
            &p,
            &hold,
            "frozen-1",
            "someone-else",
            0.9,
        )));
        let err = harvest(wrong_artifact)
            .score(&p, "frozen-1", "artifact-1", &hold)
            .await
            .expect_err("artifact mismatch");
        assert!(matches!(err, EvalError::Baseline(_)), "{err}");

        let replay = Arc::new(FakePod::ok(document(
            &p,
            &hold,
            "an-earlier-run",
            "artifact-1",
            0.9,
        )));
        let err = harvest(replay)
            .score(&p, "frozen-1", "artifact-1", &hold)
            .await
            .expect_err("replayed run");
        assert!(matches!(err, EvalError::Baseline(_)), "{err}");
    }

    #[tokio::test]
    async fn a_run_without_the_ok_marker_is_refused() {
        let hold = recs(120);
        let p = pin(&hold);
        let body = document(&p, &hold, "frozen-1", "a", 0.6).replace(OK_MARKER, "");
        let pod = Arc::new(FakePod::ok(body));
        let err = harvest(pod)
            .score(&p, "frozen-1", "a", &hold)
            .await
            .expect_err("no ok marker");
        assert!(matches!(err, EvalError::Backend(_)), "{err}");
    }

    #[tokio::test]
    async fn silence_from_the_pod_is_never_a_score() {
        let hold = recs(120);
        let p = pin(&hold);
        for body in ["", "boot ok\nsegfault\n", "RELEARN_EVAL_OK\n"] {
            let pod = Arc::new(FakePod::ok(body.to_owned()));
            assert!(
                harvest(pod)
                    .score(&p, "frozen-1", "a", &hold)
                    .await
                    .is_err(),
                "body {body:?} must not score"
            );
        }
    }

    #[test]
    fn extract_accepts_the_marker_line_or_a_bare_document() {
        let hold = recs(120);
        let p = pin(&hold);
        let full = document(&p, &hold, "f", "a", 0.5);
        extract_metrics(&full).expect("marker line");
        let bare = full
            .lines()
            .find(|l| l.starts_with(METRICS_MARKER))
            .map(|l| l[METRICS_MARKER.len()..].to_owned())
            .unwrap_or_default();
        extract_metrics(&bare).expect("bare document");
        assert!(extract_metrics("nothing here").is_err());
    }
}
