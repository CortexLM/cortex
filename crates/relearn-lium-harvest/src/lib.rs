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

use std::sync::Arc;

use async_trait::async_trait;
use harvest_pod::{truncate_tail, EvalPod, PodProgram};
use prism_lium_types::InstanceSpec;
use relearn_challenge_task::HoldoutItem;
use relearn_eval::{EvalError, LiveScorer, RelearnEvalMetrics, RelearnPin, RELEARN_METRICS_SCHEMA};
use relearn_score::SliceScores;
use serde::{Deserialize, Serialize};

pub use harvest_pod::LiumEvalPod;

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

/// Image contract for `relearn-eval` (`docs/RELEARN.md` § Eval image contract).
pub const PROGRAM: PodProgram = PodProgram {
    workdir: POD_WORKDIR,
    entrypoint: "relearn-eval score",
    metrics_marker: METRICS_MARKER,
    ok_marker: OK_MARKER,
};

/// Teacher / judge configuration the eval image reads from its environment.
///
/// `InstanceSpec` cannot carry environment — Lium provisioning has no env
/// field — so the pod sees nothing the control plane does not hand it over
/// SSH. Without `RELEARN_TEACHER_API_URL` the image has no judge and exits
/// non-zero, which is a pod that boots, runs, and never prints
/// [`OK_MARKER`].
///
/// Only the variable **names** are in git. Values come from the live host and
/// travel in an env file delivered over stdin, so nothing — least of all the
/// API key — reaches the remote command line or the pod's process table.
#[derive(Debug, Clone, Default)]
pub struct TeacherEnv {
    /// `RELEARN_TEACHER_API_URL`. The image refuses to score without it.
    pub api_url: Option<String>,
    /// `RELEARN_TEACHER_MODEL`. Falls back to the pin's `teacher_model`.
    pub model: Option<String>,
    /// `RELEARN_TEACHER_API_KEY`. Secret; see [`Self::secrets`].
    pub api_key: Option<String>,
}

impl TeacherEnv {
    /// Read the operator's teacher config off the host environment.
    #[must_use]
    pub fn from_host_env() -> Self {
        Self {
            api_url: relearn_challenge_task::teacher_api_url(),
            model: std::env::var("RELEARN_TEACHER_MODEL")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            api_key: relearn_challenge_task::teacher_api_key(),
        }
    }

    /// Whether the image has the one variable it cannot run without.
    #[must_use]
    pub fn has_judge(&self) -> bool {
        self.api_url
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
    }

    /// Variable names present, for logs and `/v1/status`. Never values.
    #[must_use]
    pub fn present_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.api_url.is_some() {
            names.push("RELEARN_TEACHER_API_URL");
        }
        if self.model.is_some() {
            names.push("RELEARN_TEACHER_MODEL");
        }
        if self.api_key.is_some() {
            names.push("RELEARN_TEACHER_API_KEY");
        }
        names
    }

    /// Values that must never appear in an error body or a log line.
    #[must_use]
    pub fn secrets(&self) -> Vec<String> {
        self.api_key.iter().cloned().collect()
    }

    /// `KEY=value` lines for the pod env file, with the pin's model as the
    /// default so the image and the control plane agree on the wire id.
    #[must_use]
    pub fn env_file(&self, pin: &RelearnPin) -> String {
        let model = self
            .model
            .clone()
            .unwrap_or_else(|| pin.teacher_model.clone());
        let mut lines = Vec::new();
        if let Some(url) = &self.api_url {
            lines.push(format!("RELEARN_TEACHER_API_URL={url}"));
        }
        lines.push(format!("RELEARN_TEACHER_MODEL={model}"));
        if let Some(key) = &self.api_key {
            lines.push(format!("RELEARN_TEACHER_API_KEY={key}"));
        }
        lines.push(format!("RELEARN_BASE_MODEL={}", pin.base_model));
        lines.push(String::new());
        lines.join("\n")
    }
}

/// Replace secret values with a placeholder before anything is surfaced.
///
/// The image's log tail goes into a miner-visible 503, and the pod was handed
/// the teacher key, so an image that echoes its own environment would leak it.
#[must_use]
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_owned();
    for s in secrets {
        let s = s.trim();
        // Short values would redact half the log; a real credential is long.
        if s.len() >= 8 {
            out = out.replace(s, "[redacted]");
        }
    }
    out
}

/// Diagnostic tail of the image's stdout: everything that is not the metrics
/// document or a marker line, redacted and bounded.
#[must_use]
pub fn log_tail(stdout: &str, secrets: &[String], max_bytes: usize) -> String {
    let body: String = stdout
        .lines()
        .filter(|l| !l.starts_with(METRICS_MARKER) && l.trim_end() != OK_MARKER)
        .collect::<Vec<_>>()
        .join("\n");
    truncate_tail(&redact(body.trim(), secrets), max_bytes)
}

/// Bytes of image log surfaced to the miner in a 503.
pub const LOG_TAIL_HTTP_BYTES: usize = 2_048;

/// Bytes of image log written to the operator's own log.
pub const LOG_TAIL_OPERATOR_BYTES: usize = 8_192;

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

/// Lium SSH key name the harvest registers its public key under.
pub const SSH_KEY_NAME: &str = "relearn-eval-worker";

/// [`LiveScorer`] over a digest-pinned eval image on a Lium pod.
pub struct LiumHarvest {
    pod: Arc<dyn EvalPod>,
    limits: HarvestLimits,
    /// Master's SSH public key(s). The pod is unreachable without one, so the
    /// request could not be delivered and no metrics could be read back.
    ssh_public_keys: Vec<String>,
    /// Teacher config forwarded into the pod environment.
    teacher: TeacherEnv,
}

impl LiumHarvest {
    /// Wrap a pod transport.
    #[must_use]
    pub fn new(
        pod: Arc<dyn EvalPod>,
        limits: HarvestLimits,
        ssh_public_keys: Vec<String>,
        teacher: TeacherEnv,
    ) -> Self {
        Self {
            pod,
            limits,
            ssh_public_keys,
            teacher,
        }
    }

    /// Teacher variable names this harvest will forward. Never values.
    #[must_use]
    pub fn teacher_env_names(&self) -> Vec<&'static str> {
        self.teacher.present_names()
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
    let body = PROGRAM.extract_document(stdout).ok_or_else(|| {
        EvalError::Backend(format!("eval image printed no {METRICS_MARKER} document"))
    })?;
    RelearnEvalMetrics::from_json(body)
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
        // Same checks `scoring_readiness` runs before a submission gets this
        // far, repeated because nothing else stands between here and a rent.
        self.ready()?;
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
        let body = serde_json::to_vec(&request)
            .map_err(|e| EvalError::Backend(format!("encode request: {e}")))?;

        let instance = self
            .pod
            .boot(&self.spec(pin, frozen_digest))
            .await
            .map_err(EvalError::Backend)?;
        let run = self
            .pod
            .run(&instance, &body, self.teacher.env_file(pin).as_bytes())
            .await;
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
            Err(e) => return Err(EvalError::Backend(e)),
        }

        let stdout = run.map_err(EvalError::Backend)?;
        if !PROGRAM.ran_to_completion(&stdout) {
            let secrets = self.teacher.secrets();
            // A pod that boots, runs, and prints no marker is the hardest
            // failure to diagnose from the outside, so the image's own log is
            // the answer. Redacted: the pod was handed the teacher key, and
            // this tail goes into a miner-visible 503.
            tracing::warn!(
                instance,
                tail = %log_tail(&stdout, &secrets, LOG_TAIL_OPERATOR_BYTES),
                "eval image did not print {OK_MARKER}; refusing the run"
            );
            let tail = log_tail(&stdout, &secrets, LOG_TAIL_HTTP_BYTES);
            let detail = if tail.is_empty() {
                "no output".to_owned()
            } else {
                tail
            };
            return Err(EvalError::Backend(format!(
                "eval image did not print {OK_MARKER}; run.log tail: {detail}"
            )));
        }
        let metrics = extract_metrics(&stdout)?;
        metrics.verify(pin, frozen_digest, artifact_digest, holdout)?;
        Ok(metrics.measurement.into_slice_scores())
    }

    fn ready(&self) -> Result<(), EvalError> {
        if self.ssh_public_keys.iter().all(|k| k.trim().is_empty()) {
            return Err(EvalError::Backend(
                "no master SSH public key; the eval pod would be unreachable".into(),
            ));
        }
        // The pod inherits nothing from this host, so without the judge URL the
        // image exits non-zero after the rent has already been paid for.
        if !self.teacher.has_judge() {
            return Err(EvalError::Backend(
                "RELEARN_TEACHER_API_URL not set on this host; the eval image has no judge \
                 and would exit without scoring"
                    .into(),
            ));
        }
        Ok(())
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
        env_files: Vec<String>,
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
            env_file: &[u8],
        ) -> Result<String, String> {
            let parsed: HarvestRequest = serde_json::from_slice(request).expect("request json");
            let mut log = self.log();
            log.requests.push(parsed);
            log.env_files
                .push(String::from_utf8_lossy(env_file).into_owned());
            drop(log);
            self.stdout.clone()
        }

        async fn shutdown(&self, instance_id: &str) -> Result<bool, String> {
            self.log().shutdowns.push(instance_id.to_owned());
            Ok(self.verified)
        }
    }

    fn teacher() -> TeacherEnv {
        TeacherEnv {
            api_url: Some("http://teacher.invalid/v1".into()),
            model: None,
            api_key: Some("tk-live-secret-value-0123456789".into()),
        }
    }

    fn harvest(pod: Arc<FakePod>) -> LiumHarvest {
        LiumHarvest::new(
            pod,
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAmaster".into()],
            teacher(),
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
        let err = LiumHarvest::new(transport, HarvestLimits::default(), Vec::new(), teacher())
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

    /// The live failure this fixes: a pod that boots and runs but sees no
    /// teacher config, because `InstanceSpec` cannot carry environment.
    #[tokio::test]
    async fn the_teacher_env_is_forwarded_into_the_pod() {
        let hold = recs(120);
        let p = pin(&hold);
        let pod = Arc::new(FakePod::ok(document(&p, &hold, "f", "a", 0.6)));
        harvest(Arc::clone(&pod))
            .score(&p, "f", "a", &hold)
            .await
            .expect("harvest");

        let log = pod.log();
        let env = &log.env_files[0];
        assert!(
            env.contains("RELEARN_TEACHER_API_URL=http://teacher.invalid/v1"),
            "{env}"
        );
        // Model defaults to the pin so the image and the pin agree.
        assert!(
            env.contains(&format!("RELEARN_TEACHER_MODEL={}", p.teacher_model)),
            "{env}"
        );
        assert!(env.contains("RELEARN_TEACHER_API_KEY="), "{env}");
        assert!(
            env.contains(&format!("RELEARN_BASE_MODEL={}", p.base_model)),
            "{env}"
        );
    }

    #[tokio::test]
    async fn no_judge_url_refuses_before_renting_a_pod() {
        let hold = recs(120);
        let p = pin(&hold);
        let pod = Arc::new(FakePod::ok(document(&p, &hold, "f", "a", 0.6)));
        let transport: Arc<dyn EvalPod> = pod.clone();
        let err = LiumHarvest::new(
            transport,
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAmaster".into()],
            TeacherEnv::default(),
        )
        .score(&p, "f", "a", &hold)
        .await
        .expect_err("no judge");
        assert!(err.to_string().contains("RELEARN_TEACHER_API_URL"), "{err}");
        assert!(
            pod.log().booted.is_empty(),
            "never pay for a pod that cannot score"
        );
    }

    /// A pod that boots, runs, and prints no marker is the hardest failure to
    /// diagnose from outside, so the image's own log has to come back.
    #[tokio::test]
    async fn a_missing_marker_surfaces_the_redacted_log_tail() {
        let hold = recs(120);
        let p = pin(&hold);
        let pod = Arc::new(FakePod::ok(
            "loading base model\nRELEARN_TEACHER_API_KEY=tk-live-secret-value-0123456789\n\
             RuntimeError: judge unreachable\nexit=1\n"
                .to_owned(),
        ));
        let err = harvest(pod)
            .score(&p, "f", "a", &hold)
            .await
            .expect_err("no marker");
        let msg = err.to_string();
        assert!(msg.contains("RuntimeError: judge unreachable"), "{msg}");
        assert!(msg.contains("exit=1"), "{msg}");
        assert!(
            !msg.contains("tk-live-secret-value-0123456789"),
            "the teacher key must never reach a miner-visible body: {msg}"
        );
        assert!(msg.contains("[redacted]"), "{msg}");
    }

    #[tokio::test]
    async fn an_empty_log_still_says_something_useful() {
        let hold = recs(120);
        let p = pin(&hold);
        let pod = Arc::new(FakePod::ok(String::new()));
        let err = harvest(pod)
            .score(&p, "f", "a", &hold)
            .await
            .expect_err("silence");
        assert!(err.to_string().contains("no output"), "{err}");
    }

    #[test]
    fn redaction_needs_a_credential_sized_secret() {
        // Redacting a short value would blank half the log for no gain.
        assert_eq!(redact("a and b", &["a".into()]), "a and b");
        assert_eq!(
            redact("key=abcdefghij done", &["abcdefghij".into()]),
            "key=[redacted] done"
        );
    }

    #[test]
    fn the_log_tail_drops_the_metrics_document() {
        let hold = recs(120);
        let p = pin(&hold);
        let full = document(&p, &hold, "f", "a", 0.5);
        let tail = log_tail(&full, &[], LOG_TAIL_HTTP_BYTES);
        assert!(!tail.contains(METRICS_MARKER), "{tail}");
        assert!(!tail.contains("holdout_commitment"), "{tail}");
        assert!(tail.contains("boot ok"), "{tail}");
    }

    #[test]
    fn env_names_are_reported_without_values() {
        let names = teacher().present_names();
        assert!(names.contains(&"RELEARN_TEACHER_API_URL"));
        assert!(names.contains(&"RELEARN_TEACHER_API_KEY"));
        assert!(
            !names.contains(&"RELEARN_TEACHER_MODEL"),
            "unset on this host"
        );
        let joined = names.join(",");
        assert!(!joined.contains("teacher.invalid"), "{joined}");
        assert!(!joined.contains("tk-live"), "{joined}");
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
