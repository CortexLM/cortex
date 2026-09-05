//! Proof HTTP API (master-only).
//!
//! ```text
//! GET  /health
//! GET  /v1/status
//! GET  /v1/proof/topics
//! GET  /v1/proof/topics/{id}
//! POST /v1/submissions          miner submit (topic_id required)
//! GET  /v1/submissions
//! GET  /v1/submissions/{id}
//! POST /v1/admin/proof/topics   operator publish (signed document)
//! ```

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};

use proof_eval::{
    contamination_evidence, eval_after_freeze, force_sim, scoring_readiness,
    secret_backed_base_url, supported_custom, EvalBackend, EvalError, LiveScorer,
};
use proof_score::{
    judge_topic, primary_from_harness, AgentVerdict, GateFail, HarnessMetrics, MinerTopicRun,
    ProofKind, ProofVerdict,
};
use proof_store::{
    freeze_submission_digest, ArtifactManifest, MemoryStore, Submission, SubmissionState,
};
use proof_task::{
    resolve_inference, InferenceOffer, OfferError, ProofPin, TopicDocument, TopicError,
    TopicStatus, CHALLENGE_ID, SCORE_MAX, SCORING_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Shared HTTP state.
#[derive(Clone)]
pub struct AppState {
    /// Submission + topic store.
    pub store: MemoryStore,
    /// Global pin (floors + image + topic key).
    pub pin: ProofPin,
    /// Backend that is allowed to produce scores on this host.
    pub backend: EvalBackend,
    /// Harvest handle for the digest-pinned eval image. `None` on a live host
    /// means nothing can score, so submissions refuse.
    pub live_scorer: Option<Arc<dyn LiveScorer>>,
    /// Live RLM judge backend (operator state). Missing/closed → can_score false.
    pub offer: Option<InferenceOffer>,
    /// Judge API key from `PROOF_INFERENCE_API_KEY_FILE`. Never on `/v1/status`.
    pub judge_api_key: Option<String>,
    /// Operator bearer hashes (sha256 hex). Empty → admin 503.
    pub admin_hashes: Arc<Vec<String>>,
    /// Chain epoch used for topic windows. v0 hosts pass 0.
    pub epoch: u64,
}

impl AppState {
    fn live(&self) -> Option<&dyn LiveScorer> {
        self.live_scorer.as_deref()
    }

    fn can_score(&self) -> bool {
        let open = self.store.any_open_scorable(self.epoch).unwrap_or(false);
        if scoring_readiness(
            &self.pin,
            self.backend,
            self.live(),
            open,
            self.offer.as_ref(),
            self.judge_api_key.as_deref(),
        )
        .is_err()
        {
            return false;
        }
        let secret = secret_backed_base_url();
        self.store.topics().unwrap_or_default().iter().any(|t| {
            t.is_open_at(self.epoch)
                && resolve_inference(
                    &self.pin,
                    Some(&t.inference),
                    secret.as_deref(),
                    self.offer.as_ref(),
                )
                .ready_to_score()
        })
    }
}

/// Build the router.
pub fn proof_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/proof/topics", get(list_topics))
        .route("/v1/proof/topics/{id}", get(get_topic))
        .route("/v1/submissions", post(submit).get(list_subs))
        .route("/v1/submissions/{id}", get(get_sub))
        .route("/v1/admin/proof/topics", post(publish_topic))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "ok": true,
        "challenge_id": CHALLENGE_ID,
        "scoring_version": SCORING_VERSION,
    }))
}

async fn status(State(st): State<AppState>) -> impl IntoResponse {
    let open = st.store.open_ids(st.epoch).unwrap_or_default();
    let baseline_sealed = st.store.any_open_scorable(st.epoch).unwrap_or(false);
    Json(serde_json::json!({
        "challenge_id": CHALLENGE_ID,
        "scoring_version": SCORING_VERSION,
        "score_max": SCORE_MAX,
        "eval_image": st.pin.eval_image,
        "eval_image_digest": st.pin.eval_image_digest,
        "inference_offer": st.offer.as_ref().map(InferenceOffer::public_view),
        "inference": {
            "provider": st.pin.inference.provider.as_str(),
            "model": st.pin.inference.model,
            "mode": st.pin.inference.mode.as_str(),
            "max_input_tokens": st.pin.inference.max_input_tokens,
            "max_output_tokens": st.pin.inference.max_output_tokens,
        },
        "eval_backend": st.backend,
        "force_sim": force_sim(),
        "can_score": st.can_score(),
        "live_harvest_wired": st.live_scorer.is_some(),
        "baseline_sealed": baseline_sealed,
        "open_topics": open,
        "epoch": st.epoch,
    }))
}

async fn list_topics(State(st): State<AppState>) -> impl IntoResponse {
    let items = st.store.topics().unwrap_or_default();
    Json(serde_json::json!({ "items": items }))
}

async fn get_topic(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let doc = st
        .store
        .topic(&id)
        .map_err(|_| err(StatusCode::NOT_FOUND, "unknown topic"))?;
    Ok(Json(doc))
}

#[derive(Debug, Deserialize)]
struct SubmitBody {
    miner_hotkey: String,
    artifact_digest: String,
    artifact_uri: Option<String>,
    #[serde(default)]
    claim: String,
    #[serde(default)]
    declared_flops: u64,
    #[serde(default)]
    topic_id: String,
    /// Optional miner label. Not compared to an HF id (that check is retired).
    #[serde(default)]
    architecture: String,
    #[serde(default)]
    manifest: ArtifactManifest,
}

#[derive(Debug, Serialize)]
struct SubmitResp {
    id: String,
    submission_digest: String,
    topic_id: String,
    state: SubmissionState,
    eval_backend: EvalBackend,
    eligible: bool,
}

fn parse_hex64(s: &str, field: &str) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let t = s.trim().trim_start_matches("0x");
    if t.len() != 64 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(err(StatusCode::BAD_REQUEST, &format!("invalid {field}")));
    }
    Ok(t.to_ascii_lowercase())
}

fn nonce_from(hotkey: &str, topic_id: &str, digest: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"proof-nonce-v1");
    h.update(hotkey.as_bytes());
    h.update(topic_id.as_bytes());
    h.update(digest.as_bytes());
    hex::encode(h.finalize())
}

async fn submit(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubmitBody>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let hotkey = parse_hex64(&body.miner_hotkey, "miner_hotkey")?;
    let artifact = parse_hex64(&body.artifact_digest, "artifact_digest")?;
    let _lium_present = headers
        .get("x-lium-api-key")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| !s.is_empty());

    let topic_id = body.topic_id.trim().to_owned();
    if topic_id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "topic_id is required"));
    }
    let Ok(topic) = st.store.topic(&topic_id) else {
        return Err(err(StatusCode::BAD_REQUEST, "unknown topic"));
    };
    if !topic.is_open_at(st.epoch) {
        return Err(err(StatusCode::BAD_REQUEST, "topic is not open"));
    }
    if body.declared_flops > topic.flops_budget {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "declared_flops exceeds the topic budget",
        ));
    }

    let nonce = nonce_from(&hotkey, &topic_id, &artifact);
    let submission_digest = freeze_submission_digest(&hotkey, &topic_id, &artifact, &nonce);

    scoring_readiness(
        &st.pin,
        st.backend,
        st.live(),
        st.store.any_open_scorable(st.epoch).unwrap_or(false),
        st.offer.as_ref(),
        st.judge_api_key.as_deref(),
    )
    .map_err(|e| eval_err(&e))?;
    let Some(offer) = st.offer.as_ref() else {
        return Err(eval_err(&EvalError::InferenceOfferMissing));
    };
    offer
        .serves_topic(&st.pin, &topic)
        .map_err(|e| offer_err(&e))?;
    let resolved = resolve_inference(
        &st.pin,
        Some(&topic.inference),
        secret_backed_base_url().as_deref(),
        Some(offer),
    );
    if !resolved.ready_to_score() {
        return Err(offer_err(&OfferError::Incomplete));
    }

    let sealed = st
        .store
        .baseline(&topic_id)
        .map_err(|e| store_err(&e))?
        .ok_or_else(|| {
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "no sealed baseline recorded for this topic",
            )
        })?;

    let holdout = st
        .store
        .unseal_holdout(&topic_id, &submission_digest)
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;

    let (declared, hits) = contamination_evidence(&body.manifest, &holdout);
    if !declared || !hits.is_empty() {
        let failed = if declared {
            vec![GateFail::Contamination]
        } else {
            vec![GateFail::EvidenceMissing {
                field: "contamination_evidence".into(),
            }]
        };
        return persist_pre_eval_reject(
            &st,
            body,
            &topic,
            hotkey,
            artifact,
            nonce,
            submission_digest,
            &failed,
        );
    }

    let eval = eval_after_freeze(
        &st.pin,
        &topic,
        offer,
        &submission_digest,
        &artifact,
        &holdout,
        &body.claim,
        st.backend,
        st.live(),
        st.judge_api_key.as_deref(),
    )
    .await
    .map_err(|e| eval_err(&e))?;

    let verdict = judge_topic(
        &topic,
        &eval.agent,
        &eval.harness,
        &sealed,
        &hits,
        &supported_custom(),
    );
    let receipt_json = serde_json::to_string(&eval.receipt).unwrap_or_default();
    persist_scored(
        &st,
        body,
        hotkey,
        artifact,
        nonce,
        submission_digest,
        verdict,
        receipt_json,
        eval.backend,
    )
}

#[allow(clippy::too_many_arguments)]
fn persist_pre_eval_reject(
    st: &AppState,
    body: SubmitBody,
    topic: &TopicDocument,
    hotkey: String,
    artifact: String,
    nonce: String,
    submission_digest: String,
    failed: &[GateFail],
) -> Result<(StatusCode, Json<SubmitResp>), (StatusCode, Json<serde_json::Value>)> {
    let verdict = ProofVerdict {
        pass: false,
        agent: AgentVerdict {
            verdict: ProofKind::Reject,
            reproduced: false,
            claim_holds_public: false,
            contamination: failed.iter().any(|f| matches!(f, GateFail::Contamination)),
            canary_hit: false,
            flops_used: 0,
            flops_budget: topic.flops_budget,
            cheat_codes: Vec::new(),
            rationale: "pre-eval reject".into(),
            topic_id: topic.id.clone(),
            family: topic.metric.family,
        },
        harness: HarnessMetrics::default(),
        failed: failed.to_vec(),
        lattice: 0,
    };
    let row = st
        .store
        .insert(Submission {
            id: String::new(),
            topic_id: topic.id.clone(),
            miner_hotkey: hotkey,
            artifact_digest: artifact,
            artifact_uri: body.artifact_uri,
            claim: body.claim,
            declared_flops: body.declared_flops,
            architecture: body.architecture,
            inference_offer_id: st
                .offer
                .as_ref()
                .map(|o| o.offer_id.clone())
                .unwrap_or_default(),
            config_commitment: st
                .offer
                .as_ref()
                .map(|o| o.config_commitment.clone())
                .unwrap_or_default(),
            manifest: body.manifest,
            nonce,
            submission_digest,
            state: SubmissionState::Rejected,
            receipt_json: None,
            verdict: Some(verdict),
            detail: Some(format!("gates={failed:?}")),
        })
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    let _ = st.store.record_topic_run(
        &row.miner_hotkey,
        &topic.id,
        MinerTopicRun {
            pass: false,
            primary: None,
            artifact_digest: row.artifact_digest.clone(),
            near_duplicate: false,
        },
    );
    Ok((
        StatusCode::CREATED,
        Json(SubmitResp {
            id: row.id,
            submission_digest: row.submission_digest,
            topic_id: topic.id.clone(),
            state: row.state,
            eval_backend: st.backend,
            eligible: false,
        }),
    ))
}

#[allow(clippy::too_many_arguments)]
fn persist_scored(
    st: &AppState,
    body: SubmitBody,
    hotkey: String,
    artifact: String,
    nonce: String,
    submission_digest: String,
    verdict: ProofVerdict,
    receipt_json: String,
    backend: EvalBackend,
) -> Result<(StatusCode, Json<SubmitResp>), (StatusCode, Json<serde_json::Value>)> {
    let topic_id = body.topic_id.trim().to_owned();
    let pass = verdict.pass;
    let primary = st
        .store
        .topic(&topic_id)
        .ok()
        .and_then(|t| primary_from_harness(&t, &verdict.harness));
    let artifact_digest = artifact.clone();
    let detail = if pass {
        None
    } else {
        Some(format!("gates={:?}", verdict.failed))
    };
    let row = st
        .store
        .insert(Submission {
            id: String::new(),
            topic_id: topic_id.clone(),
            miner_hotkey: hotkey,
            artifact_digest: artifact,
            artifact_uri: body.artifact_uri,
            claim: body.claim,
            declared_flops: body.declared_flops,
            architecture: body.architecture,
            inference_offer_id: st
                .offer
                .as_ref()
                .map(|o| o.offer_id.clone())
                .unwrap_or_default(),
            config_commitment: st
                .offer
                .as_ref()
                .map(|o| o.config_commitment.clone())
                .unwrap_or_default(),
            manifest: body.manifest,
            nonce,
            submission_digest,
            state: if pass {
                SubmissionState::AwaitingAdmin
            } else {
                SubmissionState::Rejected
            },
            receipt_json: Some(receipt_json),
            verdict: Some(verdict),
            detail,
        })
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    let _ = st.store.record_topic_run(
        &row.miner_hotkey,
        &topic_id,
        MinerTopicRun {
            pass,
            primary,
            artifact_digest,
            near_duplicate: false,
        },
    );
    Ok((
        StatusCode::CREATED,
        Json(SubmitResp {
            id: row.id,
            submission_digest: row.submission_digest,
            topic_id,
            state: row.state,
            eval_backend: backend,
            eligible: pass,
        }),
    ))
}

async fn list_subs(State(st): State<AppState>) -> impl IntoResponse {
    let rows = st.store.list().unwrap_or_default();
    Json(serde_json::json!({ "items": rows }))
}

async fn get_sub(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let row = st
        .store
        .get(&id)
        .map_err(|_| err(StatusCode::NOT_FOUND, "not_found"))?;
    Ok(Json(row))
}

async fn publish_topic(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(doc): Json<TopicDocument>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if st.admin_hashes.is_empty() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "auth_unconfigured"));
    }
    if !admin_ok(&headers, &st.admin_hashes) {
        return Err(err(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    doc.validate(&st.pin, &supported_custom())
        .map_err(|e| topic_err(&e))?;
    doc.verify_signature(&st.pin).map_err(|e| topic_err(&e))?;
    if doc.status == TopicStatus::Open && !doc.baseline.is_sealed() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "an open topic must carry a sealed baseline",
        ));
    }
    st.store.put_topic(doc.clone()).map_err(|e| store_err(&e))?;
    Ok((StatusCode::CREATED, Json(doc)))
}

fn admin_ok(headers: &HeaderMap, hashes: &[String]) -> bool {
    let Some(raw) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let token = raw.strip_prefix("Bearer ").unwrap_or(raw).trim();
    if token.is_empty() {
        return false;
    }
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hashes
        .iter()
        .any(|x| x == &hex::encode(h.clone().finalize()))
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

fn store_err(e: &proof_store::StoreError) -> (StatusCode, Json<serde_json::Value>) {
    err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
}

fn topic_err(e: &TopicError) -> (StatusCode, Json<serde_json::Value>) {
    let code = match e {
        TopicError::NoTopicKey => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    err(code, &e.to_string())
}

fn offer_err(e: &OfferError) -> (StatusCode, Json<serde_json::Value>) {
    err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string())
}

fn eval_err(e: &EvalError) -> (StatusCode, Json<serde_json::Value>) {
    let code = match e {
        EvalError::Integrity(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    err(code, &e.to_string())
}

/// Hash an admin token the same way the server does.
#[must_use]
pub fn hash_admin_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use proof_eval::{sim_document, BaselineMeasurement, BASELINE_SKILL};
    use proof_task::{
        default_adamw, holdout_commitment, inference_config_commitment, synthetic_holdout,
        Constraints, InferenceConfig, InferenceMode, InferenceOffer, InferenceProvider,
        InferenceProviderKind, MetricDirection, MetricFamily, MetricSpec, OfferStatus,
        TopicDocument, TopicStatus, FLOPS_BUDGET_MAX, HOLDOUT_SIZE, METRIC_TOKENS_PER_SEC,
        STRATUM_SIZE,
    };
    use tower::ServiceExt;

    use super::*;

    fn digest(label: &str) -> String {
        let mut h = Sha256::new();
        h.update(label.as_bytes());
        hex::encode(h.finalize())
    }

    fn sk() -> [u8; 32] {
        let mut s = [3u8; 32];
        s[0] = 17;
        s
    }

    fn pk_hex() -> String {
        hex::encode(crypto::public_key_from_mini_secret(&sk()).expect("pk"))
    }

    fn pin(digest: &str) -> ProofPin {
        let mut p = ProofPin {
            eval_image_digest: digest.to_owned(),
            topic_pubkey: pk_hex(),
            ..ProofPin::default()
        };
        p.inference.model = "master-proxy-v0".into();
        p
    }

    fn offer() -> InferenceOffer {
        let config = InferenceConfig {
            mode: InferenceMode::Chat,
            model_ref: "master-proxy-v0".into(),
            max_input_tokens: 32_768,
            max_output_tokens: 8_192,
            temperature: Some(0.0),
            top_p: None,
            timeout_ms: None,
        };
        InferenceOffer {
            offer_id: "master-v0".into(),
            provider: InferenceProvider {
                kind: InferenceProviderKind::OpenaiCompatible,
                base_url: "http://127.0.0.1:8000/v1".into(),
            },
            config_commitment: inference_config_commitment(&config, "http://127.0.0.1:8000/v1"),
            config,
            status: OfferStatus::Open,
        }
    }

    fn unsigned_topic(recs: &[proof_task::HoldoutRecord]) -> TopicDocument {
        let mut baseline = default_adamw(FLOPS_BUDGET_MAX);
        baseline.optimizer = "nccl-ib-reference".into();
        baseline.wall_budget_s = 14_400;
        baseline.script_sha256 = "11".repeat(32);
        TopicDocument {
            id: "dt-no-ib-v0".into(),
            statement: "No IB/NVLink; 12.5 Gbit/s cap; beat sealed comms baseline.".into(),
            payout_mode: proof_task::PayoutMode::Wta,
            constraints: Constraints {
                no_infiniband: true,
                no_nvlink: true,
                no_nccl_fast_fabric: true,
                max_inter_node_gbps: Some(12.5),
            },
            metric: MetricSpec {
                family: MetricFamily::Throughput,
                primary: METRIC_TOKENS_PER_SEC.into(),
                direction: MetricDirection::Max,
                unit: "tokens_per_second".into(),
                epsilon_rel: 0.05,
                quality_floor_nll: 0.02,
                wall_budget_s: 14_400,
                custom_id: String::new(),
            },
            baseline,
            holdout_commitment: holdout_commitment(recs),
            holdout_size: HOLDOUT_SIZE,
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    fn seal_topic(
        pin: &ProofPin,
        mut topic: TopicDocument,
    ) -> (TopicDocument, BaselineMeasurement) {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        topic.holdout_commitment = holdout_commitment(&recs);
        let doc = sim_document(pin, &topic, "base", "base-art", BASELINE_SKILL, true);
        let meas = BaselineMeasurement {
            eval_image_digest: pin.eval_image_digest.clone(),
            topic_id: topic.id.clone(),
            holdout_commitment: topic.holdout_commitment.clone(),
            holdout_nll: doc.harness.holdout_nll,
            split_nll: doc.harness.split_nll.clone(),
            tokens_per_sec: doc.harness.tokens_per_sec,
            step_latency_ms: doc.harness.step_latency_ms,
            custom_value: doc.harness.custom_value,
        };
        topic.baseline.metrics_commitment = meas.commitment();
        topic.signature = topic.sign_with(&sk()).expect("sign");
        topic.validate(pin, &[]).expect("valid");
        topic.verify_signature(pin).expect("sig");
        (topic, meas)
    }

    struct StubScorer {
        reproduced: bool,
        skill: f64,
        hits: AtomicUsize,
    }

    impl StubScorer {
        fn win() -> Self {
            Self {
                reproduced: true,
                skill: 0.95,
                hits: AtomicUsize::new(0),
            }
        }
        fn lose() -> Self {
            Self {
                reproduced: false,
                skill: 0.95,
                hits: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl LiveScorer for StubScorer {
        async fn score(
            &self,
            pin: &ProofPin,
            topic: &TopicDocument,
            _offer: &InferenceOffer,
            frozen: &str,
            artifact: &str,
            _holdout: &[proof_task::HoldoutRecord],
            _claim: &str,
        ) -> Result<proof_eval::ProofEvalDocument, EvalError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            Ok(sim_document(
                pin,
                topic,
                frozen,
                artifact,
                self.skill,
                self.reproduced,
            ))
        }
    }

    fn declared_manifest() -> ArtifactManifest {
        ArtifactManifest {
            train_dataset_ids: vec!["public-pretrain-v0".into()],
            ..ArtifactManifest::default()
        }
    }

    fn app_full(
        token: &str,
        backend: EvalBackend,
        eval_digest: &str,
        live: Option<Arc<dyn LiveScorer>>,
        load: bool,
        baseline: bool,
        with_offer: bool,
    ) -> Router {
        let p = pin(eval_digest);
        let store = MemoryStore::new();
        if load {
            let recs = synthetic_holdout(STRATUM_SIZE, 1);
            let (topic, meas) = seal_topic(&p, unsigned_topic(&recs));
            store.put_topic(topic.clone()).expect("topic");
            store.load_holdout(&topic.id, recs).expect("holdout");
            if baseline {
                store
                    .set_baseline(&topic.id, meas.into_sealed())
                    .expect("baseline");
            }
        }
        let judge_api_key =
            (backend == EvalBackend::Lium && live.is_some()).then(|| "test-judge-key".to_owned());
        proof_router(AppState {
            store,
            pin: p,
            backend,
            live_scorer: live,
            offer: with_offer.then(offer),
            // Lium + a wired harvest is the live path: a missing key is the
            // Testeur blocker. Sim does not call the judge, so it stays None.
            judge_api_key,
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
            epoch: 0,
        })
    }

    fn app(token: &str) -> Router {
        app_full(token, EvalBackend::Sim, "", None, true, true, true)
    }

    async fn json_req(
        app: Router,
        method: &str,
        uri: &str,
        body: serde_json::Value,
        auth: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(a) = auth {
            b = b.header(axum::http::header::AUTHORIZATION, format!("Bearer {a}"));
        }
        let req = b
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("req");
        let resp = app.oneshot(req).await.expect("resp");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, v)
    }

    fn submit_body(label: &str, extra: &serde_json::Value) -> serde_json::Value {
        let mut v = serde_json::json!({
            "miner_hotkey": digest("miner-hotkey"),
            "artifact_digest": digest(label),
            "claim": "beats the sealed reference under the cap",
            "declared_flops": FLOPS_BUDGET_MAX / 2,
            "topic_id": "dt-no-ib-v0",
            "manifest": {
                "train_dataset_ids": ["public-pretrain-v0"]
            },
        });
        if let Some(obj) = extra.as_object() {
            if let Some(dst) = v.as_object_mut() {
                for (k, val) in obj {
                    dst.insert(k.clone(), val.clone());
                }
            }
        }
        v
    }

    #[tokio::test]
    async fn health_and_status_name_the_challenge() {
        let (st, health) = json_req(app("op"), "GET", "/health", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(health["challenge_id"], "proof");

        let (st, body) =
            json_req(app("op"), "GET", "/v1/status", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["eval_backend"], "sim");
        assert_eq!(body["can_score"], true, "{body}");
        assert_eq!(body["baseline_sealed"], true, "{body}");
        assert_eq!(body["open_topics"][0], "dt-no-ib-v0");
        let dump = body.to_string();
        assert!(!dump.contains("synthetic-dev"), "{dump}");
        assert!(!dump.contains("holdout_nll"));
        assert_eq!(body["inference_offer"]["offer_id"], "master-v0");
        assert_eq!(body["inference_offer"]["status"], "open");
        assert_eq!(body["inference"]["provider"], "openai_compatible");
        assert_eq!(body["inference"]["mode"], "chat");
        assert_eq!(body["inference"]["model"], "master-proxy-v0");
        assert!(!dump.contains("8000"), "{dump}");
        assert!(!dump.contains("base_url"), "{dump}");
        assert!(!dump.contains("api_key"), "{dump}");
        assert!(!dump.contains("evil.example"), "{dump}");
    }

    #[tokio::test]
    async fn topics_are_public_documents_without_records() {
        let (st, list) = json_req(
            app("op"),
            "GET",
            "/v1/proof/topics",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(list["items"][0]["id"], "dt-no-ib-v0");
        assert_eq!(list["items"][0]["metric"]["family"], "throughput");
        assert!(list["items"][0]["holdout_commitment"].is_string());
        let dump = list.to_string();
        assert!(!dump.contains("content_sha256"), "{dump}");

        let (st, one) = json_req(
            app("op"),
            "GET",
            "/v1/proof/topics/dt-no-ib-v0",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(one["status"], "open");
    }

    #[tokio::test]
    async fn submit_requires_an_open_topic_id() {
        let app = app("op");
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({ "topic_id": "" })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({ "topic_id": "unknown-v0" })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    }

    #[tokio::test]
    async fn reproduced_true_scores_and_false_zeros() {
        let win = Arc::new(StubScorer::win());
        let lose = Arc::new(StubScorer::lose());
        let digest = format!("sha256:{}", "ab".repeat(32));

        let (st, created) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &digest,
                Some(win),
                true,
                true,
                true,
            ),
            "POST",
            "/v1/submissions",
            submit_body("miner-strong-proof", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eval_backend"], "lium");
        assert_eq!(created["eligible"], true, "{created}");
        assert_eq!(created["state"], "awaiting_admin");

        let (st, created) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &digest,
                Some(lose),
                true,
                true,
                true,
            ),
            "POST",
            "/v1/submissions",
            submit_body("miner-unreproduced", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eligible"], false, "{created}");
        assert_eq!(created["state"], "rejected");
    }

    #[tokio::test]
    async fn contamination_is_rejected_without_renting() {
        let scorer = Arc::new(StubScorer::win());
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let dirty_hash = recs[0].content_sha256.clone();
        let app = app_full(
            "op",
            EvalBackend::Lium,
            &format!("sha256:{}", "ab".repeat(32)),
            Some(scorer.clone()),
            true,
            true,
            true,
        );
        for manifest in [
            serde_json::json!({}),
            serde_json::json!({
                "manifest": { "train_content_hashes": [dirty_hash] }
            }),
        ] {
            let empty_evidence = serde_json::json!({ "manifest": { "train_dataset_ids": [] } });
            let body = if manifest.as_object().is_some_and(serde_json::Map::is_empty) {
                submit_body("junk", &empty_evidence)
            } else {
                submit_body("junk", &manifest)
            };
            let (st, created) = json_req(app.clone(), "POST", "/v1/submissions", body, None).await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
            assert_eq!(created["eligible"], false, "{created}");
            assert_eq!(created["state"], "rejected", "{created}");
        }
        assert_eq!(
            scorer.hits.load(Ordering::SeqCst),
            0,
            "contaminated / empty-evidence must not rent a pod"
        );
        let _ = declared_manifest();
    }

    #[tokio::test]
    async fn empty_digest_and_unwired_harvest_are_503() {
        let (st, body) = json_req(
            app_full("op", EvalBackend::Lium, "", None, true, true, true),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("eval image digest not pinned"));

        let (st, body) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &format!("sha256:{}", "cd".repeat(32)),
                None,
                true,
                true,
                true,
            ),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no in-process sim"));
    }

    #[tokio::test]
    async fn refused_submissions_leave_no_rows() {
        let unpinned = app_full("op", EvalBackend::Lium, "", None, true, true, true);
        let no_baseline = app_full(
            "op",
            EvalBackend::Lium,
            &format!("sha256:{}", "cd".repeat(32)),
            Some(Arc::new(StubScorer::win())),
            true,
            false,
            true,
        );
        let sealed = app_full("op", EvalBackend::Sim, "", None, false, false, true);

        for (label, app) in [
            ("unpinned digest", unpinned),
            ("no baseline", no_baseline),
            ("no open topic", sealed),
        ] {
            for _ in 0..3 {
                let (st, body) = json_req(
                    app.clone(),
                    "POST",
                    "/v1/submissions",
                    submit_body("spam", &serde_json::json!({})),
                    None,
                )
                .await;
                assert!(
                    st == StatusCode::SERVICE_UNAVAILABLE || st == StatusCode::BAD_REQUEST,
                    "{label}: {st} {body}"
                );
            }
            let (st, list) =
                json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
            assert_eq!(st, StatusCode::OK);
            assert!(
                list["items"].as_array().is_some_and(Vec::is_empty),
                "{label} banked rows: {list}"
            );
        }
    }

    #[tokio::test]
    async fn can_score_is_false_until_open_holdout_and_baseline() {
        let (st, body) = json_req(
            app_full("op", EvalBackend::Sim, "", None, false, false, true),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["can_score"], false, "{body}");
        assert_eq!(body["baseline_sealed"], false, "{body}");

        let (st, body) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &format!("sha256:{}", "ab".repeat(32)),
                Some(Arc::new(StubScorer::win())),
                true,
                false,
                true,
            ),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["live_harvest_wired"], true);
        assert_eq!(body["can_score"], false, "{body}");
    }

    #[tokio::test]
    async fn missing_or_closed_offer_is_503() {
        let (st, body) = json_req(
            app_full("op", EvalBackend::Sim, "", None, true, true, false),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["can_score"], false, "{body}");
        assert!(body["inference_offer"].is_null(), "{body}");

        let (st, body) = json_req(
            app_full("op", EvalBackend::Sim, "", None, true, true, false),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("inference offer"));
    }

    #[tokio::test]
    async fn admin_publish_requires_bearer_and_a_valid_signature() {
        let token = "op-test-token";
        let app = app(token);
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let p = pin("");
        let (mut doc, _) = seal_topic(&p, unsigned_topic(&recs));
        doc.id = "adamw-beater-v0".into();
        doc.signature = doc.sign_with(&sk()).expect("sign");

        let (st, _) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&doc).expect("json"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        let (st, created) = json_req(
            app,
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&doc).expect("json"),
            Some(token),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["id"], "adamw-beater-v0");
    }

    #[tokio::test]
    async fn unknown_custom_is_a_publish_400() {
        let token = "op";
        let app = app(token);
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let p = pin("");
        let (mut doc, _) = seal_topic(&p, unsigned_topic(&recs));
        doc.id = "custom-unknown-v0".into();
        doc.metric.family = MetricFamily::Custom;
        doc.metric.custom_id = "not-implemented".into();
        doc.signature = doc.sign_with(&sk()).expect("sign");
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&doc).expect("json"),
            Some(token),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("custom metric"));
    }

    #[tokio::test]
    async fn harness_success_rate_is_a_listed_custom_and_publishes() {
        let token = "op";
        let app = app(token);
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let p = pin("");
        let (mut doc, _) = seal_topic(&p, unsigned_topic(&recs));
        doc.id = "agent-harness-improve-v0".into();
        doc.payout_mode = proof_task::PayoutMode::Discovery;
        doc.validation = proof_task::ValidationSpec {
            score_on: "Holdout harness success rate (and secondary latency) vs sealed baseline"
                .into(),
            accept_if: "Reproduced under FLOP/wall budget; no contamination; success rate >= baseline + epsilon".into(),
            reject_if: "Unreproduced claim; eval short-circuit; FLOP over budget; near-duplicate of an accepted artifact".into(),
        };
        doc.metric = MetricSpec {
            family: MetricFamily::Custom,
            primary: "success_rate".into(),
            direction: MetricDirection::Max,
            unit: "rate".into(),
            epsilon_rel: 0.05,
            quality_floor_nll: 0.0,
            wall_budget_s: 0,
            custom_id: proof_task::CUSTOM_HARNESS_SUCCESS_RATE.into(),
        };
        doc.signature = doc.sign_with(&sk()).expect("sign");
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&doc).expect("json"),
            Some(token),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        assert_eq!(body["id"], "agent-harness-improve-v0");
        assert_eq!(body["payout_mode"], "discovery");
    }

    fn app_lium_missing_judge_key() -> Router {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let store = MemoryStore::new();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (topic, meas) = seal_topic(&p, unsigned_topic(&recs));
        store.put_topic(topic.clone()).expect("topic");
        store.load_holdout(&topic.id, recs).expect("holdout");
        store
            .set_baseline(&topic.id, meas.into_sealed())
            .expect("baseline");
        proof_router(AppState {
            store,
            pin: p,
            backend: EvalBackend::Lium,
            live_scorer: Some(Arc::new(StubScorer::win())),
            offer: Some(offer()),
            judge_api_key: None,
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            epoch: 0,
        })
    }

    #[tokio::test]
    async fn lium_without_judge_api_key_cannot_score() {
        let (st, body) = json_req(
            app_lium_missing_judge_key(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["live_harvest_wired"], true);
        assert_eq!(body["can_score"], false, "{body}");
        let dump = body.to_string();
        assert!(!dump.contains("api_key"), "{dump}");

        let (st, body) = json_req(
            app_lium_missing_judge_key(),
            "POST",
            "/v1/submissions",
            submit_body("no-key", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("API key"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn lium_with_judge_api_key_can_score_and_status_omits_the_secret() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        let (st, body) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &digest,
                Some(Arc::new(StubScorer::win())),
                true,
                true,
                true,
            ),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["can_score"], true, "{body}");
        let dump = body.to_string();
        assert!(!dump.contains("api_key"), "{dump}");
        assert!(!dump.contains("test-judge-key"), "{dump}");
        assert!(!dump.contains("base_url"), "{dump}");
        assert!(!dump.contains("8000"), "{dump}");
    }

    #[tokio::test]
    async fn spoofed_topic_origin_is_503() {
        let p = pin("");
        let store = MemoryStore::new();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (mut topic, meas) = seal_topic(&p, unsigned_topic(&recs));
        topic.inference.max_input_tokens = Some(4_096);
        topic.inference.base_url = Some("http://evil.example/v1".into());
        topic.signature = topic.sign_with(&sk()).expect("sign");
        store.put_topic(topic.clone()).expect("topic");
        store.load_holdout(&topic.id, recs).expect("holdout");
        store
            .set_baseline(&topic.id, meas.into_sealed())
            .expect("baseline");
        let app = proof_router(AppState {
            store,
            pin: p,
            backend: EvalBackend::Sim,
            live_scorer: None,
            offer: Some(offer()),
            judge_api_key: None,
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            epoch: 0,
        });
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("spoof", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("committed judge origin"),
            "{body}"
        );
    }
}
