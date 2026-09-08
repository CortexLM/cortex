//! Full control-plane path for the generic RLM engine, without any VM,
//! Lium, or paid inference: `POST /v1/submissions` on an open custom-family
//! topic through `FamilyMux` → `RlmScorer` → registry → generic
//! `VmBackedRunner` → fake orchestrator, with the memory RLM store and a
//! temp artefact root.
//!
//! Covers: an unregistered `custom_id` is a 503 with no row; a green
//! checklist scores and is crowned against the sealed value; a red checklist
//! is a persisted reject with **zero** paid runs; a later pass below the best
//! stays `awaiting_admin`; every scored row leaves its zip, the crown leaves
//! `best.json` + a promotion row, and the store holds rules v1, every
//! checklist, and the lifecycle. Then the setup driver walks
//! `draft → … → baselining` with RLM-written rules and a baseline in the
//! store, and `mark_sealed` opens the topic. Every id here is a placeholder.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::too_many_lines
)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use proof_eval::{EvalBackend, EvalError, FamilyMux, LiveScorer, ProofEvalDocument};
use proof_http::{hash_admin_token, proof_router, AppState};
use proof_rlm::fixtures::{offer, pin, pinned_template, topic, FakeOrchestrator};
use proof_rlm::{
    FileKeysProbe, OwnerDecision, RlmEvent, RlmState, RunnerRegistry, StaticOwnerHook,
    VmBackedRunner, VmJob,
};
use proof_rlm_scorer::{ArtefactStore, RlmScorer, SetupError, TopicSetup};
use proof_rlm_store::{MemoryRlmStore, RlmStore};
use proof_score::SealedBaseline;
use proof_store::MemoryStore;
use proof_task::{
    holdout_commitment, synthetic_holdout, HoldoutRecord, InferenceOffer, ProofPin, TopicDocument,
    TopicStatus, STRATUM_SIZE,
};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

/// Default route: a harvest that is ready and never asked to score here.
struct IdleHarvest;

#[async_trait::async_trait]
impl LiveScorer for IdleHarvest {
    async fn score(
        &self,
        _pin: &ProofPin,
        _topic: &TopicDocument,
        _offer: &InferenceOffer,
        _frozen: &str,
        _artifact: &str,
        _holdout: &[HoldoutRecord],
        _claim: &str,
    ) -> Result<ProofEvalDocument, EvalError> {
        Err(EvalError::Backend("idle harvest".into()))
    }
}

fn digest(label: &str) -> String {
    hex::encode(Sha256::digest(label.as_bytes()))
}

fn tmp_root(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "proof-rlm-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Stack {
    app: Router,
    orchestrator: Arc<FakeOrchestrator>,
    rlm_store: Arc<MemoryRlmStore>,
    root: std::path::PathBuf,
    topic: TopicDocument,
}

fn stack(register: bool) -> Stack {
    let pin = pin();
    let recs = synthetic_holdout(STRATUM_SIZE, 1);
    let mut t = topic();
    t.holdout_commitment = holdout_commitment(&recs);
    let registered = if register {
        vec![t.metric.custom_id.clone()]
    } else {
        Vec::new()
    };
    let registered_ref: Vec<&str> = registered.iter().map(String::as_str).collect();
    if register {
        t.validate(&pin, &registered_ref)
            .expect("open custom topic validates");
    }

    let store = MemoryStore::new();
    store.put_topic(t.clone()).unwrap();
    store.load_holdout(&t.id, recs).unwrap();
    store
        .set_baseline(
            &t.id,
            SealedBaseline {
                custom_value: Some(0.5),
                ..SealedBaseline::default()
            },
        )
        .unwrap();

    let orchestrator = FakeOrchestrator::new(0.7);
    let mut registry = RunnerRegistry::new();
    if register {
        registry
            .register(
                &t.metric.custom_id,
                Arc::new(VmBackedRunner::new(orchestrator.clone(), pinned_template())),
            )
            .unwrap();
    }
    let rlm_store = Arc::new(MemoryRlmStore::new());
    let root = tmp_root("stack");
    let scorer = RlmScorer::new(Arc::new(registry), rlm_store.clone())
        .with_artefacts(Some(ArtefactStore::new(&root)));
    let mux = FamilyMux::new(Arc::new(IdleHarvest)).with_custom_family(Arc::new(scorer));
    let app = proof_router(AppState {
        store,
        pin,
        backend: EvalBackend::Lium,
        live_scorer: Some(Arc::new(mux)),
        offer: Some(offer()),
        judge_api_key: Some("test-judge-key".into()),
        admin_hashes: Arc::new(vec![hash_admin_token("op")]),
        epoch: 0,
    });
    Stack {
        app,
        orchestrator,
        rlm_store,
        root,
        topic: t,
    }
}

async fn json_req(
    app: Router,
    method: &str,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
    (status, v)
}

fn submit_body(topic_id: &str, label: &str) -> serde_json::Value {
    serde_json::json!({
        "miner_hotkey": digest("miner"),
        "artifact_digest": digest(label),
        "claim": "placeholder claim",
        "declared_flops": 1u64,
        "topic_id": topic_id,
        "manifest": { "train_dataset_ids": ["placeholder-corpus"] },
    })
}

fn zip_names(path: &std::path::Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_owned())
        .collect();
    names.sort();
    names
}

fn paid_runs(orchestrator: &FakeOrchestrator) -> usize {
    orchestrator
        .jobs()
        .iter()
        .filter(|j| matches!(j, VmJob::Evaluate { .. }))
        .count()
}

#[tokio::test]
async fn an_unregistered_custom_id_is_503_with_no_row() {
    let Stack { app, topic, .. } = stack(false);
    let (st, status) = json_req(app.clone(), "GET", "/v1/status", serde_json::json!({})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(status["open_topics"][0], topic.id, "{status}");
    assert!(
        status["scorable_topics"].as_array().unwrap().is_empty(),
        "{status}"
    );
    assert!(
        status["registered_custom"].as_array().unwrap().is_empty(),
        "{status}"
    );
    assert_eq!(status["can_score"], false, "{status}");

    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/v1/submissions",
        submit_body(&topic.id, "artifact-a"),
    )
    .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(msg.contains(&topic.metric.custom_id), "{body}");
    assert!(msg.contains("no registered runner"), "{body}");
    let (_, list) = json_req(app, "GET", "/v1/submissions", serde_json::json!({})).await;
    assert!(list["items"].as_array().unwrap().is_empty(), "{list}");
}

#[tokio::test]
async fn submit_scores_rejects_and_promotes_through_the_registry_end_to_end() {
    let Stack {
        app,
        orchestrator,
        rlm_store,
        root,
        topic,
    } = stack(true);
    let tid = topic.id.clone();

    // 0. Open, registered, scorable.
    let (st, status) = json_req(app.clone(), "GET", "/v1/status", serde_json::json!({})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(status["can_score"], true, "{status}");
    assert_eq!(status["scorable_topics"][0], tid, "{status}");
    assert_eq!(
        status["registered_custom"][0], topic.metric.custom_id,
        "{status}"
    );
    let (_, topics) = json_req(
        app.clone(),
        "GET",
        "/v1/proof/topics",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        topics["items"][0]["metric"]["custom_id"],
        topic.metric.custom_id
    );
    assert_eq!(
        topics["items"][0]["constraints"]["firecracker_required"],
        true
    );
    assert_eq!(topics["items"][0]["checklist"][0]["id"], "rule_a");
    assert!(
        !topics.to_string().contains("content_sha256"),
        "holdout leak"
    );

    // 1. Green checklist, primary 0.70 > 0.50 * 1.02: scored and crowned.
    let (st, created) = json_req(
        app.clone(),
        "POST",
        "/v1/submissions",
        submit_body(&tid, "artifact-a"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    assert_eq!(created["eligible"], true, "{created}");
    assert_eq!(created["state"], "champion", "{created}");
    let id_a = created["id"].as_str().unwrap().to_owned();
    assert_eq!(paid_runs(&orchestrator), 1);
    assert_eq!(orchestrator.created(), 1, "one topic, one vm");

    let (_, row) = json_req(
        app.clone(),
        "GET",
        &format!("/v1/submissions/{id_a}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(row["verdict"]["pass"], true, "{row}");
    assert!((row["verdict"]["harness"]["custom_value"].as_f64().unwrap() - 0.7).abs() < 1e-12);
    assert!(row["verdict"]["agent"]["rationale"]
        .as_str()
        .unwrap()
        .contains("rules v1"));
    let dump = row.to_string();
    assert!(
        !dump.contains("api_key") && !dump.contains("127.0.0.1"),
        "{dump}"
    );

    let zip_a = root.join(&tid).join(format!("{id_a}.zip"));
    assert_eq!(
        zip_names(&zip_a),
        [
            "artifact/src/main.rs",
            "baseline_ref.json",
            "checklist.json",
            "logs/run.log",
            "manifest.json",
            "report.json",
        ]
    );
    let best = ArtefactStore::new(&root).best(&tid).expect("best.json");
    assert_eq!(best.submission_id, id_a);
    let promo = rlm_store.best(&tid).await.unwrap().expect("promotion row");
    assert_eq!(promo.submission_id, id_a);
    assert!((promo.primary_value - 0.7).abs() < 1e-12);
    assert_eq!(promo.previous_best, None);
    let rules = rlm_store
        .current_rules(&tid)
        .await
        .unwrap()
        .expect("rules v1 in store");
    assert_eq!(rules.version, 1);
    assert_eq!(rules.rules, topic.checklist);
    let (_, doc) = rlm_store
        .latest_topic(&tid)
        .await
        .unwrap()
        .expect("topic in store");
    assert_eq!(doc.signature, topic.signature);

    // 2. Red checklist: persisted reject, no paid run, red checklist in store.
    orchestrator.set_red(Some("rule_b"));
    let (st, created) = json_req(
        app.clone(),
        "POST",
        "/v1/submissions",
        submit_body(&tid, "artifact-b"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    assert_eq!(created["eligible"], false, "{created}");
    assert_eq!(created["state"], "rejected", "{created}");
    let id_b = created["id"].as_str().unwrap().to_owned();
    assert_eq!(paid_runs(&orchestrator), 1, "a red checklist must not pay");
    let (_, row_b) = json_req(
        app.clone(),
        "GET",
        &format!("/v1/submissions/{id_b}"),
        serde_json::json!({}),
    )
    .await;
    assert!(row_b["verdict"]["agent"]["rationale"]
        .as_str()
        .unwrap()
        .contains("rule_b"));
    assert!(row_b["verdict"]["harness"]["custom_value"].is_null());
    let names_b = zip_names(&root.join(&tid).join(format!("{id_b}.zip")));
    assert!(!names_b.iter().any(|n| n == "report.json"), "{names_b:?}");
    let digest_b = row_b["submission_digest"].as_str().unwrap();
    let cl = rlm_store
        .checklist(digest_b)
        .await
        .unwrap()
        .expect("checklist row");
    assert!(!cl.green);
    assert_eq!(cl.failed_ids, vec!["rule_b".to_owned()]);

    // 3. Green again but 0.60: beats the seal (pass) yet not the best
    //    0.70 * 1.02, so it stays awaiting_admin and the crown stays.
    orchestrator.set_red(None);
    orchestrator.set_primary(0.6);
    let (st, created) = json_req(
        app.clone(),
        "POST",
        "/v1/submissions",
        submit_body(&tid, "artifact-c"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    assert_eq!(created["eligible"], true, "{created}");
    assert_eq!(created["state"], "awaiting_admin", "{created}");
    assert_eq!(paid_runs(&orchestrator), 2);
    assert_eq!(
        ArtefactStore::new(&root).best(&tid).unwrap().submission_id,
        id_a
    );
    assert_eq!(rlm_store.promotions(&tid).await.unwrap().len(), 1);
    assert_eq!(rlm_store.artefacts(&tid).await.unwrap().len(), 3);
    assert_eq!(ArtefactStore::new(&root).events(&tid).len(), 4);

    // 4. Lifecycle mirror is back at open with every move persisted.
    let lc = rlm_store.lifecycle(&tid).await.unwrap().expect("lifecycle");
    assert_eq!(lc.state, RlmState::Open);
    let events: Vec<RlmEvent> = lc.history.iter().map(|h| h.event).collect();
    assert_eq!(
        events,
        [
            RlmEvent::SubmissionReceived,
            RlmEvent::PromotionCandidate,
            RlmEvent::Promoted,
            RlmEvent::SubmissionReceived,
            RlmEvent::VerdictRecorded,
            RlmEvent::SubmissionReceived,
            RlmEvent::VerdictRecorded,
        ]
    );
    // Jobs never carried a host path or a secret.
    for job in orchestrator.jobs() {
        let dump = serde_json::to_string(&job).unwrap();
        for forbidden in ["/run/base", "/opt/base", "api_key", "127.0.0.1"] {
            assert!(!dump.contains(forbidden), "job leaked {forbidden}");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The agentic setup: owner hook, key probe, VM provision, RLM-written
/// rules and baseline persisted, then the operator seals and opens.
#[tokio::test]
async fn topic_setup_walks_the_lifecycle_over_the_vm_boundary() {
    let root = tmp_root("setup");
    let key = root.join("owner_key");
    let orchestrator = FakeOrchestrator::new(0.42);
    let rlm_store: Arc<MemoryRlmStore> = Arc::new(MemoryRlmStore::new());
    let mut draft = topic();
    draft.status = TopicStatus::Draft;
    draft.baseline.script_sha256 = "11".repeat(32);
    draft.baseline.metrics_commitment.clear();
    let mut setup = TopicSetup {
        orchestrator: orchestrator.clone(),
        store: rlm_store.clone(),
        template: pinned_template(),
        owner: Arc::new(StaticOwnerHook(OwnerDecision::Decline {
            reason: "not yet".into(),
        })),
        keys: Arc::new(FileKeysProbe::new(&key)),
        spend_cap_usd: Some(10.0),
    };

    // A decline returns to draft; nothing is provisioned.
    let err = setup
        .run(&draft, &pin(), &offer())
        .await
        .expect_err("declined");
    assert!(matches!(err, SetupError::Declined(_)), "{err}");
    assert_eq!(
        rlm_store.lifecycle(&draft.id).await.unwrap().unwrap().state,
        RlmState::Draft
    );
    assert_eq!(orchestrator.created(), 0);

    // Approved but no key file: stops at awaiting_owner_keys, nothing provisioned.
    setup.owner = Arc::new(StaticOwnerHook(OwnerDecision::Approve));
    let err = setup
        .run(&draft, &pin(), &offer())
        .await
        .expect_err("no key");
    assert!(err.to_string().contains("owner keys not present"), "{err}");
    assert_eq!(
        rlm_store.lifecycle(&draft.id).await.unwrap().unwrap().state,
        RlmState::AwaitingOwnerKeys
    );
    assert_eq!(orchestrator.created(), 0);

    // Key present: provision, RLM rules v1 in store, baseline in store.
    std::fs::write(&key, "not-a-real-secret\n").unwrap();
    let out = setup.run(&draft, &pin(), &offer()).await.expect("setup");
    assert_eq!(out.rules_version, 1);
    assert!((out.baseline_primary - 0.42).abs() < 1e-12);
    assert_eq!(orchestrator.created(), 1);
    let rules = rlm_store.current_rules(&draft.id).await.unwrap().unwrap();
    assert_eq!(rules.source, proof_rlm::RuleSource::Rlm);
    assert_eq!(rules.rules[0].id, "rlm_rule");
    let baseline = rlm_store.baseline(&draft.id).await.unwrap().unwrap();
    assert_eq!(baseline.rules_version, 1);
    let lc = rlm_store.lifecycle(&draft.id).await.unwrap().unwrap();
    assert_eq!(lc.state, RlmState::Baselining);
    assert!(lc.history.iter().any(|h| h.event == RlmEvent::Provisioned));
    let dump = serde_json::to_string(&lc).unwrap();
    assert!(
        !dump.contains("not-a-real-secret"),
        "key value must never be recorded"
    );
    assert!(orchestrator.jobs().iter().any(|j| matches!(
        j,
        VmJob::ProposeRules {
            current_version: None,
            ..
        }
    )));

    // The operator seals custom_value (0.42) and re-signs open.
    let mut open = draft.clone();
    open.status = TopicStatus::Open;
    open.baseline.metrics_commitment = "22".repeat(32);
    assert_eq!(setup.mark_sealed(&open).await.unwrap(), RlmState::Open);
    let (v, latest) = rlm_store.latest_topic(&draft.id).await.unwrap().unwrap();
    assert_eq!(v, 2);
    assert_eq!(latest.status, TopicStatus::Open);
    let _ = std::fs::remove_dir_all(&root);
}
