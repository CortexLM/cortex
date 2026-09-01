//! Relearn Agent live harvest: replay episodes on a digest-pinned eval image.
//!
//! This is the control-plane client for the [`LiveScorer`] seam. The tool
//! environment, the trace replay, and both ablation arms live inside
//! `eval_image` from [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
//! This crate boots that image on a Lium pod, hands it the run request, reads
//! back the metrics document it printed, verifies the document against the pin
//! and the run identity, and tears the pod down.
//!
//! Nothing here computes a score. There is no sim fallback: a pod that does
//! not return a well-formed, correctly bound metrics document is an error, and
//! the submission answers 503.
//!
//! Image contract: `docs/RELEARN-AGENT.md` § Eval image contract.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::doc_markdown)]

use std::sync::Arc;

use async_trait::async_trait;
use harvest_pod::{EvalPod, PodProgram};
use prism_lium_types::InstanceSpec;
use relearn_agent_eval::{AgentEvalMetrics, EvalError, LiveScorer, AGENT_METRICS_SCHEMA};
use relearn_agent_score::AgentSliceScores;
use relearn_agent_task::{AgentEpisode, RelearnAgentPin};
use serde::{Deserialize, Serialize};

/// Prefix the eval image prints before its metrics document.
///
/// Shared with the other Relearn images on purpose: a harvest client is this
/// crate with a different document type, not a second marker protocol.
pub const METRICS_MARKER: &str = "RELEARN_METRICS=";

/// Marker the eval image prints on a completed run.
pub const OK_MARKER: &str = "RELEARN_EVAL_OK";

/// Directory the request and metrics sidecar live in, on the pod.
pub const POD_WORKDIR: &str = "/tmp/relearn_agent_eval";

/// Lium SSH key name the harvest registers its public key under.
pub const SSH_KEY_NAME: &str = "relearn-agent-eval-worker";

/// Image contract for the Relearn Agent eval entrypoint.
pub const PROGRAM: PodProgram = PodProgram {
    workdir: POD_WORKDIR,
    entrypoint: "relearn-agent-eval score",
    metrics_marker: METRICS_MARKER,
    ok_marker: OK_MARKER,
};

/// What the eval image is asked to score.
///
/// This is the published image's request (`docs/AGENT-EVAL-IMAGE.md` in
/// [`CortexLM/relearn`](https://github.com/CortexLM/relearn)): recorded traces
/// under `holdout`, plus the run identity. Unknown fields are tolerated on
/// the image side; missing ones are a failed run.
///
/// The request carries the **private holdout**. Rotate the episode set (salt
/// + catalogue, then re-sign) if a pod is ever suspected of exfiltration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestRequest {
    /// Must equal [`AGENT_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// Must be `relearn-agent`. The image refuses any other challenge.
    pub challenge_id: String,
    /// Frozen submission digest. Echoed back in the metrics document.
    pub submission_digest: String,
    /// Artifact to score. Echoed back in the metrics document.
    pub artifact_digest: String,
    /// Base checkpoint the artifact post-trained.
    pub base_model: String,
    /// Teacher wire id. Judge-only, for the free-text final answer.
    pub teacher_model: String,
    /// Eval image digest, so the image can stamp its own provenance.
    pub eval_image_digest: String,
    /// Commitment the episodes below must hash to.
    pub holdout_commitment: String,
    /// The verified recorded traces to replay.
    pub holdout: Vec<AgentEpisode>,
}

/// Arms the eval image always runs. Published on `/v1/status`; the image
/// itself decides the set — naming them here is documentation, not a knob.
pub const REQUIRED_ARMS: [&str; 3] = ["trace_replay", "tool_ablation", "observation_shuffle"];

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
        // Three arms over the same episode set, so the run is longer than the
        // text challenge's single pass but stays on one card.
        Self {
            max_price_per_hour: 12.0,
            gpu_count: 2,
            max_lifetime_hours: 3.0,
        }
    }
}

/// [`LiveScorer`] over a digest-pinned eval image on a Lium pod.
pub struct LiumAgentHarvest {
    pod: Arc<dyn EvalPod>,
    limits: HarvestLimits,
    /// Master's SSH public key(s). The pod is unreachable without one, so the
    /// request could not be delivered and no metrics could be read back.
    ssh_public_keys: Vec<String>,
}

impl LiumAgentHarvest {
    /// Wrap a pod transport.
    #[must_use]
    pub fn new(pod: Arc<dyn EvalPod>, limits: HarvestLimits, ssh_public_keys: Vec<String>) -> Self {
        Self {
            pod,
            limits,
            ssh_public_keys,
        }
    }

    fn spec(&self, pin: &RelearnAgentPin, frozen_digest: &str) -> InstanceSpec {
        InstanceSpec {
            name: format!(
                "relearn-agent-{}",
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

/// Pull the metrics document out of the image's stdout.
pub fn extract_metrics(stdout: &str) -> Result<AgentEvalMetrics, EvalError> {
    let body = PROGRAM.extract_document(stdout).ok_or_else(|| {
        EvalError::Backend(format!("eval image printed no {METRICS_MARKER} document"))
    })?;
    AgentEvalMetrics::from_json(body)
}

#[async_trait]
impl LiveScorer for LiumAgentHarvest {
    async fn score(
        &self,
        pin: &RelearnAgentPin,
        frozen_digest: &str,
        artifact_digest: &str,
        episodes: &[AgentEpisode],
    ) -> Result<AgentSliceScores, EvalError> {
        if !pin.can_rent() {
            return Err(EvalError::EvalImageUnpinned);
        }
        if episodes.is_empty() {
            return Err(EvalError::EpisodesSealed);
        }
        if self.ssh_public_keys.iter().all(|k| k.trim().is_empty()) {
            return Err(EvalError::Backend(
                "no master SSH public key; the pod would be unreachable".into(),
            ));
        }
        let request = HarvestRequest {
            schema_version: AGENT_METRICS_SCHEMA,
            challenge_id: relearn_agent_task::CHALLENGE_ID.to_owned(),
            submission_digest: frozen_digest.to_owned(),
            artifact_digest: artifact_digest.to_owned(),
            base_model: pin.base_model.clone(),
            teacher_model: pin.teacher_model.clone(),
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.holdout_commitment.clone(),
            holdout: episodes.to_vec(),
        };
        let body = serde_json::to_vec(&request)
            .map_err(|e| EvalError::Backend(format!("encode request: {e}")))?;

        let instance = self
            .pod
            .boot(&self.spec(pin, frozen_digest))
            .await
            .map_err(EvalError::Backend)?;
        let run = self.pod.run(&instance, &body, b"").await;
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
        metrics.verify(pin, frozen_digest, artifact_digest, episodes)?;
        Ok(metrics.measurement.into_slice_scores())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use relearn_agent_eval::BaselineMeasurement;
    use relearn_agent_score::AblationEvidence;
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

    fn pin(eps: &[AgentEpisode]) -> RelearnAgentPin {
        RelearnAgentPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            holdout_commitment: episode_commitment(eps),
            holdout_size: eps.len(),
            public_ids: (1..=40).collect(),
            ..RelearnAgentPin::default()
        }
    }

    /// A metrics document exactly as the eval image is contracted to print it.
    /// Fixture data, not a harness: this crate must never produce a score.
    fn document(
        p: &RelearnAgentPin,
        eps: &[AgentEpisode],
        frozen: &str,
        artifact: &str,
        level: f64,
    ) -> String {
        let flat = |prefix: &str, n: usize, v: f64| {
            (0..n)
                .map(|i| (format!("{prefix}{i}"), v))
                .collect::<BTreeMap<String, f64>>()
        };
        let arm = AblationEvidence {
            episodes: u32::try_from(eps.len()).unwrap_or(u32::MAX),
            score: level,
            ablated_score: (level - 0.5).max(0.0),
        };
        let m = AgentEvalMetrics {
            schema_version: AGENT_METRICS_SCHEMA,
            submission_digest: frozen.to_owned(),
            artifact_digest: artifact.to_owned(),
            measurement: BaselineMeasurement {
                eval_image_digest: p.eval_image_digest.clone(),
                holdout_commitment: p.holdout_commitment.clone(),
                holdout: eps.iter().map(|e| (format!("e{}", e.id), level)).collect(),
                public: flat("p", 40, level + 0.02),
                trace_valid: eps.iter().map(|e| (format!("e{}", e.id), 0.95)).collect(),
                capability_canary: flat("c", 40, 0.97),
                tool_ablation: arm,
                observation_shuffle: arm,
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

        async fn run(
            &self,
            _instance_id: &str,
            request: &[u8],
            _env_file: &[u8],
        ) -> Result<String, String> {
            let parsed: HarvestRequest = serde_json::from_slice(request).expect("request json");
            self.log().requests.push(parsed);
            self.stdout.clone()
        }

        async fn shutdown(&self, instance_id: &str) -> Result<bool, String> {
            self.log().shutdowns.push(instance_id.to_owned());
            Ok(self.verified)
        }
    }

    fn harvest(pod: Arc<FakePod>) -> LiumAgentHarvest {
        LiumAgentHarvest::new(
            pod,
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAmaster".into()],
        )
    }

    #[tokio::test]
    async fn boots_the_pinned_digest_and_asks_for_every_arm() {
        let eps = episodes(120);
        let p = pin(&eps);
        let pod = Arc::new(FakePod::ok(document(
            &p,
            &eps,
            "frozen-1",
            "artifact-1",
            0.61,
        )));
        let scores = harvest(Arc::clone(&pod))
            .score(&p, "frozen-1", "artifact-1", &eps)
            .await
            .expect("harvest");

        assert_eq!(scores.holdout.len(), 120);
        assert!(!scores.trace_valid.is_empty());
        assert!(scores.tool_ablation.episodes > 0);

        let log = pod.log();
        assert_eq!(
            log.booted[0].image_digest.as_deref(),
            Some(p.eval_image_digest.as_str())
        );
        assert_eq!(log.booted[0].ssh_key_name.as_deref(), Some(SSH_KEY_NAME));
        assert_eq!(log.requests[0].holdout.len(), 120);
        assert_eq!(log.requests[0].challenge_id, "relearn-agent");
        assert_eq!(log.requests[0].teacher_model, p.teacher_model);
        assert_eq!(log.shutdowns, vec!["pod-1".to_owned()]);
    }

    /// The pod needs the environment, not just the goal text: an episode
    /// without its tools and observation hash cannot be replayed.
    #[tokio::test]
    async fn the_request_carries_the_whole_environment() {
        let eps = episodes(120);
        let p = pin(&eps);
        let pod = Arc::new(FakePod::ok(document(&p, &eps, "f", "a", 0.5)));
        harvest(Arc::clone(&pod))
            .score(&p, "f", "a", &eps)
            .await
            .expect("harvest");
        let log = pod.log();
        let ep = &log.requests[0].holdout[0];
        assert!(!ep.tools.is_empty());
        assert_eq!(ep.observation_hash().len(), 64);
        assert!(ep.min_tool_calls() > 0);
        assert!(!ep.steps.is_empty());
        assert!(!ep.final_answer.is_empty());
    }

    #[tokio::test]
    async fn refuses_without_a_digest_pin_or_a_master_key_and_never_boots() {
        let eps = episodes(120);
        let p = pin(&eps);
        let unpinned = RelearnAgentPin {
            eval_image_digest: String::new(),
            ..p.clone()
        };
        let pod = Arc::new(FakePod::ok(String::new()));
        let err = harvest(Arc::clone(&pod))
            .score(&unpinned, "f", "a", &eps)
            .await
            .expect_err("unpinned");
        assert!(matches!(err, EvalError::EvalImageUnpinned), "{err}");
        assert!(pod.log().booted.is_empty());

        let keyless = LiumAgentHarvest::new(
            Arc::clone(&pod) as Arc<dyn EvalPod>,
            HarvestLimits::default(),
            Vec::new(),
        );
        assert!(keyless.score(&p, "f", "a", &eps).await.is_err());
        assert!(pod.log().booted.is_empty());
    }

    #[tokio::test]
    async fn always_tears_the_pod_down_and_refuses_an_orphan() {
        let eps = episodes(120);
        let p = pin(&eps);
        let failed = Arc::new(FakePod {
            stdout: Err("cuda oom".into()),
            verified: true,
            log: Mutex::new(Recorder::default()),
        });
        assert!(harvest(Arc::clone(&failed))
            .score(&p, "f", "a", &eps)
            .await
            .is_err());
        assert_eq!(failed.log().shutdowns, vec!["pod-1".to_owned()]);

        let orphan = Arc::new(FakePod {
            stdout: Ok(document(&p, &eps, "f", "a", 0.6)),
            verified: false,
            log: Mutex::new(Recorder::default()),
        });
        let err = harvest(orphan)
            .score(&p, "f", "a", &eps)
            .await
            .expect_err("orphan pod");
        assert!(matches!(err, EvalError::Integrity(_)), "{err}");
    }

    #[tokio::test]
    async fn a_document_for_another_run_is_refused() {
        let eps = episodes(120);
        let p = pin(&eps);
        for (frozen, artifact) in [
            ("frozen-1", "someone-else"),
            ("an-earlier-run", "artifact-1"),
        ] {
            let pod = Arc::new(FakePod::ok(document(&p, &eps, frozen, artifact, 0.9)));
            let err = harvest(pod)
                .score(&p, "frozen-1", "artifact-1", &eps)
                .await
                .expect_err("run identity mismatch");
            assert!(matches!(err, EvalError::Baseline(_)), "{err}");
        }
    }

    #[tokio::test]
    async fn silence_from_the_pod_is_never_a_score() {
        let eps = episodes(120);
        let p = pin(&eps);
        let full = document(&p, &eps, "f", "a", 0.6);
        for body in [
            String::new(),
            "boot ok\nsegfault\n".to_owned(),
            format!("{OK_MARKER}\n"),
            full.replace(OK_MARKER, ""),
        ] {
            let pod = Arc::new(FakePod::ok(body.clone()));
            assert!(
                harvest(pod).score(&p, "f", "a", &eps).await.is_err(),
                "body {body:?} must not score"
            );
        }
    }
}
