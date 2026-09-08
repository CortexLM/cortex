//! Proof live harvest: score an artifact on a digest-pinned `proof-eval` image.
//!
//! The RLM agent and the metric harness live inside the image. This crate
//! boots that image on a Lium pod, hands it the run request (including the
//! live `InferenceOffer` judge backend and the topic constraints the image
//! must enforce — e.g. a 12.5 Gbit/s cap), reads back the metrics document,
//! and tears the pod down. Nothing here computes a score. Miners do not bind
//! or train against the offer. There is no sim fallback.
//!
//! **Where** the image runs is the live `EvalExecutorOffer`: the pod is
//! rented on that offer's Lium template, at exactly the pinned `1x` width
//! (any other rent width aborts before the rent), and the run is held to the
//! resolved proof deadline (offer, tightened by the topic, or an operator
//! `PROOF_HARVEST_*` override) both by the pod-side `timeout` and by this
//! crate's wait. A run cut at the deadline is a **503** carrying the pod's
//! stdout tail, never a zero.
//!
//! This crate is the **only** path from the control plane to a rented GPU.
//! The control-plane host runs neither the eval image nor the RLM judge; the
//! judge is a remote offer the image calls from the pod, and the executor is
//! a remote machine class. Nothing here knows a topic beyond the signed
//! document it is handed: no topic ids, benchmarks, or model names are
//! compiled in.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::too_many_arguments
)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use harvest_pod::{
    hit_deadline, killed_externally, truncate_tail, EvalPod, PodProgram, RunExtras, HOLDOUT_DIR,
    PROXY_DIR,
};
use prism_lium_types::InstanceSpec;
use proof_eval::{
    map_executor_err, secret_backed_base_url, EvalError, LiveScorer, ProofEvalDocument,
    PROOF_METRICS_SCHEMA,
};
use proof_executor::{executor_plan, EvalExecutorOffer, ExecutorPlan, HarvestOverrides};
use proof_task::{
    resolve_inference, HoldoutRecord, InferenceOffer, ProofPin, TopicDocument, CHALLENGE_ID,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Prefix the eval image prints before its metrics document.
pub const METRICS_MARKER: &str = "PROOF_METRICS=";

/// Marker the eval image prints on a completed run.
pub const OK_MARKER: &str = "PROOF_EVAL_OK";

/// Bytes of pod stdout retained on a missing [`OK_MARKER`] or deadline refuse.
const STDOUT_TAIL_BYTES: usize = 8 * 1024;

/// Seconds the harvest waits past the proof deadline for the pod-side
/// `timeout --kill-after=60` and the SSH round trip to report back before it
/// stops waiting and tears the pod down anyway.
pub const DEADLINE_WAIT_GRACE_SECS: u64 = 300;

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
    /// Live executor offer the pod was rented on (host stamp).
    pub executor_offer_id: String,
    /// The offer's published `config_commitment`.
    pub executor_offer_commitment: String,
    /// Commitment of the executor configuration this run actually got
    /// (template, shape, `max_proof_deadline_s`, digest). Equals the offer
    /// commitment unless a topic tighten or an operator override changed it.
    pub executor_commitment: String,
    /// Proof deadline this run is held to, seconds.
    pub max_proof_deadline_s: u64,
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

impl HarvestRequest {
    /// Resolve the judge for `topic` against the committed offer and bind the
    /// run to the executor plan. Refuses (no rent) when the offer cannot
    /// serve the topic, the resolved judge config is incomplete, or a topic
    /// tried to redirect the judge origin.
    pub fn build(
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        plan: &ExecutorPlan,
        frozen_digest: &str,
        artifact_digest: &str,
        holdout: &[HoldoutRecord],
        claim: &str,
    ) -> Result<Self, EvalError> {
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
        Ok(Self {
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
            max_input_tokens: resolved.max_input_tokens.min(offer.config.max_input_tokens),
            max_output_tokens: resolved
                .max_output_tokens
                .min(offer.config.max_output_tokens),
            config_commitment: offer.config_commitment.clone(),
            executor_offer_id: plan.offer_id.clone(),
            executor_offer_commitment: plan.offer_commitment.clone(),
            executor_commitment: plan.config_commitment.clone(),
            max_proof_deadline_s: plan.deadline_s,
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: topic.holdout_commitment.clone(),
            constraints: topic.constraints.clone(),
            flops_budget: topic.flops_budget,
            wall_budget_s: topic.metric.wall_budget_s,
            claim: claim.to_owned(),
            holdout: holdout.to_vec(),
        })
    }
}

/// Rent limits for one harvest. Width is not a limit: it is the executor
/// plan's exact `1x`, and any other rent aborts.
#[derive(Debug, Clone)]
pub struct HarvestLimits {
    /// Max pod lifetime hours.
    pub max_lifetime_hours: f64,
    /// Max USD per GPU-hour.
    pub max_price_per_hour: f64,
}

impl Default for HarvestLimits {
    fn default() -> Self {
        Self {
            max_lifetime_hours: 6.0,
            max_price_per_hour: 12.0,
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
    pack_proxy_tar_to(dir, &std::env::temp_dir())
}

fn next_proxy_tar_path(dest_dir: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    dest_dir.join(format!(
        "proof-proxy-{}-{nanos}-{seq}.tar",
        std::process::id()
    ))
}

fn pack_proxy_tar_to(dir: &Path, dest_dir: &Path) -> Result<PathBuf, EvalError> {
    proxy_archive_usable(dir)?;
    let dest = next_proxy_tar_path(dest_dir);
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
    /// Host directory that receives the staged `proof-proxy-*.tar`.
    ///
    /// Defaults to the process temp dir. Tests override this so leftover
    /// scans do not race sibling unit tests that share `/tmp`.
    proxy_tar_dir: PathBuf,
    /// Operator `PROOF_HARVEST_*` hot-swap. `None` reads the process env at
    /// score time; tests inject a value so they never touch the env.
    overrides: Option<HarvestOverrides>,
    /// Seconds past the deadline the wait tolerates ([`DEADLINE_WAIT_GRACE_SECS`]).
    deadline_wait_grace_secs: u64,
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
            proxy_tar_dir: std::env::temp_dir(),
            overrides: None,
            deadline_wait_grace_secs: DEADLINE_WAIT_GRACE_SECS,
        }
    }

    /// Pin the operator overrides instead of reading `PROOF_HARVEST_*` env.
    #[must_use]
    pub fn with_harvest_overrides(mut self, overrides: Option<HarvestOverrides>) -> Self {
        self.overrides = overrides;
        self
    }

    /// Tighten the post-deadline wait grace (tests).
    #[cfg(test)]
    #[must_use]
    fn with_deadline_wait_grace_secs(mut self, secs: u64) -> Self {
        self.deadline_wait_grace_secs = secs;
        self
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

    /// Host directory for the staged proxy tar (`proof-proxy-*.tar`).
    #[cfg(test)]
    #[must_use]
    fn with_proxy_tar_dir(mut self, dir: PathBuf) -> Self {
        if !dir.as_os_str().is_empty() {
            self.proxy_tar_dir = dir;
        }
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
        let proxy_path = pack_proxy_tar_to(proxy, &self.proxy_tar_dir)?;
        let guard = ProxyTarGuard(Some(proxy_path.clone()));
        Ok((
            RunExtras {
                holdout_tar,
                proxy_tar_path: Some(proxy_path),
            },
            guard,
        ))
    }

    /// Rent spec for one run: the executor plan's digest-scoped template at
    /// exactly its width. `template_id` stays `None` on purpose — the
    /// provider would rent a raw id verbatim — so the digest-bound resolver
    /// is the only way a template is chosen: it reuses a listed template only
    /// when its image is `eval_image@digest`, and otherwise creates one bound
    /// to that pin.
    fn spec(&self, pin: &ProofPin, frozen_digest: &str, plan: &ExecutorPlan) -> InstanceSpec {
        InstanceSpec {
            name: format!("proof-{}", &frozen_digest[..12.min(frozen_digest.len())]),
            max_lifetime_hours: self.limits.max_lifetime_hours,
            max_price_per_hour: self.limits.max_price_per_hour,
            gpu_count: plan.gpu_count,
            image_digest: Some(pin.eval_image_digest.clone()),
            docker_image: Some(pin.eval_image.clone()),
            startup_commands: None,
            ssh_public_keys: self.ssh_public_keys.clone(),
            ssh_key_name: Some(SSH_KEY_NAME.to_owned()),
            preferred_offer_id: None,
            template_id: None,
            template_name: Some(plan.template_id.clone()),
            exact_gpu_count: true,
        }
    }

    /// Stage, then run under the deadline, then always tear down. The run
    /// wait is bounded here too: a pod that never reports back is not a
    /// reason to hold the submission open past the deadline.
    async fn run_on_pod(
        &self,
        instance: &str,
        body: &[u8],
        env: &[u8],
        extras: &RunExtras,
        deadline_s: u64,
    ) -> Result<String, EvalError> {
        let outcome = if let Err(e) = self.pod.stage(instance, body, env, extras).await {
            Err(EvalError::Backend(e))
        } else {
            let wait =
                Duration::from_secs(deadline_s.saturating_add(self.deadline_wait_grace_secs));
            match tokio::time::timeout(wait, self.pod.run(instance, Some(deadline_s))).await {
                Ok(Ok(stdout)) => Ok(stdout),
                Ok(Err(e)) => Err(EvalError::Backend(e)),
                Err(_elapsed) => Err(EvalError::ProofDeadlineExceeded {
                    deadline_s,
                    stdout_tail: format!(
                        "harvest wait exceeded the {deadline_s}s deadline (+{}s grace); \
                         no pod stdout",
                        self.deadline_wait_grace_secs
                    ),
                }),
            }
        };
        match self.pod.shutdown(instance).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(EvalError::Integrity(format!(
                    "pod {instance} terminate not verified"
                )))
            }
            Err(e) => return Err(EvalError::Backend(e)),
        }
        let stdout = outcome?;
        if hit_deadline(&stdout) {
            let stdout_tail = truncate_tail(&stdout, STDOUT_TAIL_BYTES);
            tracing::warn!(
                instance,
                deadline_s,
                stdout_tail = %stdout_tail,
                "eval run cut at the proof deadline; refusing"
            );
            return Err(EvalError::ProofDeadlineExceeded {
                deadline_s,
                stdout_tail,
            });
        }
        Ok(stdout)
    }
}

#[async_trait]
impl LiveScorer for LiumProofHarvest {
    /// Pin ceilings + open offer + topic tighten + this host's
    /// `PROOF_HARVEST_*` overrides, collapsed into one rent. Refuses before
    /// anything is staged or rented: no judge env, no proxy tar, no pod.
    fn plan(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        executor: &EvalExecutorOffer,
    ) -> Result<ExecutorPlan, EvalError> {
        let overrides = match &self.overrides {
            Some(o) => o.clone(),
            None => HarvestOverrides::from_env().map_err(map_executor_err)?,
        };
        if !overrides.is_empty() {
            tracing::info!(?overrides, "PROOF_HARVEST_* override in effect");
        }
        executor_plan(pin, Some(executor), topic, &overrides).map_err(map_executor_err)
    }

    async fn score(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        plan: &ExecutorPlan,
        frozen_digest: &str,
        artifact_digest: &str,
        // The digest-pinned image fetches by digest from the artifact store
        // and its agent observes usage against the topic budget; the miner
        // locator and declaration are custom-family concerns.
        _artifact_uri: Option<&str>,
        _declared_flops: u64,
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
        // The plan is what `Self::plan` resolved for this topic; a plan for
        // another topic is a caller bug and must not rent.
        if plan.topic_id != topic.id {
            return Err(EvalError::ExecutorOffer(format!(
                "executor plan is for topic {:?}, scoring {:?}",
                plan.topic_id, topic.id
            )));
        }
        let request = HarvestRequest::build(
            pin,
            topic,
            offer,
            plan,
            frozen_digest,
            artifact_digest,
            holdout,
            claim,
        )?;
        let env = judge_teacher_env(self.judge_api_key.as_deref().unwrap_or(""))?;
        let (extras, _proxy_guard) = self.live_extras(holdout)?;
        let body = serde_json::to_vec(&request)
            .map_err(|e| EvalError::Backend(format!("encode request: {e}")))?;

        tracing::info!(
            executor_offer_id = %plan.offer_id,
            topic_id = %plan.topic_id,
            template_id = %plan.template_id,
            gpu_count = plan.gpu_count,
            deadline_s = plan.deadline_s,
            overridden = plan.overridden,
            "proof harvest rent plan"
        );
        let instance = self
            .pod
            .boot(&self.spec(pin, frozen_digest, plan))
            .await
            .map_err(EvalError::Backend)?;
        let stdout = self
            .run_on_pod(&instance, &body, &env, &extras, plan.deadline_s)
            .await?;
        if !PROGRAM.ran_to_completion(&stdout) {
            let stdout_tail = truncate_tail(&stdout, STDOUT_TAIL_BYTES);
            // An external SIGKILL (GPU OOM, host pressure) is an
            // infrastructure failure, not the proof deadline; name it so the
            // operator does not chase the wrong budget.
            let what = if killed_externally(&stdout) {
                "eval image was SIGKILLed before the deadline (exit=137: external kill such as OOM)"
                    .to_owned()
            } else {
                format!("eval image did not print {OK_MARKER}")
            };
            tracing::warn!(instance, stdout_tail = %stdout_tail, "{what}; refusing");
            return Err(EvalError::Backend(format!(
                "{what}; stdout_tail: {stdout_tail}"
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

    /// What `eval_after_freeze` does on the Lium path: resolve the rent plan
    /// with this harvest's overrides, then score under it.
    async fn score_via_plan(
        harvest: &LiumProofHarvest,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        executor: &EvalExecutorOffer,
        frozen: &str,
        artifact: &str,
        holdout: &[HoldoutRecord],
        claim: &str,
    ) -> Result<ProofEvalDocument, EvalError> {
        let plan = harvest.plan(pin, topic, executor)?;
        harvest
            .score(
                pin, topic, offer, &plan, frozen, artifact, None, 1, holdout, claim,
            )
            .await
    }

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
                ..proof_task::Constraints::default()
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
            executor_offer_id: "lium-1x-v0".into(),
            executor_offer_commitment: "cd".repeat(32),
            executor_commitment: "cd".repeat(32),
            max_proof_deadline_s: 3_600,
            eval_image_digest: String::new(),
            holdout_commitment: topic.holdout_commitment.clone(),
            constraints: topic.constraints.clone(),
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
        assert_eq!(v["executor_offer_id"], "lium-1x-v0");
        assert_eq!(v["max_proof_deadline_s"], 3_600);
        assert!(v.get("proxy_model").is_none());
        assert!(v.get("api_key").is_none());
        assert!(v.get("lium_api_key").is_none());
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
        spec: std::sync::Mutex<Option<InstanceSpec>>,
        /// Deadline the harvest handed to `run` (the harvest always passes one).
        run_deadline: std::sync::Mutex<Option<u64>>,
        shutdowns: std::sync::Mutex<u32>,
    }

    impl CapturePod {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                env: std::sync::Mutex::new(Vec::new()),
                request: std::sync::Mutex::new(Vec::new()),
                extras: std::sync::Mutex::new(RunExtras::default()),
                proxy_tar_len: std::sync::Mutex::new(0),
                booted: std::sync::Mutex::new(false),
                spec: std::sync::Mutex::new(None),
                run_deadline: std::sync::Mutex::new(None),
                shutdowns: std::sync::Mutex::new(0),
            })
        }

        fn spec(&self) -> InstanceSpec {
            self.spec
                .lock()
                .expect("spec")
                .clone()
                .expect("booted spec")
        }
    }

    #[async_trait]
    impl EvalPod for CapturePod {
        async fn boot(&self, spec: &InstanceSpec) -> Result<String, String> {
            *self.booted.lock().expect("boot") = true;
            *self.spec.lock().expect("spec") = Some(spec.clone());
            Ok("pod-1".into())
        }

        async fn stage(
            &self,
            _instance_id: &str,
            request: &[u8],
            env_file: &[u8],
            extras: &RunExtras,
        ) -> Result<(), String> {
            *self.request.lock().expect("req") = request.to_vec();
            *self.env.lock().expect("env") = env_file.to_vec();
            *self.extras.lock().expect("extras") = extras.clone();
            let n = extras
                .proxy_tar_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map_or(0, |m| m.len());
            *self.proxy_tar_len.lock().expect("proxy len") = n;
            Ok(())
        }

        async fn run(
            &self,
            _instance_id: &str,
            deadline_secs: Option<u64>,
        ) -> Result<String, String> {
            *self.run_deadline.lock().expect("deadline") = deadline_secs;
            Err("captured".into())
        }

        async fn shutdown(&self, _instance_id: &str) -> Result<bool, String> {
            *self.shutdowns.lock().expect("shutdowns") += 1;
            Ok(true)
        }
    }

    fn harvest_pin() -> ProofPin {
        let mut p = ProofPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            topic_pubkey: "ab".repeat(32),
            allowed_lium_template_prefixes: vec!["proof-eval-".into()],
            ..ProofPin::default()
        };
        p.inference.model = "master-proxy-v0".into();
        p
    }

    /// Open `1x` executor on the digest-scoped template of [`harvest_pin`].
    fn harvest_executor() -> EvalExecutorOffer {
        let mut o = EvalExecutorOffer {
            offer_id: "lium-1x-v0".into(),
            lium_template_id: "proof-eval-abababababab".into(),
            machine_shape: "1x".into(),
            max_proof_deadline_s: 3_600,
            eval_image_digest: harvest_pin().eval_image_digest,
            config_commitment: String::new(),
            status: proof_executor::OfferStatus::Open,
        };
        o.config_commitment = o.expected_commitment();
        o
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
        // Deterministic: never read PROOF_HARVEST_* from the test process env.
        .with_harvest_overrides(Some(HarvestOverrides::default()))
    }

    #[tokio::test]
    async fn harvest_stages_teacher_env_and_never_puts_the_key_on_the_request() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let topic = harvest_topic(&recs);
        let pod = CapturePod::new();
        let harvest =
            harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()));
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &topic,
            &harvest_offer(),
            &harvest_executor(),
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
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &topic,
            &harvest_offer(),
            &harvest_executor(),
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
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &topic,
            &harvest_offer(),
            &harvest_executor(),
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
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &harvest_topic(&recs),
            &harvest_offer(),
            &harvest_executor(),
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

    fn leftover_proxy_tars(dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("proof-proxy-"))
                    && p.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("tar"))
            })
            .collect();
        paths.sort();
        paths
    }

    fn proxy_tar_staging(proxy: &Path) -> PathBuf {
        let dir = proxy.parent().expect("root").join("proxy-tars");
        std::fs::create_dir_all(&dir).expect("staging");
        dir
    }

    fn assert_live_extras_holdout_refuse_leaves_no_proxy_tar(
        harvest: &LiumProofHarvest,
        holdout: &[HoldoutRecord],
        staging: &Path,
    ) {
        let before = leftover_proxy_tars(staging);
        match harvest.live_extras(holdout) {
            Ok(_) => panic!("holdout refuse"),
            Err(err) => assert!(matches!(err, EvalError::HoldoutStoreMissing), "{err}"),
        }
        let after = leftover_proxy_tars(staging);
        assert_eq!(after, before, "proxy tar leaked: {after:?}");
    }

    #[tokio::test]
    async fn missing_holdout_shard_does_not_leak_a_proxy_tar() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        let staging = proxy_tar_staging(&proxy);
        let pod = CapturePod::new();
        let harvest = LiumProofHarvest::new(
            pod.clone(),
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAtest proof".into()],
        )
        .with_judge_api_key(Some("sk-live-not-a-real-secret".into()))
        .with_proxy_model_dir(Some(proxy))
        .with_holdout_store(Some(store))
        .with_proxy_tar_dir(staging.clone());
        harvest.ready().expect("store is usable");
        let mut missing = recs.clone();
        missing[0].content_sha256 = "ab".repeat(32);
        let before = leftover_proxy_tars(&staging);
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &harvest_topic(&recs),
            &harvest_offer(),
            &harvest_executor(),
            "digest-abcdef",
            "artifact",
            &missing,
            "claim",
        )
        .await
        .expect_err("missing shard");
        assert!(matches!(err, EvalError::HoldoutStoreMissing), "{err}");
        assert!(!*pod.booted.lock().expect("booted"));
        let after = leftover_proxy_tars(&staging);
        assert_eq!(after, before, "proxy tar leaked: {after:?}");
    }

    #[test]
    fn live_extras_holdout_refuse_does_not_leak_a_proxy_tar() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        let staging = proxy_tar_staging(&proxy);
        let harvest =
            harvest_for_ready(proxy.clone(), store.clone()).with_proxy_tar_dir(staging.clone());
        let mut missing = recs.clone();
        missing[0].content_sha256 = "ab".repeat(32);
        assert_live_extras_holdout_refuse_leaves_no_proxy_tar(&harvest, &missing, &staging);

        std::fs::write(
            store.join(recs[0].content_sha256.to_ascii_lowercase()),
            b"not-the-catalogued-shard\n",
        )
        .expect("tamper");
        let harvest = harvest_for_ready(proxy, store).with_proxy_tar_dir(staging.clone());
        assert_live_extras_holdout_refuse_leaves_no_proxy_tar(&harvest, &recs, &staging);
    }

    #[test]
    fn live_extras_guard_deletes_the_proxy_tar_in_the_configured_dir() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        let staging = proxy_tar_staging(&proxy);
        let harvest = harvest_for_ready(proxy, store).with_proxy_tar_dir(staging.clone());
        {
            let (extras, _guard) = harvest.live_extras(&recs).expect("extras");
            let path = extras.proxy_tar_path.expect("path");
            assert!(path.starts_with(&staging), "{path:?}");
            assert!(path.is_file(), "{path:?}");
            assert_eq!(leftover_proxy_tars(&staging).len(), 1);
        }
        assert_eq!(leftover_proxy_tars(&staging), Vec::<PathBuf>::new());
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
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &topic,
            &harvest_offer(),
            &harvest_executor(),
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

        async fn stage(
            &self,
            _instance_id: &str,
            _request: &[u8],
            _env_file: &[u8],
            _extras: &RunExtras,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn run(
            &self,
            _instance_id: &str,
            _deadline_secs: Option<u64>,
        ) -> Result<String, String> {
            Ok(self.stdout.clone())
        }

        async fn shutdown(&self, _instance_id: &str) -> Result<bool, String> {
            Ok(true)
        }
    }

    /// Pod stdout for a run the wrapper ended: `124` on TERM, or `137` after
    /// `--kill-after` — the run command marks both with the deadline line.
    fn deadline_stdout(rc: u32) -> String {
        format!(
            "{}\nexit={rc}\n{}\nTraceback: still training step 4200 when the proof deadline hit\n",
            "boot ok\n".repeat(3),
            harvest_pod::DEADLINE_MARKER
        )
    }

    #[tokio::test]
    async fn harvest_rents_the_executor_template_at_exactly_one_gpu_under_its_deadline() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let pod = CapturePod::new();
        let harvest =
            harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()))
                .with_harvest_overrides(Some(HarvestOverrides::default()));
        let mut topic = harvest_topic(&recs);
        topic.eval_executor.max_proof_deadline_s = Some(1_800);
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &topic,
            &harvest_offer(),
            &harvest_executor(),
            "digest-abcdef",
            "artifact",
            &recs,
            "claim",
        )
        .await
        .expect_err("capture");
        assert!(matches!(err, EvalError::Backend(_)), "{err}");
        let spec = pod.spec();
        assert_eq!(spec.gpu_count, 1);
        assert!(spec.exact_gpu_count, "any other rent width must abort");
        assert_eq!(
            spec.template_id, None,
            "a template name is resolved, not rented verbatim"
        );
        assert_eq!(
            spec.template_name.as_deref(),
            Some("proof-eval-abababababab")
        );
        assert_eq!(
            spec.image_digest.as_deref(),
            Some(harvest_pin().eval_image_digest.as_str())
        );
        assert_eq!(
            *pod.run_deadline.lock().expect("deadline"),
            Some(1_800),
            "topic tightens the offer deadline and the pod run is held to it"
        );
        assert_eq!(*pod.shutdowns.lock().expect("shutdowns"), 1);
        let req: serde_json::Value =
            serde_json::from_slice(&pod.request.lock().expect("req")).expect("json");
        assert_eq!(req["executor_offer_id"], "lium-1x-v0");
        assert_eq!(
            req["executor_offer_commitment"],
            harvest_executor().config_commitment
        );
        assert_eq!(
            req["executor_commitment"],
            proof_executor::executor_config_commitment(
                "proof-eval-abababababab",
                "1x",
                1_800,
                &harvest_pin().eval_image_digest
            ),
            "the request commits the executed configuration (topic-tightened 1800s)"
        );
        assert_eq!(req["max_proof_deadline_s"], 1_800);
    }

    /// A raw Lium template UUID would be rented verbatim by the provider, so
    /// the digest-bound resolver could never check its image: refused before
    /// boot even when the pin allowlist is empty (offer or env override).
    #[tokio::test]
    async fn harvest_refuses_a_raw_uuid_template_from_offer_or_override() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let mut pin = harvest_pin();
        pin.allowed_lium_template_prefixes.clear();
        let mut executor = harvest_executor();
        executor.lium_template_id = "f2f5e84c-3b09-4090-be83-1913eabd009e".into();
        executor.config_commitment = executor.expected_commitment();
        let pod = CapturePod::new();
        let harvest =
            harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()));
        let err = score_via_plan(
            &harvest,
            &pin,
            &harvest_topic(&recs),
            &harvest_offer(),
            &executor,
            "digest-abcdef",
            "artifact",
            &recs,
            "claim",
        )
        .await
        .expect_err("raw uuid offer");
        assert!(
            matches!(err, EvalError::ExecutorOffer(ref m) if m.contains("raw Lium template id")),
            "{err}"
        );
        assert!(!*pod.booted.lock().expect("booted"));

        let pod = CapturePod::new();
        let harvest =
            harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()))
                .with_harvest_overrides(Some(HarvestOverrides {
                    template_id: Some("f2f5e84c-3b09-4090-be83-1913eabd009e".into()),
                    ..HarvestOverrides::default()
                }));
        let err = score_via_plan(
            &harvest,
            &pin,
            &harvest_topic(&recs),
            &harvest_offer(),
            &harvest_executor(),
            "digest-abcdef",
            "artifact",
            &recs,
            "claim",
        )
        .await
        .expect_err("raw uuid override");
        assert!(
            matches!(err, EvalError::ExecutorOffer(ref m) if m.contains("raw Lium template id")),
            "{err}"
        );
        assert!(!*pod.booted.lock().expect("booted"));
    }

    /// A topic that pinned the offer commitment never runs under an
    /// operator override that changes the template or deadline; the same
    /// override on an unpinned topic runs and is re-committed as what ran.
    #[tokio::test]
    async fn harvest_refuses_config_changing_override_when_the_topic_pins_the_commitment() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let mut pinned = harvest_topic(&recs);
        pinned.eval_executor.require_offer_commitment = Some(harvest_executor().config_commitment);
        let override_ = HarvestOverrides {
            deadline_secs: Some(600),
            ..HarvestOverrides::default()
        };
        let pod = CapturePod::new();
        let harvest =
            harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()))
                .with_harvest_overrides(Some(override_.clone()));
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &pinned,
            &harvest_offer(),
            &harvest_executor(),
            "digest-abcdef",
            "artifact",
            &recs,
            "claim",
        )
        .await
        .expect_err("pinned topic");
        assert!(
            matches!(err, EvalError::ExecutorOffer(ref m) if m.contains("pins the offer config_commitment")),
            "{err}"
        );
        assert!(!*pod.booted.lock().expect("booted"), "must not rent");

        let pod = CapturePod::new();
        let harvest =
            harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()))
                .with_harvest_overrides(Some(override_));
        let _ = score_via_plan(
            &harvest,
            &harvest_pin(),
            &harvest_topic(&recs),
            &harvest_offer(),
            &harvest_executor(),
            "digest-abcdef",
            "artifact",
            &recs,
            "claim",
        )
        .await;
        assert!(*pod.booted.lock().expect("booted"));
        let req: serde_json::Value =
            serde_json::from_slice(&pod.request.lock().expect("req")).expect("json");
        assert_eq!(req["max_proof_deadline_s"], 600);
        assert_eq!(
            req["executor_offer_commitment"],
            harvest_executor().config_commitment
        );
        assert_ne!(
            req["executor_commitment"], req["executor_offer_commitment"],
            "the run is stamped with what actually ran, not the offer's knobs"
        );
        assert_eq!(
            req["executor_commitment"],
            proof_executor::executor_config_commitment(
                "proof-eval-abababababab",
                "1x",
                600,
                &harvest_pin().eval_image_digest
            )
        );
    }

    /// `exit=137` without the wrapper's deadline marker is an external
    /// SIGKILL (GPU OOM): a backend refusal that names it, not a deadline 503.
    #[tokio::test]
    async fn an_external_sigkill_is_not_reported_as_the_deadline() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        let pod = StdoutPod::new("boot ok\nexit=137\nCUDA error: out of memory\n");
        let harvest = LiumProofHarvest::new(
            pod.clone(),
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAtest proof".into()],
        )
        .with_judge_api_key(Some("sk-live-not-a-real-secret".into()))
        .with_proxy_model_dir(Some(proxy))
        .with_holdout_store(Some(store))
        .with_harvest_overrides(Some(HarvestOverrides::default()));
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &harvest_topic(&recs),
            &harvest_offer(),
            &harvest_executor(),
            "digest-abcdef",
            "artifact",
            &recs,
            "claim",
        )
        .await
        .expect_err("oom");
        assert!(
            matches!(err, EvalError::Backend(ref m) if m.contains("SIGKILLed before the deadline") && m.contains("out of memory")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn harvest_refuses_a_closed_wide_or_topic_mismatched_executor_before_boot() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let mut closed = harvest_executor();
        closed.status = proof_executor::OfferStatus::Closed;
        let mut wide = harvest_executor();
        wide.machine_shape = "8x".into();
        wide.config_commitment = wide.expected_commitment();
        let mut pinned_topic = harvest_topic(&recs);
        pinned_topic.eval_executor.require_offer_commitment = Some("cd".repeat(32));
        for (label, executor, topic, want) in [
            ("closed", closed, harvest_topic(&recs), "closed"),
            ("8x", wide, harvest_topic(&recs), "machine_shape"),
            (
                "topic pins another executor",
                harvest_executor(),
                pinned_topic,
                "cannot serve",
            ),
        ] {
            let pod = CapturePod::new();
            let harvest =
                harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()))
                    .with_harvest_overrides(Some(HarvestOverrides::default()));
            let err = score_via_plan(
                &harvest,
                &harvest_pin(),
                &topic,
                &harvest_offer(),
                &executor,
                "digest-abcdef",
                "artifact",
                &recs,
                "claim",
            )
            .await
            .expect_err(label);
            assert!(err.to_string().contains(want), "{label}: {err}");
            assert!(
                !*pod.booted.lock().expect("booted"),
                "{label} must not rent"
            );
        }
    }

    #[tokio::test]
    async fn harvest_env_override_aborts_any_width_but_one_and_never_loosens_the_ceiling() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        for (overrides, want) in [
            (
                HarvestOverrides {
                    gpu_count: Some(8),
                    ..HarvestOverrides::default()
                },
                "abort: executor would rent 8x",
            ),
            (
                HarvestOverrides {
                    deadline_secs: Some(7_201),
                    ..HarvestOverrides::default()
                },
                "max_proof_deadline_s = 7201",
            ),
            (
                HarvestOverrides {
                    template_id: Some("prism-recipe-v10".into()),
                    ..HarvestOverrides::default()
                },
                "does not carry the pinned eval image digest prefix",
            ),
            (
                HarvestOverrides {
                    template_id: Some("other-abababababab".into()),
                    ..HarvestOverrides::default()
                },
                "allowed_lium_template_prefixes",
            ),
        ] {
            let pod = CapturePod::new();
            let harvest =
                harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()))
                    .with_harvest_overrides(Some(overrides));
            let err = score_via_plan(
                &harvest,
                &harvest_pin(),
                &harvest_topic(&recs),
                &harvest_offer(),
                &harvest_executor(),
                "digest-abcdef",
                "artifact",
                &recs,
                "claim",
            )
            .await
            .expect_err("override refused");
            assert!(
                matches!(err, EvalError::ExecutorOffer(ref m) if m.contains(want)),
                "{err}"
            );
            assert!(!*pod.booted.lock().expect("booted"));
        }

        // A legal hot-swap: same width, shorter deadline, another allowed template.
        let pod = CapturePod::new();
        let harvest =
            harvest_with_assets(pod.clone(), &recs, Some("sk-live-not-a-real-secret".into()))
                .with_harvest_overrides(Some(HarvestOverrides {
                    template_id: Some("proof-eval-abababababab-hotfix".into()),
                    gpu_count: Some(1),
                    deadline_secs: Some(600),
                }));
        let _ = score_via_plan(
            &harvest,
            &harvest_pin(),
            &harvest_topic(&recs),
            &harvest_offer(),
            &harvest_executor(),
            "digest-abcdef",
            "artifact",
            &recs,
            "claim",
        )
        .await;
        assert!(*pod.booted.lock().expect("booted"));
        assert_eq!(
            pod.spec().template_name.as_deref(),
            Some("proof-eval-abababababab-hotfix")
        );
        assert_eq!(*pod.run_deadline.lock().expect("deadline"), Some(600));
    }

    #[tokio::test]
    async fn a_run_cut_at_the_deadline_is_a_503_with_the_stdout_tail() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        for rc in [
            harvest_pod::DEADLINE_EXIT_CODE,
            harvest_pod::SIGKILL_EXIT_CODE,
        ] {
            let (proxy, store) = live_asset_dirs(&recs);
            let pod = StdoutPod::new(deadline_stdout(rc));
            let harvest = LiumProofHarvest::new(
                pod.clone(),
                HarvestLimits::default(),
                vec!["ssh-ed25519 AAAAtest proof".into()],
            )
            .with_judge_api_key(Some("sk-live-not-a-real-secret".into()))
            .with_proxy_model_dir(Some(proxy))
            .with_holdout_store(Some(store))
            .with_harvest_overrides(Some(HarvestOverrides::default()));
            let err = score_via_plan(
                &harvest,
                &harvest_pin(),
                &harvest_topic(&recs),
                &harvest_offer(),
                &harvest_executor(),
                "digest-abcdef",
                "artifact",
                &recs,
                "claim",
            )
            .await
            .expect_err("deadline");
            match err {
                EvalError::ProofDeadlineExceeded {
                    deadline_s,
                    stdout_tail,
                } => {
                    assert_eq!(deadline_s, 3_600);
                    assert!(stdout_tail.contains(&format!("exit={rc}")), "{stdout_tail}");
                    assert!(stdout_tail.contains("step 4200"), "{stdout_tail}");
                }
                other => panic!("exit={rc}: expected deadline refuse, got {other}"),
            }
            assert!(*pod.booted.lock().expect("booted"));
        }
    }

    struct HangingPod {
        shutdowns: std::sync::Mutex<u32>,
    }

    #[async_trait]
    impl EvalPod for HangingPod {
        async fn boot(&self, _spec: &InstanceSpec) -> Result<String, String> {
            Ok("pod-hang".into())
        }

        async fn stage(
            &self,
            _instance_id: &str,
            _request: &[u8],
            _env_file: &[u8],
            _extras: &RunExtras,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn run(
            &self,
            _instance_id: &str,
            _deadline_secs: Option<u64>,
        ) -> Result<String, String> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok("PROOF_EVAL_OK\n".into())
        }

        async fn shutdown(&self, _instance_id: &str) -> Result<bool, String> {
            *self.shutdowns.lock().expect("shutdowns") += 1;
            Ok(true)
        }
    }

    #[tokio::test]
    async fn a_pod_that_never_reports_back_is_torn_down_at_the_deadline() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (proxy, store) = live_asset_dirs(&recs);
        let pod = Arc::new(HangingPod {
            shutdowns: std::sync::Mutex::new(0),
        });
        let mut executor = harvest_executor();
        executor.max_proof_deadline_s = 1;
        executor.config_commitment = executor.expected_commitment();
        let harvest = LiumProofHarvest::new(
            pod.clone(),
            HarvestLimits::default(),
            vec!["ssh-ed25519 AAAAtest proof".into()],
        )
        .with_judge_api_key(Some("sk-live-not-a-real-secret".into()))
        .with_proxy_model_dir(Some(proxy))
        .with_holdout_store(Some(store))
        .with_harvest_overrides(Some(HarvestOverrides::default()))
        .with_deadline_wait_grace_secs(0);
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &harvest_topic(&recs),
            &harvest_offer(),
            &executor,
            "digest-abcdef",
            "artifact",
            &recs,
            "claim",
        )
        .await
        .expect_err("wait elapsed");
        assert!(
            matches!(
                err,
                EvalError::ProofDeadlineExceeded { deadline_s: 1, ref stdout_tail }
                    if stdout_tail.contains("harvest wait exceeded")
            ),
            "{err}"
        );
        assert_eq!(
            *pod.shutdowns.lock().expect("shutdowns"),
            1,
            "the pod is terminated even when its run never returned"
        );
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
        let err = score_via_plan(
            &harvest,
            &harvest_pin(),
            &topic,
            &harvest_offer(),
            &harvest_executor(),
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
