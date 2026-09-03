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
use harvest_pod::{harvest_template_name, EvalPod, PodProgram};
use prism_lium_types::InstanceSpec;
use relearn_t2i_eval::{EvalError, LiveJudge, T2iEvalMetrics, T2I_METRICS_SCHEMA};
use relearn_t2i_score::T2iSliceScores;
use relearn_t2i_store::ArtifactManifest;
use relearn_t2i_task::{FrozenPrompt, RelearnT2iPin};
use serde::{Deserialize, Serialize};

/// Prefix the eval image prints before its metrics document.
///
/// Shared with the other Relearn images on purpose: a harvest client is this
/// crate with a different document type, not a second marker protocol.
pub const METRICS_MARKER: &str = "RELEARN_METRICS=";

/// Marker the eval image prints on a completed run.
pub const OK_MARKER: &str = "RELEARN_EVAL_OK";

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
    score_binary: "/usr/bin/relearn-image-eval",
};

/// What the eval image is asked to score.
///
/// This is the published image's request (`docs/IMAGE-EVAL-IMAGE.md` in
/// [`CortexLM/relearn`](https://github.com/CortexLM/relearn)): frozen prompts
/// under `holdout` / `public`, the seed lattice, the sampler, and the miner's
/// manifest. The image derives cells itself from `pin_salt` +
/// `variations_per_prompt`. Unknown fields are tolerated on the image side;
/// missing ones are a failed run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestRequest {
    /// Must equal [`T2I_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// `relearn-image` (or the legacy `relearn-t2i` the image still accepts).
    pub challenge_id: String,
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
    /// Salt mixed into every generation seed.
    pub pin_salt: String,
    /// Images generated per prompt.
    pub variations_per_prompt: u32,
    /// Private split, prompts verbatim.
    pub holdout: Vec<FrozenPrompt>,
    /// Published split, prompts verbatim.
    pub public: Vec<FrozenPrompt>,
    /// Frozen sampler configuration.
    pub sampler: relearn_t2i_task::SamplerConfig,
    /// Miner's declared base, license, train ids, and claimed output hashes.
    pub manifest: ArtifactManifest,
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

/// Q-Judger configuration the eval image reads from its environment.
///
/// `InstanceSpec` cannot carry environment, so the pod sees nothing the
/// control plane does not hand it over SSH. Without
/// `RELEARN_T2I_JUDGE_API_URL` the image has no judge and exits without
/// printing [`OK_MARKER`].
///
/// Only the variable **names** are in git. Values travel in an env file
/// delivered over stdin.
#[derive(Debug, Clone, Default)]
pub struct JudgeEnv {
    /// `RELEARN_T2I_JUDGE_API_URL`. The image refuses to score without it.
    pub api_url: Option<String>,
    /// `RELEARN_T2I_JUDGE_MODEL`. Falls back to the pin's `judge_model`.
    pub model: Option<String>,
    /// `RELEARN_T2I_JUDGE_API_KEY`. Secret; see [`Self::secrets`].
    pub api_key: Option<String>,
    /// Backbone dir as the eval **pod** sees it. Cosmos3 is not baked in.
    pub base_model_dir: Option<String>,
    /// `HF_HOME` on the pod, when the operator set one.
    pub hf_home: Option<String>,
    /// `HF_HUB_CACHE` on the pod, when the operator set one.
    pub hf_hub_cache: Option<String>,
    /// Explicit one-shot pull. Never defaulted on.
    pub allow_model_download: bool,
}

fn env_trim(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

impl JudgeEnv {
    /// Read the operator's judge config off the host environment.
    #[must_use]
    pub fn from_host_env() -> Self {
        Self {
            api_url: relearn_t2i_eval::judge_api_url(),
            model: env_trim("RELEARN_T2I_JUDGE_MODEL"),
            api_key: relearn_t2i_eval::judge_api_key(),
            base_model_dir: env_trim("RELEARN_T2I_BASE_MODEL_DIR")
                .or_else(|| env_trim("RELEARN_BASE_MODEL_DIR")),
            hf_home: env_trim("HF_HOME"),
            hf_hub_cache: env_trim("HF_HUB_CACHE"),
            allow_model_download: env_flag("RELEARN_T2I_ALLOW_MODEL_DOWNLOAD")
                || env_flag("RELEARN_ALLOW_MODEL_DOWNLOAD"),
        }
    }

    /// Whether the image has the one variable it cannot run without.
    #[must_use]
    pub fn has_judge(&self) -> bool {
        self.api_url
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
    }

    /// Whether the pod will have Cosmos3: local dir, or an explicit download.
    #[must_use]
    pub fn has_base_weights(&self) -> bool {
        self.base_model_dir
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            || self.allow_model_download
    }

    /// Which priming var is set. Name only — never the path.
    #[must_use]
    pub fn base_weights_via(&self) -> Option<&'static str> {
        if self
            .base_model_dir
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            Some("RELEARN_T2I_BASE_MODEL_DIR")
        } else if self.allow_model_download {
            Some("RELEARN_ALLOW_MODEL_DOWNLOAD")
        } else {
            None
        }
    }

    /// Variable names present, for logs. Never values.
    #[must_use]
    pub fn present_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.api_url.is_some() {
            names.push("RELEARN_T2I_JUDGE_API_URL");
        }
        if self.model.is_some() {
            names.push("RELEARN_T2I_JUDGE_MODEL");
        }
        if self.api_key.is_some() {
            names.push("RELEARN_T2I_JUDGE_API_KEY");
        }
        if self.base_model_dir.is_some() {
            names.push("RELEARN_T2I_BASE_MODEL_DIR");
        }
        if self.hf_home.is_some() {
            names.push("HF_HOME");
        }
        if self.hf_hub_cache.is_some() {
            names.push("HF_HUB_CACHE");
        }
        if self.allow_model_download {
            names.push("RELEARN_ALLOW_MODEL_DOWNLOAD");
        }
        names
    }

    /// Values that must never appear in an error body or a log line.
    #[must_use]
    pub fn secrets(&self) -> Vec<String> {
        self.api_key.iter().cloned().collect()
    }

    /// `KEY=value` lines for the pod env file.
    #[must_use]
    pub fn env_file(&self, pin: &RelearnT2iPin) -> String {
        let model = self
            .model
            .clone()
            .unwrap_or_else(|| pin.judge_model.clone());
        let mut lines = Vec::new();
        if let Some(url) = &self.api_url {
            lines.push(format!("RELEARN_T2I_JUDGE_API_URL={url}"));
        }
        lines.push(format!("RELEARN_T2I_JUDGE_MODEL={model}"));
        if let Some(key) = &self.api_key {
            lines.push(format!("RELEARN_T2I_JUDGE_API_KEY={key}"));
        }
        lines.push(format!("RELEARN_T2I_BASE_MODEL={}", pin.base));
        if let Some(dir) = &self.base_model_dir {
            lines.push(format!("RELEARN_T2I_BASE_MODEL_DIR={dir}"));
            lines.push(format!("RELEARN_BASE_MODEL_DIR={dir}"));
        }
        if let Some(home) = &self.hf_home {
            lines.push(format!("HF_HOME={home}"));
        }
        if let Some(cache) = &self.hf_hub_cache {
            lines.push(format!("HF_HUB_CACHE={cache}"));
        }
        if self.allow_model_download {
            lines.push("RELEARN_T2I_ALLOW_MODEL_DOWNLOAD=1".into());
            lines.push("RELEARN_ALLOW_MODEL_DOWNLOAD=1".into());
        }
        lines.push(String::new());
        lines.join("\n")
    }
}

/// [`LiveJudge`] over a digest-pinned eval image on a Lium pod.
pub struct LiumImageHarvest {
    pod: Arc<dyn EvalPod>,
    limits: HarvestLimits,
    /// Master's SSH public key(s). The pod is unreachable without one, so the
    /// request could not be delivered and no metrics could be read back.
    ssh_public_keys: Vec<String>,
    /// Judge config forwarded into the pod environment.
    judge: JudgeEnv,
}

impl LiumImageHarvest {
    /// Wrap a pod transport.
    #[must_use]
    pub fn new(
        pod: Arc<dyn EvalPod>,
        limits: HarvestLimits,
        ssh_public_keys: Vec<String>,
        judge: JudgeEnv,
    ) -> Self {
        Self {
            pod,
            limits,
            ssh_public_keys,
            judge,
        }
    }

    /// Judge variable names this harvest will forward. Never values.
    #[must_use]
    pub fn judge_env_names(&self) -> Vec<&'static str> {
        self.judge.present_names()
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
            docker_image: Some(pin.eval_image.clone()),
            startup_commands: None,
            ssh_public_keys: self.ssh_public_keys.clone(),
            ssh_key_name: Some(SSH_KEY_NAME.to_owned()),
            preferred_offer_id: None,
            template_id: None,
            template_name: Some(harvest_template_name(
                &pin.eval_image,
                &pin.eval_image_digest,
            )),
        }
    }
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
        manifest: &ArtifactManifest,
    ) -> Result<T2iSliceScores, EvalError> {
        if !pin.can_rent() {
            return Err(EvalError::EvalImageUnpinned);
        }
        if holdout.is_empty() {
            return Err(EvalError::Holdout("holdout still sealed".into()));
        }
        self.ready()?;
        let expected = holdout
            .len()
            .saturating_mul(pin.prompts.variations_per_prompt as usize);
        let request = HarvestRequest {
            schema_version: T2I_METRICS_SCHEMA,
            challenge_id: relearn_t2i_task::CHALLENGE_ID.to_owned(),
            submission_digest: frozen_digest.to_owned(),
            artifact_digest: artifact_digest.to_owned(),
            base_model: pin.base.clone(),
            judge_model: pin.judge_model.clone(),
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.prompts.holdout_commitment.clone(),
            pin_salt: pin.prompts.pin_salt.clone(),
            variations_per_prompt: pin.prompts.variations_per_prompt,
            holdout: holdout.to_vec(),
            public: pin.frozen_prompts.clone(),
            sampler: pin.sampler.clone(),
            manifest: manifest.clone(),
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
            .run(&instance, &body, self.judge.env_file(pin).as_bytes())
            .await;
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

    fn ready(&self) -> Result<(), EvalError> {
        if self.ssh_public_keys.iter().all(|k| k.trim().is_empty()) {
            return Err(EvalError::Backend(
                "no master SSH public key; the eval pod would be unreachable".into(),
            ));
        }
        if !self.judge.has_judge() {
            return Err(EvalError::Backend(
                "RELEARN_T2I_JUDGE_API_URL not set on this host; the eval image has no judge \
                 and would exit without scoring"
                    .into(),
            ));
        }
        if !self.judge.has_base_weights() {
            return Err(EvalError::Backend(
                "RELEARN_T2I_BASE_MODEL_DIR not set and RELEARN_ALLOW_MODEL_DOWNLOAD is not 1; \
                 the eval image has no base weights and would preflight-fail"
                    .into(),
            ));
        }
        Ok(())
    }

    fn base_weights_primed(&self) -> bool {
        self.judge.has_base_weights()
    }

    fn base_weights_via(&self) -> Option<&'static str> {
        self.judge.base_weights_via()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use relearn_t2i_eval::T2iBaselineMeasurement;
    use relearn_t2i_task::{cell_key, frozen_prompt_commitment, L1Dimension, PromptPin};

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
        let ids: Vec<u32> = holdout().iter().map(|h| h.id).collect();
        let hold: BTreeMap<String, f64> = p
            .seed_cells(&ids)
            .into_iter()
            .map(|c| (cell_key(c.prompt_id, c.variation_index), level))
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

    fn judge() -> JudgeEnv {
        JudgeEnv {
            api_url: Some("http://judge.invalid/v1".into()),
            api_key: Some("jk-live-secret-value-0123456789".into()),
            base_model_dir: Some("/models/base".into()),
            ..JudgeEnv::default()
        }
    }

    fn harvest(pod: Arc<FakePod>) -> LiumImageHarvest {
        LiumImageHarvest::new(
            pod,
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAmaster".into()],
            judge(),
        )
    }

    #[tokio::test]
    async fn boots_the_pinned_digest_and_returns_the_image_numbers() {
        let p = pin();
        let pod = Arc::new(FakePod::ok(document(&p, "frozen-1", "artifact-1", 0.61)));
        let scores = harvest(Arc::clone(&pod))
            .score(
                &p,
                "frozen-1",
                "artifact-1",
                &holdout(),
                &ArtifactManifest::default(),
            )
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
        assert_eq!(
            log.booted[0].docker_image.as_deref(),
            Some(p.eval_image.as_str())
        );
        assert_eq!(
            log.booted[0].template_name.as_deref(),
            Some("relearn-image-eval-abababababab")
        );
        assert!(log.booted[0].startup_commands.is_none());
        assert_eq!(log.booted[0].ssh_key_name.as_deref(), Some(SSH_KEY_NAME));
        assert_eq!(log.requests[0].holdout.len(), 25);
        assert_eq!(log.requests[0].challenge_id, "relearn-image");
        assert_eq!(log.requests[0].artifact_digest, "artifact-1");
        assert_eq!(log.shutdowns, vec!["pod-1".to_owned()]);
    }

    /// Miners never bring an upsampler to the scored split, so the pod must be
    /// handed the frozen strings and the seed lattice, not ids to resolve.
    #[tokio::test]
    async fn the_request_carries_frozen_prompts_and_the_seed_lattice() {
        let p = pin();
        let pod = Arc::new(FakePod::ok(document(&p, "f", "a", 0.5)));
        harvest(Arc::clone(&pod))
            .score(&p, "f", "a", &holdout(), &ArtifactManifest::default())
            .await
            .expect("harvest");
        let log = pod.log();
        let req = &log.requests[0];
        assert_eq!(req.holdout[0].text, "prompt 900");
        assert!(!req.pin_salt.is_empty());
        assert_eq!(req.variations_per_prompt, 4);
        assert!(!req.public.is_empty());
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
            .score(
                &unpinned,
                "f",
                "a",
                &holdout(),
                &ArtifactManifest::default(),
            )
            .await
            .expect_err("unpinned");
        assert!(matches!(err, EvalError::EvalImageUnpinned), "{err}");
        assert!(pod.log().booted.is_empty());

        let keyless = LiumImageHarvest::new(
            Arc::clone(&pod) as Arc<dyn EvalPod>,
            HarvestLimits::default(),
            Vec::new(),
            judge(),
        );
        assert!(keyless
            .score(&p, "f", "a", &holdout(), &ArtifactManifest::default())
            .await
            .is_err());
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
            .score(&p, "f", "a", &holdout(), &ArtifactManifest::default())
            .await
            .is_err());
        assert_eq!(failed.log().shutdowns, vec!["pod-1".to_owned()]);

        let orphan = Arc::new(FakePod {
            stdout: Ok(document(&p, "f", "a", 0.6)),
            verified: false,
            log: Mutex::new(Recorder::default()),
        });
        let err = harvest(orphan)
            .score(&p, "f", "a", &holdout(), &ArtifactManifest::default())
            .await
            .expect_err("orphan pod");
        assert!(matches!(err, EvalError::Integrity(_)), "{err}");
    }

    #[tokio::test]
    async fn a_document_for_another_run_is_refused() {
        let p = pin();
        for (frozen, artifact) in [
            ("frozen-1", "someone-else"),
            ("an-earlier-run", "artifact-1"),
        ] {
            let pod = Arc::new(FakePod::ok(document(&p, frozen, artifact, 0.9)));
            let err = harvest(pod)
                .score(
                    &p,
                    "frozen-1",
                    "artifact-1",
                    &holdout(),
                    &ArtifactManifest::default(),
                )
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
                harvest(pod)
                    .score(&p, "f", "a", &holdout(), &ArtifactManifest::default())
                    .await
                    .is_err(),
                "body {body:?} must not score"
            );
        }
    }

    #[tokio::test]
    async fn the_judge_env_is_forwarded_into_the_pod() {
        let p = pin();
        let pod = Arc::new(FakePod::ok(document(&p, "f", "a", 0.6)));
        harvest(Arc::clone(&pod))
            .score(&p, "f", "a", &holdout(), &ArtifactManifest::default())
            .await
            .expect("harvest");
        let env = &pod.log().env_files[0];
        assert!(
            env.contains("RELEARN_T2I_JUDGE_API_URL=http://judge.invalid/v1"),
            "{env}"
        );
        assert!(
            env.contains(&format!("RELEARN_T2I_JUDGE_MODEL={}", p.judge_model)),
            "{env}"
        );
        assert!(env.contains("RELEARN_T2I_JUDGE_API_KEY="), "{env}");
        assert!(
            env.contains(&format!("RELEARN_T2I_BASE_MODEL={}", p.base)),
            "{env}"
        );
        assert!(env.contains("RELEARN_BASE_MODEL_DIR=/models/base"), "{env}");
        assert!(
            !env.contains("RELEARN_ALLOW_MODEL_DOWNLOAD"),
            "ALLOW_DOWNLOAD is never defaulted: {env}"
        );
    }

    #[tokio::test]
    async fn no_judge_url_refuses_before_renting_a_pod() {
        let p = pin();
        let pod = Arc::new(FakePod::ok(document(&p, "f", "a", 0.6)));
        let transport: Arc<dyn EvalPod> = pod.clone();
        let err = LiumImageHarvest::new(
            transport,
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAmaster".into()],
            JudgeEnv::default(),
        )
        .score(&p, "f", "a", &holdout(), &ArtifactManifest::default())
        .await
        .expect_err("no judge");
        assert!(
            err.to_string().contains("RELEARN_T2I_JUDGE_API_URL"),
            "{err}"
        );
        assert!(
            pod.log().booted.is_empty(),
            "never pay for a pod that cannot score"
        );
    }

    #[tokio::test]
    async fn no_base_weights_refuses_before_renting_a_pod() {
        let p = pin();
        let pod = Arc::new(FakePod::ok(document(&p, "f", "a", 0.6)));
        let transport: Arc<dyn EvalPod> = pod.clone();
        let err = LiumImageHarvest::new(
            transport,
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAmaster".into()],
            JudgeEnv {
                api_url: Some("http://judge.invalid/v1".into()),
                ..JudgeEnv::default()
            },
        )
        .score(&p, "f", "a", &holdout(), &ArtifactManifest::default())
        .await
        .expect_err("no backbone");
        assert!(
            err.to_string().contains("RELEARN_T2I_BASE_MODEL_DIR"),
            "{err}"
        );
        assert!(
            pod.log().booted.is_empty(),
            "never pay for a pod that will preflight-fail"
        );
    }
}
