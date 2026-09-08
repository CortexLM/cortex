//! Proof live harvest: score an artifact on a digest-pinned `proof-eval` image.
//!
//! The RLM agent and the metric harness live inside the image. This crate
//! boots that image on a Lium pod, hands it the run request (including the
//! live `InferenceOffer` judge backend and the topic constraints the image
//! must enforce — e.g. a 12.5 Gbit/s cap), reads back the metrics document,
//! and tears the pod down. Nothing here computes a score. Miners do not bind
//! or train against the offer. There is no sim fallback.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::too_many_arguments
)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use async_trait::async_trait;
use harvest_pod::{
    harvest_template_name, truncate_tail, EvalPod, PodProgram, RunExtras, HOLDOUT_DIR, PROXY_DIR,
};
use prism_lium_types::InstanceSpec;
use proof_eval::{
    secret_backed_base_url, EvalError, LiveScorer, ProofEvalDocument, PROOF_METRICS_SCHEMA,
};
use proof_task::{
    resolve_inference, HoldoutRecord, InferenceOffer, ProofPin, TopicDocument, CHALLENGE_ID,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Prefix the eval image prints before its metrics document.
pub const METRICS_MARKER: &str = "PROOF_METRICS=";

/// Marker the eval image prints on a completed run.
pub const OK_MARKER: &str = "PROOF_EVAL_OK";

/// Bytes of pod stdout retained on a missing [`OK_MARKER`] refuse.
const STDOUT_TAIL_BYTES: usize = 8 * 1024;

/// Directory the request and metrics sidecar live in, on the pod.
pub const POD_WORKDIR: &str = "/tmp/proof_eval";

/// Lium SSH key name the harvest registers its public key under.
pub const SSH_KEY_NAME: &str = "proof-eval-worker";

/// Image contract for the Proof eval entrypoint.
pub const PROGRAM: PodProgram = PodProgram {
    workdir: POD_WORKDIR,
    entrypoint: "proof-eval score",
    metrics_marker: METRICS_MARKER,
    ok_marker: OK_MARKER,
    score_binary: "/usr/bin/proof-eval",
};

/// What the eval image is asked to score.
///
/// The request carries the **private holdout** and the topic constraints.
/// The image enforces the comms cap; it does not trust the miner's claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestRequest {
    /// Must equal [`PROOF_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// Must be `proof`.
    pub challenge_id: String,
    /// Frozen submission digest.
    pub submission_digest: String,
    /// Artifact to score.
    pub artifact_digest: String,
    /// Topic id.
    pub topic_id: String,
    /// Metric family wire name.
    pub family: String,
    /// Live judge offer id the eval image must call.
    pub inference_offer_id: String,
    /// Provider kind wire name.
    pub provider_kind: String,
    /// Judge origin (operator state; not a public status field).
    pub base_url: String,
    /// Serving mode.
    pub mode: String,
    /// Provider model id (not an HF bake).
    pub model_ref: String,
    /// Input token cap for this run (min of offer and topic).
    pub max_input_tokens: u32,
    /// Output token cap for this run.
    pub max_output_tokens: u32,
    /// Judge config commitment.
    pub config_commitment: String,
    /// Eval image digest, so the image can stamp its own provenance.
    pub eval_image_digest: String,
    /// Commitment the records below must hash to.
    pub holdout_commitment: String,
    /// Topic constraints the image enforces (12.5 Gbit/s, no IB, …).
    pub constraints: proof_task::Constraints,
    /// FLOP budget.
    pub flops_budget: u64,
    /// Wall budget (throughput).
    pub wall_budget_s: u64,
    /// Miner claim string.
    pub claim: String,
    /// Verified holdout records. Rotate the set if a pod is suspected of exfil.
    pub holdout: Vec<HoldoutRecord>,
}

/// Rent limits for one harvest.
#[derive(Debug, Clone)]
pub struct HarvestLimits {
    /// Max pod lifetime hours.
    pub max_lifetime_hours: f64,
    /// Max USD per GPU-hour.
    pub max_price_per_hour: f64,
    /// GPUs requested.
    pub gpu_count: u32,
}

impl Default for HarvestLimits {
    fn default() -> Self {
        Self {
            max_lifetime_hours: 6.0,
            max_price_per_hour: 12.0,
            gpu_count: 1,
        }
    }
}

/// Env file staged as `teacher.env` and sourced by the eval image.
///
/// The key never enters [`HarvestRequest`] or `/v1/status`. Values are
/// single-quoted so a special character cannot break `set -a` sourcing.
/// Live score also pins the pod-local measurement dir and holdout store
/// (operator-staged trees; not Hugging Face ids).
pub fn judge_teacher_env(api_key: &str) -> Result<Vec<u8>, EvalError> {
    let key = api_key.trim();
    if key.is_empty() || key.contains('\n') || key.contains('\r') || key.contains('\0') {
        return Err(EvalError::InferenceAuthMissing);
    }
    let escaped = key.replace('\'', "'\\''");
    Ok(format!(
        "OPENAI_API_KEY='{escaped}'\nPROOF_INFERENCE_API_KEY='{escaped}'\n\
         PROOF_PROXY_MODEL_DIR='{POD_WORKDIR}/{PROXY_DIR}'\n\
         PROOF_HOLDOUT_STORE='{POD_WORKDIR}/{HOLDOUT_DIR}'\n"
    )
    .into_bytes())
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn file_sha256_hex(path: &Path) -> Result<String, EvalError> {
    let mut file = std::fs::File::open(path).map_err(|_| EvalError::HoldoutStoreMissing)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|_| EvalError::HoldoutStoreMissing)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn verify_shard_bytes(path: &Path, digest: &str) -> Result<(), EvalError> {
    let got = file_sha256_hex(path)?;
    if got != digest {
        return Err(EvalError::HoldoutStoreMissing);
    }
    Ok(())
}

/// The holdout store can supply at least one digest-named shard whose
/// bytes match the filename / catalog digest.
pub fn holdout_store_usable(store: &Path) -> Result<(), EvalError> {
    if !store.is_dir() {
        return Err(EvalError::HoldoutStoreMissing);
    }
    let entries = std::fs::read_dir(store).map_err(|_| EvalError::HoldoutStoreMissing)?;
    let mut usable = false;
    for ent in entries {
        let ent = ent.map_err(|_| EvalError::HoldoutStoreMissing)?;
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let digest = name.to_ascii_lowercase();
        if !is_hex64(&digest) {
            continue;
        }
        verify_shard_bytes(&path, &digest)?;
        usable = true;
    }
    if usable {
        Ok(())
    } else {
        Err(EvalError::HoldoutStoreMissing)
    }
}

/// Proxy tree exists, is non-empty, and `tar` can archive it (stdout discarded).
pub fn proxy_archive_usable(dir: &Path) -> Result<(), EvalError> {
    if !dir.is_dir() {
        return Err(EvalError::ProxyModelMissing);
    }
    let mut entries = std::fs::read_dir(dir).map_err(|_| EvalError::ProxyModelMissing)?;
    match entries.next() {
        Some(Ok(_)) => {}
        Some(Err(_)) | None => return Err(EvalError::ProxyModelMissing),
    }
    let status = Command::new("tar")
        .arg("-C")
        .arg(dir)
        .arg("-cf")
        .arg("-")
        .arg(".")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| EvalError::ProxyModelMissing)?;
    if status.success() {
        Ok(())
    } else {
        Err(EvalError::ProxyModelMissing)
    }
}

/// Pack operator-staged holdout shard bytes for the pod.
///
/// Each record must already exist as `store/<content_sha256>`, and the
/// file bytes must hash to that digest. Missing or stale bytes refuse.
pub fn pack_holdout_tar(store: &Path, holdout: &[HoldoutRecord]) -> Result<Vec<u8>, EvalError> {
    if !store.is_dir() {
        return Err(EvalError::HoldoutStoreMissing);
    }
    let mut names = Vec::with_capacity(holdout.len());
    for rec in holdout {
        let digest = rec.content_sha256.to_ascii_lowercase();
        if !is_hex64(&digest) {
            return Err(EvalError::HoldoutStoreMissing);
        }
        let path = store.join(&digest);
        if !path.is_file() {
            return Err(EvalError::HoldoutStoreMissing);
        }
        verify_shard_bytes(&path, &digest)?;
        names.push(digest);
    }
    tar_named_files(store, &names).map_err(|_| EvalError::HoldoutStoreMissing)
}

/// Pack operator-provided local measurement weights to a temp tar (no HF bake).
///
/// The archive is written to disk so live score can stream it to SSH
/// instead of buffering the model in control-plane memory.
pub fn pack_proxy_tar(dir: &Path) -> Result<PathBuf, EvalError> {
    proxy_archive_usable(dir)?;
    let dest = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("proof-proxy-{}-{nanos}.tar", std::process::id()))
    };
    let status = Command::new("tar")
        .arg("-C")
        .arg(dir)
        .arg("-cf")
        .arg(&dest)
        .arg(".")
        .status()
        .map_err(|_| EvalError::ProxyModelMissing)?;
    if !status.success() {
        let _ = std::fs::remove_file(&dest);
        return Err(EvalError::ProxyModelMissing);
    }
    match std::fs::metadata(&dest) {
        Ok(meta) if meta.len() > 0 => Ok(dest),
        _ => {
            let _ = std::fs::remove_file(&dest);
            Err(EvalError::ProxyModelMissing)
        }
    }
}

fn tar_named_files(dir: &Path, names: &[String]) -> Result<Vec<u8>, String> {
    if names.is_empty() {
        return Err("no holdout shards to pack".into());
    }
    let mut cmd = Command::new("tar");
    cmd.arg("-C").arg(dir).arg("-cf").arg("-");
    for name in names {
        cmd.arg(name);
    }
    run_tar(cmd)
}

fn run_tar(mut cmd: Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| format!("tar: {e}"))?;
    if !out.status.success() {
        return Err(format!("tar exit {}", out.status));
    }
    if out.stdout.is_empty() {
        return Err("tar produced no archive".into());
    }
    Ok(out.stdout)
}

/// Delete a staged proxy tar when the harvest returns (or unwinds).
struct ProxyTarGuard(Option<PathBuf>);

impl Drop for ProxyTarGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// [`LiveScorer`] over a digest-pinned eval image on a Lium pod.
pub struct LiumProofHarvest {
    pod: Arc<dyn EvalPod>,
    limits: HarvestLimits,
    ssh_public_keys: Vec<String>,
    judge_api_key: Option<String>,
    /// Host directory of local measurement weights. Staged onto the pod as
    /// [`PROOF_PROXY_MODEL_DIR`]. Missing → refuse (no HF bake).
    proxy_model_dir: Option<PathBuf>,
    /// Host directory of holdout shard files named `<content_sha256>`.
    holdout_store: Option<PathBuf>,
}

impl LiumProofHarvest {
    /// Wrap a pod transport.
    #[must_use]
    pub fn new(pod: Arc<dyn EvalPod>, limits: HarvestLimits, ssh_public_keys: Vec<String>) -> Self {
        Self {
            pod,
            limits,
            ssh_public_keys,
            judge_api_key: None,
            proxy_model_dir: None,
            holdout_store: None,
        }
    }

    /// Inject the judge API key staged into `teacher.env` on the pod.
    ///
    /// Empty / whitespace is treated as missing (fail-closed on a live run).
    #[must_use]
    pub fn with_judge_api_key(mut self, key: Option<String>) -> Self {
        self.judge_api_key = key.and_then(|s| {
            let t = s.trim().to_owned();
            (!t.is_empty()).then_some(t)
        });
        self
    }

    /// Host path to local measurement weights (no HF bake / download).
    #[must_use]
    pub fn with_proxy_model_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.proxy_model_dir = dir.filter(|p| !p.as_os_str().is_empty());
        self
    }

    /// Host path to holdout shard bytes (`<content_sha256>` files).
    #[must_use]
    pub fn with_holdout_store(mut self, dir: Option<PathBuf>) -> Self {
        self.holdout_store = dir.filter(|p| !p.as_os_str().is_empty());
        self
    }

    fn live_extras(
        &self,
        holdout: &[HoldoutRecord],
    ) -> Result<(RunExtras, ProxyTarGuard), EvalError> {
        let proxy = self
            .proxy_model_dir
            .as_deref()
            .ok_or(EvalError::ProxyModelMissing)?;
        let store = self
            .holdout_store
            .as_deref()
            .ok_or(EvalError::HoldoutStoreMissing)?;
        // Holdout first: a missing/mismatched shard must not leave a
        // model-sized proxy tar on the control plane.
        let holdout_tar = pack_holdout_tar(store, holdout)?;
        let proxy_path = pack_proxy_tar(proxy)?;
        let guard = ProxyTarGuard(Some(proxy_path.clone()));
        Ok((
            RunExtras {
                holdout_tar,
                proxy_tar_path: Some(proxy_path),
            },
            guard,
        ))
    }

    fn spec(&self, pin: &ProofPin, frozen_digest: &str) -> InstanceSpec {
        InstanceSpec {
            name: format!("proof-{}", &frozen_digest[..12.min(frozen_digest.len())]),
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

#[async_trait]
impl LiveScorer for LiumProofHarvest {
    async fn score(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        frozen_digest: &str,
        artifact_digest: &str,
        holdout: &[HoldoutRecord],
        claim: &str,
    ) -> Result<ProofEvalDocument, EvalError> {
        if !pin.can_rent() {
            return Err(EvalError::EvalImageUnpinned);
        }
        if holdout.is_empty() {
            return Err(EvalError::HoldoutSealed);
        }
        self.ready()?;
        offer
            .serves_topic(pin, topic)
            .map_err(|e| EvalError::InferenceOffer(e.to_string()))?;
        let resolved = resolve_inference(
            pin,
            Some(&topic.inference),
            secret_backed_base_url().as_deref(),
            Some(offer),
        );
        if !resolved.ready_to_score() {
            return Err(EvalError::InferenceOffer(
                proof_task::OfferError::Incomplete.to_string(),
            ));
        }
        if resolved.base_url.trim() != offer.provider.base_url.trim() {
            return Err(EvalError::InferenceOffer(
                proof_task::OfferError::OriginMismatch.to_string(),
            ));
        }
        let env = judge_teacher_env(self.judge_api_key.as_deref().unwrap_or(""))?;
        let (extras, _proxy_guard) = self.live_extras(holdout)?;
        let max_in = resolved.max_input_tokens.min(offer.config.max_input_tokens);
        let max_out = resolved
            .max_output_tokens
            .min(offer.config.max_output_tokens);
        let request = HarvestRequest {
            schema_version: PROOF_METRICS_SCHEMA,
            challenge_id: CHALLENGE_ID.to_owned(),
            submission_digest: frozen_digest.to_owned(),
            artifact_digest: artifact_digest.to_owned(),
            topic_id: topic.id.clone(),
            family: topic.metric.family.as_str().to_owned(),
            inference_offer_id: offer.offer_id.clone(),
            provider_kind: resolved.provider.as_str().to_owned(),
            base_url: resolved.base_url.clone(),
            mode: resolved.mode.as_str().to_owned(),
            model_ref: resolved.model.clone(),
            max_input_tokens: max_in,
            max_output_tokens: max_out,
            config_commitment: offer.config_commitment.clone(),
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: topic.holdout_commitment.clone(),
            constraints: topic.constraints,
            flops_budget: topic.flops_budget,
            wall_budget_s: topic.metric.wall_budget_s,
            claim: claim.to_owned(),
            holdout: holdout.to_vec(),
        };
        let body = serde_json::to_vec(&request)
            .map_err(|e| EvalError::Backend(format!("encode request: {e}")))?;

        let instance = self
            .pod
            .boot(&self.spec(pin, frozen_digest))
            .await
            .map_err(EvalError::Backend)?;
        let run = self.pod.run(&instance, &body, &env, &extras).await;
        let shutdown = self.pod.shutdown(&instance).await;
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
            let stdout_tail = truncate_tail(&stdout, STDOUT_TAIL_BYTES);
            tracing::warn!(
                instance,
                stdout_tail = %stdout_tail,
                "eval image did not print {OK_MARKER}; refusing"
            );
            return Err(EvalError::Backend(format!(
                "eval image did not print {OK_MARKER}"
            )));
        }
        let body = PROGRAM.extract_document(&stdout).ok_or_else(|| {
            EvalError::NoVerdict(format!("eval image printed no {METRICS_MARKER} document"))
        })?;
        let doc = ProofEvalDocument::from_json(body)?;
        doc.verify(pin, topic, frozen_digest, artifact_digest)?;
        Ok(doc)
    }

    fn ready(&self) -> Result<(), EvalError> {
        if self.ssh_public_keys.iter().all(|k| k.trim().is_empty()) {
            return Err(EvalError::Backend(
                "no master SSH public key; the eval pod would be unreachable".into(),
            ));
        }
        match self.proxy_model_dir.as_deref() {
            Some(dir) => proxy_archive_usable(dir)?,
            None => return Err(EvalError::ProxyModelMissing),
        }
        match self.holdout_store.as_deref() {
            Some(dir) => holdout_store_usable(dir)?,
            None => return Err(EvalError::HoldoutStoreMissing),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use proof_task::{default_adamw, holdout_commitment, synthetic_holdout, STRATUM_SIZE};

    use super::*;

    #[test]
    fn program_is_proof_not_relearn() {
        assert_eq!(PROGRAM.metrics_marker, "PROOF_METRICS=");
        assert_eq!(PROGRAM.ok_marker, "PROOF_EVAL_OK");
        assert!(PROGRAM.entrypoint.contains("proof-eval"));
    }

    #[test]
    fn request_carries_constraints_the_image_must_enforce() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let mut b = default_adamw(proof_task::FLOPS_BUDGET_MAX);
        b.script_sha256 = "11".repeat(32);
        b.metrics_commitment = "22".repeat(32);
        let topic = TopicDocument {
            id: "dt-no-ib-v0".into(),
            statement: "no IB".into(),
            constraints: proof_task::Constraints {
                no_infiniband: true,
                no_nvlink: true,
                no_nccl_fast_fabric: true,
                max_inter_node_gbps: Some(12.5),
            },
            baseline: b,
            holdout_commitment: holdout_commitment(&recs),
            status: proof_task::TopicStatus::Open,
            ..TopicDocument::default()
        };
        let req = HarvestRequest {
            schema_version: PROOF_METRICS_SCHEMA,
            challenge_id: CHALLENGE_ID.into(),
            submission_digest: "d".into(),
            artifact_digest: "a".into(),
            topic_id: topic.id.clone(),
            family: topic.metric.family.as_str().into(),
            inference_offer_id: "master-v0".into(),
            provider_kind: "openai_compatible".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
            mode: "chat".into(),
            model_ref: "master-proxy-v0".into(),
            max_input_tokens: 4_096,
            max_output_tokens: 256,
            config_commitment: "ab".repeat(32),
            eval_image_digest: String::new(),
            holdout_commitment: topic.holdout_commitment.clone(),
            constraints: topic.constraints,
            flops_budget: topic.flops_budget,
            wall_budget_s: topic.metric.wall_budget_s,
            claim: String::new(),
            holdout: recs,
        };
        let v = serde_json::to_value(&req).expect("json");
        assert_eq!(v["constraints"]["max_inter_node_gbps"], 12.5);
        assert_eq!(v["constraints"]["no_infiniband"], true);
        assert_eq!(v["challenge_id"], "proof");
        assert_eq!(v["provider_kind"], "openai_compatible");
        assert_eq!(v["mode"], "chat");
        assert!(v.get("proxy_model").is_none());
        assert!(v.get("api_key").is_none());
    }

    #[test]
    fn teacher_env_carries_the_judge_key_and_never_the_request() {
        let env = judge_teacher_env("sk-live-not-a-real-secret").expect("env");
        let text = String::from_utf8(env).expect("utf8");
        assert!(text.contains("OPENAI_API_KEY='sk-live-not-a-real-secret'"));
        assert!(text.contains("PROOF_INFERENCE_API_KEY='sk-live-not-a-real-secret'"));
        assert!(judge_teacher_env("").is_err());
        assert!(judge_teacher_env("has\nnewline").is_err());
        let quoted = judge_teacher_env("o'reilly").expect("quote");
        let quoted = String::from_utf8(quoted).expect("utf8");
        assert!(quoted.contains("OPENAI_API_KEY='o'\\''reilly'"));
        assert!(quoted.contains("PROOF_INFERENCE_API_KEY='o'\\''reilly'"));
        assert!(quoted.contains("PROOF_PROXY_MODEL_DIR='/tmp/proof_eval/proxy'"));
        assert!(quoted.contains("PROOF_HOLDOUT_STORE='/tmp/proof_eval/holdout'"));
    }

    struct CapturePod {
        env: std::sync::Mutex<Vec<u8>>,
        request: std::sync::Mutex<Vec<u8>>,
        extras: std::sync::Mutex<RunExtras>,
        proxy_tar_len: std::sync::Mutex<u64>,
        booted: std::sync::Mutex<bool>,
    }

    impl CapturePod {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                env: std::sync::Mutex::new(Vec::new()),
                request: std::sync::Mutex::new(Vec::new()),
                extras: std::sync::Mutex::new(RunExtras::default()),
                proxy_tar_len: std::sync::Mutex::new(0),
                booted: std::sync::Mutex::new(false),
            })
        }
    }

    #[async_trait]
    impl EvalPod for CapturePod {
        async fn boot(&self, _spec: &InstanceSpec) -> Result<String, String> {
            *self.booted.lock().expect("boot") = true;
            Ok("pod-1".into())
        }

        async fn run(
            &self,
            _instance_id: &str,
            request: &[u8],
            env_file: &[u8],
            extras: &RunExtras,
        ) -> Result<String, String> {
            *self.request.lock().expect("req") = request.to_vec();
            *self.env.lock().expect("env") = env_file.to_vec();
            *self.extras.lock().expect("extras") = extras.clone();
            let n = extras
                .proxy_tar_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map_or(0, |m| m.len());
            *self.proxy_tar_len.lock().expect("proxy len") = n;
            Err("captured".into())
        }

        async fn shutdown(&self, _instance_id: &str) -> Result<bool, String> {
            Ok(true)
        }
    }

    fn harvest_pin() -> ProofPin {
        let mut p = ProofPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            topic_pubkey: "ab".repeat(32),
            ..ProofPin::default()
        };
        p.inference.model = "master-proxy-v0".into();
        p
    }

    fn harvest_offer() -> InferenceOffer {
        let config = proof_task::InferenceConfig {
            mode: proof_task::InferenceMode::Chat,
            model_ref: "master-proxy-v0".into(),
            max_input_tokens: 32_768,
            max_output_tokens: 8_192,
            temperature: Some(0.0),
            top_p: None,
            timeout_ms: None,
        };
        InferenceOffer {
            offer_id: "master-v0".into(),
            provider: proof_task::InferenceProvider {
                kind: proof_task::InferenceProviderKind::OpenaiCompatible,
                base_url: "http://127.0.0.1:8000/v1".into(),
            },
            config_commitment: proof_task::inference_config_commitment(
                &config,
                "http://127.0.0.1:8000/v1",
            ),
            config,
            status: proof_task::OfferStatus::Open,
        }
    }

    fn harvest_topic(recs: &[proof_task::HoldoutRecord]) -> TopicDocument {
        let mut b = default_adamw(proof_task::FLOPS_BUDGET_MAX);
        b.script_sha256 = "11".repeat(32);
        b.metrics_commitment = "22".repeat(32);
        TopicDocument {
            id: "dt-no-ib-v0".into(),
            holdout_commitment: holdout_commitment(recs),
            baseline: b,
            status: proof_task::TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    fn synthetic_shard_bytes(rec: &proof_task::HoldoutRecord) -> Vec<u8> {
        let mut buf = b"proof-synthetic-shard-v1".to_vec();
        buf.extend_from_slice(rec.split.as_str().as_bytes());
        buf.extend_from_slice(&rec.id.to_le_bytes());
        buf
    }

    fn live_asset_dirs(recs: &[proof_task::HoldoutRecord]) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "proof-harvest-assets-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let proxy = root.join("proxy");
        let store = root.join("holdout");
        std::fs::create_dir_all(&proxy).expect("proxy");
        std::fs::create_dir_all(&store).expect("store");
        std::fs::write(
            proxy.join("config.json"),
            b"{\"architectures\":[\"test\"]}\n",
        )
        .expect("proxy file");
        for rec in recs {
            std::fs::write(
                store.join(rec.content_sha256.to_ascii_lowercase()),
                synthetic_shard_bytes(rec),
            )
            .expect("shard");
        }
        (proxy, store)
    }

    fn harvest_with_assets(
        pod: Arc<CapturePod>,
        recs: &[proof_task::HoldoutRecord],
        key: Option<String>,
    ) -> LiumProofHarvest {
        let (proxy, store) = live_asset_dirs(recs);
        LiumProofHarvest::new(
            pod,
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAtest proof".into()],
        )
        .with_judge_api_key(key)
        .with_proxy_model_dir(Some(proxy))
        .with_holdout_store(Some(store))
    }

    #[tokio::test]
    async fn harvest_stages_teacher_env_and_never_puts_the_key_on_the_request() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let topic = harvest_topic(&recs);
        let pod = CapturePod::new();
        let harvest =
            harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()));
        let err = harvest
            .score(
                &harvest_pin(),
                &topic,
                &harvest_offer(),
                "digest-abcdef",
                "artifact",
                &recs,
                "claim",
            )
            .await
            .expect_err("capture");
        assert!(matches!(err, EvalError::Backend(_)), "{err}");
        assert!(*pod.booted.lock().expect("booted"));
        let env = String::from_utf8(pod.env.lock().expect("env").clone()).expect("utf8");
        assert!(
            env.contains("OPENAI_API_KEY='sk-live-not-a-real-secret'"),
            "{env}"
        );
        assert!(
            env.contains("PROOF_INFERENCE_API_KEY='sk-live-not-a-real-secret'"),
            "{env}"
        );
        assert!(
            env.contains("PROOF_PROXY_MODEL_DIR='/tmp/proof_eval/proxy'"),
            "{env}"
        );
        assert!(
            env.contains("PROOF_HOLDOUT_STORE='/tmp/proof_eval/holdout'"),
            "{env}"
        );
        let extras = pod.extras.lock().expect("extras").clone();
        assert!(
            !extras.holdout_tar.is_empty(),
            "holdout shards must be staged"
        );
        assert!(
            extras.proxy_tar_path.is_some(),
            "proxy archive path must be staged"
        );
        assert!(
            *pod.proxy_tar_len.lock().expect("proxy len") > 0,
            "proxy archive must be non-empty on disk"
        );
        let req: serde_json::Value =
            serde_json::from_slice(&pod.request.lock().expect("req")).expect("json");
        assert!(req.get("api_key").is_none(), "{req}");
        let dump = req.to_string();
        assert!(!dump.contains("sk-live"), "{dump}");
        assert_eq!(req["base_url"], "http://127.0.0.1:8000/v1");
    }

    #[tokio::test]
    async fn harvest_without_judge_key_does_not_boot() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let topic = harvest_topic(&recs);
        let pod = CapturePod::new();
        let harvest = harvest_with_assets(pod.clone(), &recs, None);
        let err = harvest
            .score(
                &harvest_pin(),
                &topic,
                &harvest_offer(),
                "digest-abcdef",
                "artifact",
                &recs,
                "claim",
            )
            .await
            .expect_err("no key");
        assert!(matches!(err, EvalError::InferenceAuthMissing), "{err}");
        assert!(!*pod.booted.lock().expect("booted"));
    }

    #[tokio::test]
    async fn harvest_without_proxy_dir_does_not_boot() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let topic = harvest_topic(&recs);
        let pod = CapturePod::new();
        let harvest = LiumProofHarvest::new(
            pod.clone(),
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAtest proof".into()],
        )
        .with_judge_api_key(Some("sk-live-not-a-real-secret".into()));
        let err = harvest
            .score(
                &harvest_pin(),
                &topic,
                &harvest_offer(),
                "digest-abcdef",
                "artifact",
                &recs,
                "claim",
            )
            .await
            .expect_err("no proxy");
        assert!(matches!(err, EvalError::ProxyModelMissing), "{err}");
        assert!(!*pod.booted.lock().expect("booted"));
    }

    #[tokio::test]
    async fn harvest_empty_proxy_dir_does_not_boot() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (_proxy, store) = live_asset_dirs(&recs);
        let empty = store.parent().expect("root").join("empty-proxy-score");
        std::fs::create_dir_all(&empty).expect("empty");
        let pod = CapturePod::new();
        let harvest = LiumProofHarvest::new(
            pod.clone(),
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAtest proof".into()],
        )
        .with_judge_api_key(Some("sk-live-not-a-real-secret".into()))
        .with_proxy_model_dir(Some(empty))
        .with_holdout_store(Some(store));
        let err = harvest
            .score(
                &harvest_pin(),
                &harvest_topic(&recs),
                &harvest_offer(),
                "digest-abcdef",
                "artifact",
                &recs,
                "claim",
            )
            .await
            .expect_err("empty proxy");
        assert!(matches!(err, EvalError::ProxyModelMissing), "{err}");
        assert!(!*pod.booted.lock().expect("booted"));
    }

    fn leftover_proxy_tars() -> Vec<PathBuf> {
        let prefix = format!("proof-proxy-{}-", std::process::id());
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix))
                    && p.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("tar"))
            })
            .collect();
        paths.sort();
        paths
    }

    fn assert_live_extras_holdout_refuse_leaves_no_proxy_tar(
        harvest: &LiumProofHarvest,
        holdout: &[HoldoutRecord],
    ) {
        let before = leftover_proxy_tars();
        match harvest.live_extras(holdout) {
            Ok(_) => panic!("holdout refuse"),
            Err(err) => assert!(matches!(err, EvalError::HoldoutStoreMissing), "{err}"),
        }
        let after = leftover_proxy_tars();
        assert_eq!(after, before, "proxy tar leaked: {after:?}");
    }

    #[tokio::test]
    async fn missing_holdout_shard_does_not_leak_a_proxy_tar() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        let pod = CapturePod::new();
        let harvest = LiumProofHarvest::new(
            pod.clone(),
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAtest proof".into()],
        )
        .with_judge_api_key(Some("sk-live-not-a-real-secret".into()))
        .with_proxy_model_dir(Some(proxy))
        .with_holdout_store(Some(store));
        harvest.ready().expect("store is usable");
        let mut missing = recs.clone();
        missing[0].content_sha256 = "ab".repeat(32);
        let before = leftover_proxy_tars();
        let err = harvest
            .score(
                &harvest_pin(),
                &harvest_topic(&recs),
                &harvest_offer(),
                "digest-abcdef",
                "artifact",
                &missing,
                "claim",
            )
            .await
            .expect_err("missing shard");
        assert!(matches!(err, EvalError::HoldoutStoreMissing), "{err}");
        assert!(!*pod.booted.lock().expect("booted"));
        let after = leftover_proxy_tars();
        assert_eq!(after, before, "proxy tar leaked: {after:?}");
    }

    #[test]
    fn live_extras_holdout_refuse_does_not_leak_a_proxy_tar() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        let harvest = harvest_for_ready(proxy.clone(), store.clone());
        let mut missing = recs.clone();
        missing[0].content_sha256 = "ab".repeat(32);
        assert_live_extras_holdout_refuse_leaves_no_proxy_tar(&harvest, &missing);

        std::fs::write(
            store.join(recs[0].content_sha256.to_ascii_lowercase()),
            b"not-the-catalogued-shard\n",
        )
        .expect("tamper");
        let harvest = harvest_for_ready(proxy, store);
        assert_live_extras_holdout_refuse_leaves_no_proxy_tar(&harvest, &recs);
    }

    #[test]
    fn pack_holdout_tar_refuses_a_missing_shard() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let empty =
            std::env::temp_dir().join(format!("proof-empty-holdout-{}", std::process::id()));
        std::fs::create_dir_all(&empty).expect("dir");
        assert!(matches!(
            pack_holdout_tar(&empty, &recs),
            Err(EvalError::HoldoutStoreMissing)
        ));
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn pack_holdout_tar_refuses_a_hash_mismatch() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let store = std::env::temp_dir().join(format!(
            "proof-holdout-mismatch-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&store).expect("dir");
        std::fs::write(
            store.join(recs[0].content_sha256.to_ascii_lowercase()),
            b"not-the-catalogued-shard\n",
        )
        .expect("wrong bytes");
        assert!(matches!(
            pack_holdout_tar(&store, &recs),
            Err(EvalError::HoldoutStoreMissing)
        ));
        let _ = std::fs::remove_dir_all(&store);
    }

    #[test]
    fn pack_proxy_tar_refuses_a_missing_dir() {
        assert!(matches!(
            pack_proxy_tar(Path::new("/no/such/proof-proxy-model")),
            Err(EvalError::ProxyModelMissing)
        ));
    }

    #[test]
    fn pack_helpers_succeed_on_operator_staged_trees() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        let holdout = pack_holdout_tar(&store, &recs).expect("holdout tar");
        let weights = pack_proxy_tar(&proxy).expect("proxy tar");
        assert!(!holdout.is_empty());
        assert!(std::fs::metadata(&weights).expect("meta").len() > 0);
        let listed = Command::new("tar")
            .args(["-tf", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .as_mut()
                    .expect("stdin")
                    .write_all(&holdout)
                    .expect("write");
                child.wait_with_output()
            })
            .expect("tar -tf");
        let listing = String::from_utf8_lossy(&listed.stdout);
        assert!(
            listing.contains(&recs[0].content_sha256.to_ascii_lowercase()),
            "{listing}"
        );
        let _ = std::fs::remove_file(&weights);
    }

    fn harvest_for_ready(proxy: PathBuf, store: PathBuf) -> LiumProofHarvest {
        LiumProofHarvest::new(
            CapturePod::new(),
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAtest proof".into()],
        )
        .with_proxy_model_dir(Some(proxy))
        .with_holdout_store(Some(store))
    }

    #[test]
    fn ready_rejects_empty_proxy_dir() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (_proxy, store) = live_asset_dirs(&recs);
        let empty = store.parent().expect("root").join("empty-proxy");
        std::fs::create_dir_all(&empty).expect("empty proxy");
        let harvest = harvest_for_ready(empty, store);
        assert!(matches!(harvest.ready(), Err(EvalError::ProxyModelMissing)));
    }

    #[test]
    fn ready_rejects_empty_holdout_store() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        for rec in &recs {
            let _ = std::fs::remove_file(store.join(rec.content_sha256.to_ascii_lowercase()));
        }
        let harvest = harvest_for_ready(proxy, store);
        assert!(matches!(
            harvest.ready(),
            Err(EvalError::HoldoutStoreMissing)
        ));
    }

    #[test]
    fn ready_rejects_holdout_hash_mismatch() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        std::fs::write(
            store.join(recs[0].content_sha256.to_ascii_lowercase()),
            b"stale-or-wrong-holdout-bytes\n",
        )
        .expect("tamper");
        let harvest = harvest_for_ready(proxy, store);
        assert!(matches!(
            harvest.ready(),
            Err(EvalError::HoldoutStoreMissing)
        ));
    }

    #[test]
    fn ready_accepts_usable_staged_assets() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        let harvest = harvest_for_ready(proxy, store);
        harvest.ready().expect("usable assets");
    }

    #[tokio::test]
    async fn harvest_refuses_a_spoofed_topic_origin_before_boot() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let mut topic = harvest_topic(&recs);
        topic.inference.max_input_tokens = Some(4_096);
        topic.inference.base_url = Some("http://evil.example/v1".into());
        let pod = CapturePod::new();
        let harvest =
            harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()));
        let err = harvest
            .score(
                &harvest_pin(),
                &topic,
                &harvest_offer(),
                "digest-abcdef",
                "artifact",
                &recs,
                "claim",
            )
            .await
            .expect_err("spoof");
        assert!(err.to_string().contains("committed judge origin"), "{err}");
        assert!(!*pod.booted.lock().expect("booted"));
    }

    #[test]
    fn stdout_tail_keeps_the_last_8kib() {
        let fatal = "refused: no model: Qwen/Qwen3.8-0.6B";
        let stdout = format!("{}{fatal}", "x".repeat(10 * 1024));
        let tail = truncate_tail(&stdout, STDOUT_TAIL_BYTES);
        assert!(tail.starts_with('…'), "{tail}");
        assert!(tail.ends_with(fatal), "{tail}");
        assert!(tail.len() <= STDOUT_TAIL_BYTES + '…'.len_utf8());
    }

    struct StdoutPod {
        stdout: String,
        booted: std::sync::Mutex<bool>,
    }

    impl StdoutPod {
        fn new(stdout: impl Into<String>) -> Arc<Self> {
            Arc::new(Self {
                stdout: stdout.into(),
                booted: std::sync::Mutex::new(false),
            })
        }
    }

    #[async_trait]
    impl EvalPod for StdoutPod {
        async fn boot(&self, _spec: &InstanceSpec) -> Result<String, String> {
            *self.booted.lock().expect("boot") = true;
            Ok("pod-1".into())
        }

        async fn run(
            &self,
            _instance_id: &str,
            _request: &[u8],
            _env_file: &[u8],
            _extras: &RunExtras,
        ) -> Result<String, String> {
            Ok(self.stdout.clone())
        }

        async fn shutdown(&self, _instance_id: &str) -> Result<bool, String> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn harvest_refuses_stdout_without_ok_marker() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let topic = harvest_topic(&recs);
        let (proxy, store) = live_asset_dirs(&recs);
        let pod = StdoutPod::new("refused: no model: Qwen/Qwen3.8-0.6B\nexit=2\n");
        let harvest = LiumProofHarvest::new(
            pod.clone(),
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAtest proof".into()],
        )
        .with_judge_api_key(Some("sk-live-not-a-real-secret".into()))
        .with_proxy_model_dir(Some(proxy))
        .with_holdout_store(Some(store));
        let err = harvest
            .score(
                &harvest_pin(),
                &topic,
                &harvest_offer(),
                "digest-abcdef",
                "artifact",
                &recs,
                "claim",
            )
            .await
            .expect_err("no ok");
        assert!(
            matches!(err, EvalError::Backend(ref m) if m.contains(OK_MARKER)),
            "{err}"
        );
        assert!(*pod.booted.lock().expect("booted"));
    }
}
