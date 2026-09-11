//! Full control-plane path for the generic RLM engine, without any VM,
//! Lium, or paid inference: `POST /v1/submissions` on an open custom-family
//! topic through `FamilyMux::custom_only` (the shape a host with no Lium
//! harvest boots) → `RlmScorer` → registry → generic `VmBackedRunner` →
//! fake orchestrator, with the memory RLM store and a temp artefact root.
//!
//! Covers: `/v1/status` reports the custom family wired and ready while
//! `live_harvest_wired` stays false; an unregistered `custom_id` is a 503
//! with no row; a custom
//! submission without an artefact locator is a 400 with no row; a green
//! checklist scores and is crowned against the sealed value, with the
//! miner's artefact locator and declaration reaching the runner and the
//! runner's measured FLOPs in the verdict; a red checklist is a persisted
//! reject with **zero** paid runs; a later pass below the best stays
//! `awaiting_admin`; a measurement over the budget or over the miner's
//! declaration is a persisted reject and a missing one is a 503;
//! every scored row leaves its zip, the crown leaves `best.json` + a
//! promotion row, and the store holds rules v1, every checklist, and the
//! lifecycle. Runs hold their topic lease until persisted, so a worse run
//! decided against a stale bar can never displace the champion, a moved
//! best pointer refuses a stale crown, and an abandoned run releases its
//! lease after the TTL. Then the setup driver walks `draft → … →
//! baselining` with RLM-written rules and a baseline in the store, and
//! `mark_sealed` opens the topic only for the signed, valid, open document
//! whose sealed value is the RLM's. Every id here is a placeholder.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::too_many_lines
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use proof_eval::{
    BaselineMeasurement, EvalBackend, EvalError, FamilyMux, LiveScorer, ProofEvalDocument,
};
use proof_executor::{EvalExecutorOffer, ExecutorPlan};
use proof_http::{executor_slot, hash_admin_token, proof_router, AppState};
use proof_rlm::fixtures::{offer, pin, pinned_template, topic, FakeOrchestrator};
use proof_rlm::{
    FileKeysProbe, OwnerDecision, RlmEvent, RlmState, RunnerRegistry, StaticOwnerHook,
    VmBackedRunner, VmJob,
};
use proof_rlm_scorer::{ArtefactStore, RlmScorer, SetupError, TopicSetup};
use proof_rlm_store::{MemoryRlmStore, PromotionRow, RlmStore};
use proof_score::SealedBaseline;
use proof_store::MemoryStore;
use proof_task::{
    holdout_commitment, synthetic_holdout, HoldoutSplit, ProofPin, TopicDocument, TopicError,
    TopicStatus, STRATUM_SIZE,
};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

/// Open `1x` executor on the digest-scoped template of `pin` (host state the
/// live path requires; the RLM path only records its plan commitment).
fn test_executor(pin: &ProofPin) -> EvalExecutorOffer {
    let hex = pin.eval_image_digest.trim_start_matches("sha256:");
    let mut o = EvalExecutorOffer {
        offer_id: "executor-placeholder".into(),
        lium_template_id: format!("proof-eval-{}", hex.get(..12).unwrap_or("unpinned")),
        machine_shape: "1x".into(),
        max_proof_deadline_s: 3_600,
        eval_image_digest: pin.eval_image_digest.clone(),
        config_commitment: String::new(),
        status: proof_executor::OfferStatus::Open,
    };
    o.config_commitment = o.expected_commitment();
    o
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
    // No Lium harvest on this host: the custom family stands on its own.
    let mux = FamilyMux::custom_only(Arc::new(scorer));
    let executor = test_executor(&pin);
    let app = proof_router(AppState {
        store,
        pin,
        backend: EvalBackend::Lium,
        live_scorer: Some(Arc::new(mux)),
        offer: Some(offer()),
        executor: executor_slot(Some(executor)),
        judge_api_key: Some("test-judge-key".into()),
        admin_hashes: Arc::new(vec![hash_admin_token("op")]),
        vm_probe: None,
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

/// Where a test miner says the bytes behind `label` live.
fn locator(label: &str) -> String {
    format!("https://example.invalid/artefacts/{label}.zip")
}

/// A custom-topic submission declaring `declared_flops`; the locator is
/// required on custom topics, so every body carries one.
fn fixture_sk() -> [u8; 32] {
    let mut s = [0x11u8; 32];
    s[0] = 0x42;
    s
}

fn submit_declaring(topic_id: &str, label: &str, declared_flops: u64) -> serde_json::Value {
    let mut v = serde_json::json!({
        "artifact_digest": digest(label),
        "artifact_uri": locator(label),
        "claim": "placeholder claim",
        "declared_flops": declared_flops,
        "topic_id": topic_id,
        "manifest": { "train_dataset_ids": ["placeholder-corpus"] },
    });
    proof_submit::attach_to_json(&mut v, &fixture_sk()).expect("sign");
    v
}

fn submit_body(topic_id: &str, label: &str) -> serde_json::Value {
    submit_declaring(topic_id, label, 1)
}

fn no_flop_gate(row: &serde_json::Value) {
    let codes = row["verdict"]["agent"]["cheat_codes"].to_string();
    assert!(!codes.contains("flops_over_budget"), "{codes}");
    assert!(!codes.contains("flops_under_declared"), "{codes}");
    let failed = row["verdict"]["failed"].to_string();
    assert!(!failed.contains("flops_over_budget"), "{failed}");
    assert_ne!(row["state"], "rejected", "{row}");
    assert_eq!(row["verdict"]["agent"]["verdict"], "clean", "{row}");
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

fn manifest_of(path: &std::path::Path) -> serde_json::Value {
    let bytes = std::fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut file = archive.by_name("manifest.json").unwrap();
    let mut body = String::new();
    std::io::Read::read_to_string(&mut file, &mut body).unwrap();
    serde_json::from_str(&body).unwrap()
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
    assert_eq!(status["live_harvest_wired"], false, "{status}");
    assert_eq!(status["custom_family_wired"], false, "{status}");
    assert!(status["custom_ready"].as_array().unwrap().is_empty());
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

    // 0. Open, registered, scorable — and reported per family: the custom
    //    family is wired and ready, the Lium harvest is not.
    let (st, status) = json_req(app.clone(), "GET", "/v1/status", serde_json::json!({})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(status["can_score"], true, "{status}");
    assert_eq!(status["scorable_topics"][0], tid, "{status}");
    assert_eq!(
        status["registered_custom"][0], topic.metric.custom_id,
        "{status}"
    );
    assert_eq!(status["live_harvest_wired"], false, "{status}");
    assert_eq!(status["custom_family_wired"], true, "{status}");
    assert_eq!(
        status["custom_ready"][0], topic.metric.custom_id,
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

    // 0b. A custom submission without a locator is refused at intake: no
    //     row, no VM job, no rent — the runner would have nothing to fetch.
    let mut no_locator = submit_body(&tid, "artifact-a");
    no_locator["artifact_uri"] = serde_json::Value::Null;
    let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", no_locator).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "artifact required");
    assert!(orchestrator.jobs().is_empty(), "no job without a locator");

    // 1. Green checklist, primary 0.70 > 0.50 * 1.02: scored and crowned.
    //    The miner's artefact locator and declaration travel to the runner
    //    with the digest.
    let uri = locator("artifact-a");
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
    let requests: Vec<proof_rlm::CustomRunRequest> = orchestrator
        .jobs()
        .into_iter()
        .filter_map(|j| match j {
            VmJob::Inspect { request, .. } | VmJob::Evaluate { request, .. } => Some(request),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 2, "one inspect, one evaluate");
    for req in &requests {
        assert_eq!(
            req.artifact_uri.as_deref(),
            Some(uri.as_str()),
            "runner must receive the submitted locator"
        );
        assert_eq!(req.artifact_digest, digest("artifact-a"));
        assert_eq!(req.flops_budget, topic.flops_budget);
        assert_eq!(req.declared_flops, 1);
    }

    let (_, row) = json_req(
        app.clone(),
        "GET",
        &format!("/v1/submissions/{id_a}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(row["verdict"]["pass"], true, "{row}");
    assert!((row["verdict"]["harness"]["custom_value"].as_f64().unwrap() - 0.7).abs() < 1e-12);
    assert_eq!(
        row["verdict"]["agent"]["flops_used"], 1u64,
        "verdict carries the runner's measured usage, not zero"
    );
    assert_eq!(row["verdict"]["agent"]["flops_budget"], topic.flops_budget);
    assert_eq!(row["artifact_uri"], uri, "{row}");
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
    assert_eq!(manifest_of(&zip_a)["promoted"], true);
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
    assert_eq!(
        row_b["verdict"]["agent"]["flops_used"], 0u64,
        "nothing ran, nothing was spent"
    );
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

/// Custom / agent topics do not reject or cheat-code on FLOP accounting:
/// over the signed budget, over the miner's declaration, a missing
/// measurement, or `declared_flops: 0` all still score.
#[tokio::test]
async fn custom_topics_ignore_flop_accounting() {
    let Stack {
        app,
        orchestrator,
        rlm_store,
        root,
        topic,
    } = stack(true);
    let tid = topic.id.clone();
    let budget = topic.flops_budget;

    orchestrator.set_flops_used(Some(budget + 1));
    let (st, created) = json_req(
        app.clone(),
        "POST",
        "/v1/submissions",
        submit_declaring(&tid, "artifact-over", budget),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    let (_, row) = json_req(
        app.clone(),
        "GET",
        &format!("/v1/submissions/{id}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(row["verdict"]["agent"]["flops_used"], budget + 1, "{row}");
    no_flop_gate(&row);
    assert_eq!(created["state"], "champion", "{created}");
    assert!(ArtefactStore::new(&root).best(&tid).is_some());

    orchestrator.set_flops_used(Some(2));
    let (st, created) = json_req(
        app.clone(),
        "POST",
        "/v1/submissions",
        submit_body(&tid, "artifact-under-declared"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    let (_, row) = json_req(
        app.clone(),
        "GET",
        &format!("/v1/submissions/{id}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(row["verdict"]["agent"]["flops_used"], 2u64, "{row}");
    assert_eq!(row["declared_flops"], 1u64, "{row}");
    no_flop_gate(&row);

    orchestrator.set_flops_used(None);
    let (st, created) = json_req(
        app.clone(),
        "POST",
        "/v1/submissions",
        submit_body(&tid, "artifact-unmeasured"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    let (_, row) = json_req(
        app.clone(),
        "GET",
        &format!("/v1/submissions/{id}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(row["verdict"]["agent"]["flops_used"], 0u64, "{row}");
    no_flop_gate(&row);

    orchestrator.set_flops_used(Some(1));
    let (st, created) = json_req(
        app.clone(),
        "POST",
        "/v1/submissions",
        submit_declaring(&tid, "artifact-zero-declared", 0),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    let (_, row) = json_req(
        app,
        "GET",
        &format!("/v1/submissions/{id}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(row["declared_flops"], 0u64, "{row}");
    no_flop_gate(&row);
    assert!(rlm_store.best(&tid).await.unwrap().is_some());
    let _ = std::fs::remove_dir_all(&root);
}

struct Direct {
    scorer: Arc<RlmScorer>,
    orchestrator: Arc<FakeOrchestrator>,
    rlm_store: Arc<MemoryRlmStore>,
    root: std::path::PathBuf,
    topic: TopicDocument,
    pin: ProofPin,
    plan: ExecutorPlan,
}

/// The scorer alone (no router), for lease and persist ordering tests.
fn direct(tag: &str, lease_ttl: Option<Duration>) -> Direct {
    let pin = pin();
    let recs = synthetic_holdout(STRATUM_SIZE, 1);
    let mut t = topic();
    t.holdout_commitment = holdout_commitment(&recs);
    let orchestrator = FakeOrchestrator::new(0.7);
    let registry = RunnerRegistry::new().with(
        &t.metric.custom_id,
        Arc::new(VmBackedRunner::new(orchestrator.clone(), pinned_template())),
    );
    let rlm_store = Arc::new(MemoryRlmStore::new());
    let root = tmp_root(tag);
    let mut scorer = RlmScorer::new(Arc::new(registry), rlm_store.clone())
        .with_artefacts(Some(ArtefactStore::new(&root)));
    if let Some(ttl) = lease_ttl {
        scorer = scorer.with_lease_ttl(ttl);
    }
    let plan = scorer.plan(&pin, &t, &test_executor(&pin)).expect("plan");
    Direct {
        scorer: Arc::new(scorer),
        orchestrator,
        rlm_store,
        root,
        topic: t,
        pin,
        plan,
    }
}

async fn score(d: &Direct, label: &str) -> Result<ProofEvalDocument, EvalError> {
    d.scorer
        .score(
            &d.pin,
            &d.topic,
            &offer(),
            &d.plan,
            &format!("digest-{label}"),
            &digest(label),
            Some(&locator(label)),
            d.topic.flops_budget,
            &[],
            "placeholder claim",
            &proof_rlm::MinerEnv::new(),
            None,
        )
        .await
}

/// A `tracing` subscriber that keeps every event as `(level, "field=value …")`
/// so a test can read what the host journal would hold. No dependency: the
/// `tracing` crate the scorer already logs through is enough.
#[derive(Default)]
struct LogCapture {
    lines: Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>,
    ids: std::sync::atomic::AtomicU64,
}

struct Flatten<'a>(&'a mut String);

impl tracing::field::Visit for Flatten<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}

impl tracing::Subscriber for LogCapture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let n = self.ids.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tracing::span::Id::from_u64(n + 1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = String::new();
        event.record(&mut Flatten(&mut line));
        self.lines
            .lock()
            .unwrap()
            .push((*event.metadata().level(), line));
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// A paid run that fails **after a green inspection** is a refusal with no
/// row — and the refusal is written to the host journal. The miner's 503
/// body is otherwise the only place that error string lives, so the host
/// logs it with the topic and the submission digest before the lifecycle
/// note `refused; no row`; the lease is released and the next run scores.
#[tokio::test]
async fn a_refused_paid_run_is_host_logged_with_its_error() {
    use tracing::instrument::WithSubscriber;
    let d = direct("refused-log", None);
    // Inspection is green; the paid run comes back claiming no sandbox,
    // which the runner refuses on a `firecracker_required` topic.
    d.orchestrator.set_sandboxed(false);
    let capture = LogCapture::default();
    let lines = capture.lines.clone();
    let err = score(&d, "fail")
        .with_subscriber(capture)
        .await
        .expect_err("an unsandboxed paid run is not evidence");
    let text = err.to_string();
    assert!(text.contains("outside the Firecracker guest"), "{text}");
    assert_eq!(
        d.orchestrator
            .jobs()
            .iter()
            .filter(|j| matches!(j, VmJob::Evaluate { .. }))
            .count(),
        1,
        "the paid run was attempted after the green inspect"
    );
    assert_eq!(d.scorer.pending_len(), 0, "no bundle awaits a row");

    // The verdict phase closed with the refusal note; the topic is open again.
    let lc = d
        .rlm_store
        .lifecycle(&d.topic.id)
        .await
        .unwrap()
        .expect("lifecycle");
    assert_eq!(lc.state, RlmState::Open);
    let last = lc.history.last().expect("a transition");
    assert_eq!(last.event, RlmEvent::VerdictRecorded);
    assert_eq!(last.note, "refused; no row");

    // The host journal holds the same string the miner's 503 body carries,
    // at error level, naming the topic and the submission digest.
    let logged = lines.lock().unwrap().clone();
    let refusal = logged
        .iter()
        .find(|(level, line)| {
            *level == tracing::Level::ERROR && line.contains("evaluate refused; no row")
        })
        .unwrap_or_else(|| panic!("no refusal in the journal: {logged:?}"));
    assert!(
        refusal.1.contains(&format!("topic_id={}", d.topic.id)),
        "{refusal:?}"
    );
    assert!(
        refusal.1.contains("frozen_digest=\"digest-fail\""),
        "{refusal:?}"
    );
    assert!(
        refusal.1.contains("outside the Firecracker guest"),
        "the error string reaches the journal: {refusal:?}"
    );

    // The lease was released with the refusal: the next run scores.
    d.orchestrator.set_sandboxed(true);
    let doc = score(&d, "ok").await.expect("the topic is not stuck");
    assert_eq!(doc.harness.custom_value, Some(0.7));
    assert_eq!(d.scorer.pending_len(), 1);
    let _ = std::fs::remove_dir_all(&d.root);
}

/// Two runs decided against the same old bar: the second cannot score until
/// the first is persisted, its decision then sees the new best, and a moved
/// best pointer refuses a crown that was decided before it moved.
#[tokio::test]
async fn a_worse_run_never_displaces_the_champion_under_the_topic_lease() {
    let d = direct("lease", None);
    let tid = d.topic.id.clone();

    // A scores 0.70 and now holds the topic lease until its row lands.
    let doc_a = score(&d, "a").await.expect("a scores");
    assert!((doc_a.harness.custom_value.unwrap() - 0.7).abs() < 1e-12);
    assert_eq!(d.scorer.pending_len(), 1);

    // B (0.60) is blocked on the lease, not scored against the stale world.
    d.orchestrator.set_primary(0.6);
    let scorer_b = d.scorer.clone();
    let (pin_b, topic_b, plan_b) = (d.pin.clone(), d.topic.clone(), d.plan.clone());
    let mut task_b = tokio::spawn(async move {
        let budget = topic_b.flops_budget;
        scorer_b
            .score(
                &pin_b,
                &topic_b,
                &offer(),
                &plan_b,
                "digest-b",
                &digest("b"),
                Some(&locator("b")),
                budget,
                &[],
                "placeholder claim",
                &proof_rlm::MinerEnv::new(),
                None,
            )
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut task_b)
            .await
            .is_err(),
        "b must wait for a's row"
    );
    assert_eq!(paid_runs(&d.orchestrator), 1, "b has not run");

    // A is decided against bar 0.50 and persisted under the lease.
    assert!(
        d.scorer
            .auto_promote(&d.topic, "digest-a", true, Some(0.7), Some(0.5))
            .await
    );
    d.scorer
        .on_persisted(&tid, "digest-a", "pf_0000000000000001", true)
        .await;
    assert_eq!(d.scorer.pending_len(), 0);

    // The lease is free: b scores, and its decision — even handed the stale
    // bar 0.50 — is taken against the store's best 0.70.
    let doc_b = task_b.await.unwrap().expect("b scores after a persisted");
    assert!((doc_b.harness.custom_value.unwrap() - 0.6).abs() < 1e-12);
    assert!(
        !d.scorer
            .auto_promote(&d.topic, "digest-b", true, Some(0.6), Some(0.5))
            .await,
        "0.60 does not beat the reigning 0.70"
    );
    d.scorer
        .on_persisted(&tid, "digest-b", "pf_0000000000000002", false)
        .await;
    let best = d.rlm_store.best(&tid).await.unwrap().expect("best");
    assert_eq!(best.submission_id, "pf_0000000000000001");
    assert!((best.primary_value - 0.7).abs() < 1e-12);
    assert_eq!(d.rlm_store.promotions(&tid).await.unwrap().len(), 1);
    assert_eq!(
        ArtefactStore::new(&d.root)
            .best(&tid)
            .unwrap()
            .submission_id,
        "pf_0000000000000001"
    );

    // C (0.90) is decided to promote; another writer crowns 0.95 before C's
    // row lands. The compare-and-swap on the best pointer refuses C.
    d.orchestrator.set_primary(0.9);
    score(&d, "c").await.expect("c scores");
    assert!(
        d.scorer
            .auto_promote(&d.topic, "digest-c", true, Some(0.9), Some(0.7))
            .await
    );
    d.rlm_store
        .record_promotion(&PromotionRow {
            topic_id: tid.clone(),
            submission_id: "pf_00000000000000ff".into(),
            submission_digest: "digest-elsewhere".into(),
            primary_value: 0.95,
            bar: Some(0.7),
            previous_best: Some("pf_0000000000000001".into()),
        })
        .await
        .unwrap();
    d.scorer
        .on_persisted(&tid, "digest-c", "pf_0000000000000003", true)
        .await;
    let best = d.rlm_store.best(&tid).await.unwrap().unwrap();
    assert_eq!(
        best.submission_id, "pf_00000000000000ff",
        "stale crown refused"
    );
    assert_eq!(d.rlm_store.promotions(&tid).await.unwrap().len(), 2);
    assert_eq!(
        ArtefactStore::new(&d.root)
            .best(&tid)
            .unwrap()
            .submission_id,
        "pf_0000000000000001",
        "best pointer untouched by the refused crown"
    );
    let zip_c = d.root.join(&tid).join("pf_0000000000000003.zip");
    assert_eq!(manifest_of(&zip_c)["promoted"], false);
    let artefacts = d.rlm_store.artefacts(&tid).await.unwrap();
    assert!(
        !artefacts
            .iter()
            .find(|a| a.submission_id == "pf_0000000000000003")
            .unwrap()
            .promoted
    );
    let lc = d.rlm_store.lifecycle(&tid).await.unwrap().unwrap();
    assert_eq!(lc.state, RlmState::Open);
    let events: Vec<RlmEvent> = lc.history.iter().map(|h| h.event).collect();
    assert_eq!(
        &events[events.len() - 3..],
        [
            RlmEvent::SubmissionReceived,
            RlmEvent::PromotionCandidate,
            RlmEvent::PromotionRefused,
        ]
    );
    let _ = std::fs::remove_dir_all(&d.root);
}

/// A run whose row never lands must not hold its topic hostage: past the
/// lease TTL the next run reaps it, recovers the lifecycle, and proceeds.
#[tokio::test]
async fn an_abandoned_run_releases_its_topic_lease_after_the_ttl() {
    let d = direct("ttl", Some(Duration::ZERO));
    let tid = d.topic.id.clone();
    score(&d, "abandoned").await.expect("scores");
    assert_eq!(d.scorer.pending_len(), 1);
    let next = tokio::time::timeout(Duration::from_secs(10), score(&d, "next"))
        .await
        .expect("the abandoned lease is reaped, not waited on")
        .expect("scores");
    assert!((next.harness.custom_value.unwrap() - 0.7).abs() < 1e-12);
    assert_eq!(d.scorer.pending_len(), 1, "only the live run is pending");
    assert!(
        !d.scorer
            .auto_promote(&d.topic, "digest-abandoned", true, Some(0.7), Some(0.5))
            .await,
        "a reaped run cannot be promoted"
    );
    let lc = d.rlm_store.lifecycle(&tid).await.unwrap().unwrap();
    assert!(
        lc.history
            .iter()
            .any(|h| h.event == RlmEvent::VerdictRecorded && h.note.contains("recovered")),
        "{lc:?}"
    );
    assert_eq!(lc.state, RlmState::Evaluating, "the live run is still open");
    let _ = std::fs::remove_dir_all(&d.root);
}

fn sk() -> [u8; 32] {
    let mut s = [7u8; 32];
    s[0] = 42;
    s
}

fn pin_with_topic_key() -> ProofPin {
    let mut p = pin();
    p.topic_pubkey = hex::encode(crypto::public_key_from_mini_secret(&sk()).unwrap());
    p
}

/// A sealed measurement for a custom topic: every scored split present, the
/// custom value the operator seals.
fn sealed_measurement(pin: &ProofPin, topic: &TopicDocument, custom: f64) -> BaselineMeasurement {
    let split_nll: BTreeMap<String, f64> = HoldoutSplit::SCORED
        .iter()
        .map(|s| (s.as_str().to_owned(), 0.0))
        .collect();
    BaselineMeasurement {
        eval_image_digest: pin.eval_image_digest.clone(),
        topic_id: topic.id.clone(),
        holdout_commitment: topic.holdout_commitment.clone(),
        holdout_nll: 0.0,
        split_nll,
        tokens_per_sec: None,
        step_latency_ms: None,
        custom_value: Some(custom),
    }
}

/// The open document sealing `meas`, signed with the test topic key.
fn signed_open(draft: &TopicDocument, meas: &BaselineMeasurement) -> TopicDocument {
    let mut open = draft.clone();
    open.status = TopicStatus::Open;
    open.baseline.metrics_commitment = meas.commitment();
    open.signature = open.sign_with(&sk()).unwrap();
    open
}

/// The agentic setup: owner hook, key probe, VM provision, RLM-written
/// rules and baseline persisted, then the operator seals and opens — and
/// only a signed, valid, open document sealing the RLM's value opens.
#[tokio::test]
async fn topic_setup_walks_the_lifecycle_over_the_vm_boundary() {
    let root = tmp_root("setup");
    let key = root.join("owner_key");
    let pin = pin_with_topic_key();
    let orchestrator = FakeOrchestrator::new(0.42);
    let rlm_store: Arc<MemoryRlmStore> = Arc::new(MemoryRlmStore::new());
    let mut draft = topic();
    draft.status = TopicStatus::Draft;
    draft.holdout_commitment = holdout_commitment(&synthetic_holdout(STRATUM_SIZE, 1));
    draft.baseline.script_sha256 = "11".repeat(32);
    draft.baseline.metrics_commitment.clear();
    let registered = [draft.metric.custom_id.as_str()];
    let mut setup = TopicSetup {
        orchestrator: orchestrator.clone(),
        store: rlm_store.clone(),
        template: pinned_template(),
        experiments: proof_rlm::ExperimentPolicy::default(),
        owner: Arc::new(StaticOwnerHook(OwnerDecision::Decline {
            reason: "not yet".into(),
        })),
        keys: Arc::new(FileKeysProbe::new(&key)),
        spend_cap_usd: Some(10.0),
    };

    // A decline returns to draft; nothing is provisioned.
    let err = setup
        .run(&draft, &pin, &offer())
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
    let err = setup.run(&draft, &pin, &offer()).await.expect_err("no key");
    assert!(err.to_string().contains("owner keys not present"), "{err}");
    assert_eq!(
        rlm_store.lifecycle(&draft.id).await.unwrap().unwrap().state,
        RlmState::AwaitingOwnerKeys
    );
    assert_eq!(orchestrator.created(), 0);

    // Key present: an unmeasured / over-budget baseline is still sealed
    // (custom / agent setup does not gate on FLOP accounting).
    std::fs::write(&key, "not-a-real-secret\n").unwrap();
    orchestrator.set_flops_used(None);
    let out = setup.run(&draft, &pin, &offer()).await.expect("setup");
    assert_eq!(out.rules_version, 1);
    assert!((out.baseline_primary - 0.42).abs() < 1e-12);
    assert_eq!(
        orchestrator.created(),
        1,
        "the vm is attached, not re-created"
    );
    let rules = rlm_store.current_rules(&draft.id).await.unwrap().unwrap();
    assert_eq!(rules.version, 1);
    assert_eq!(rules.source, proof_rlm::RuleSource::Rlm);
    assert_eq!(rules.rules[0].id, "rlm_rule");
    let baseline = rlm_store.baseline(&draft.id).await.unwrap().unwrap();
    assert_eq!(baseline.rules_version, 1);
    assert_eq!(baseline.report.flops_used, None);
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

    // Sealing refuses everything that is not the signed, valid, open
    // document sealing the RLM's 0.42 — and nothing moves or is stored.
    let meas = sealed_measurement(&pin, &draft, 0.42);
    let still_baselining = |store: &Arc<MemoryRlmStore>| {
        let store = store.clone();
        let id = draft.id.clone();
        async move {
            let lc = store.lifecycle(&id).await.unwrap().unwrap();
            assert_eq!(lc.state, RlmState::Baselining);
            let (version, latest) = store.latest_topic(&id).await.unwrap().unwrap();
            assert_eq!(version, 1, "no new version was stored");
            assert_eq!(latest.status, TopicStatus::Draft);
        }
    };

    // Still a draft.
    let err = setup
        .mark_sealed(&draft, &pin, &registered, &meas)
        .await
        .expect_err("draft");
    assert!(matches!(err, SetupError::NotOpen(_)), "{err}");
    still_baselining(&rlm_store).await;

    // Open but not signed by the operator key.
    let mut unsigned = signed_open(&draft, &meas);
    unsigned.signature = "00".repeat(64);
    let err = setup
        .mark_sealed(&unsigned, &pin, &registered, &meas)
        .await
        .expect_err("unsigned");
    assert!(
        matches!(err, SetupError::Topic(TopicError::SignatureInvalid)),
        "{err}"
    );
    still_baselining(&rlm_store).await;

    // Open and signed, but not valid as an open topic on this host (no runner).
    let open = signed_open(&draft, &meas);
    let err = setup
        .mark_sealed(&open, &pin, &[], &meas)
        .await
        .expect_err("unregistered custom id cannot open");
    assert!(
        matches!(
            err,
            SetupError::Topic(TopicError::UnknownCustomMetric { .. })
        ),
        "{err}"
    );
    still_baselining(&rlm_store).await;

    // Open, signed, valid, but the sealed value is not what the RLM measured.
    let wrong = sealed_measurement(&pin, &draft, 0.99);
    let wrong_doc = signed_open(&draft, &wrong);
    let err = setup
        .mark_sealed(&wrong_doc, &pin, &registered, &wrong)
        .await
        .expect_err("sealed a value the rlm never measured");
    assert!(
        matches!(err, SetupError::Seal(ref m) if m.contains("not the measured baseline")),
        "{err}"
    );
    still_baselining(&rlm_store).await;

    // Open, signed, valid, but the measurement does not bind to the document.
    let err = setup
        .mark_sealed(&open, &pin, &registered, &wrong)
        .await
        .expect_err("commitment mismatch");
    assert!(matches!(err, SetupError::Seal(_)), "{err}");
    still_baselining(&rlm_store).await;

    // The real thing: baselining → open, version 2 stored.
    assert_eq!(
        setup
            .mark_sealed(&open, &pin, &registered, &meas)
            .await
            .unwrap(),
        RlmState::Open
    );
    let (v, latest) = rlm_store.latest_topic(&draft.id).await.unwrap().unwrap();
    assert_eq!(v, 2);
    assert_eq!(latest.status, TopicStatus::Open);
    assert_eq!(latest.signature, open.signature);
    // Sealing twice is illegal from open.
    let err = setup
        .mark_sealed(&open, &pin, &registered, &meas)
        .await
        .expect_err("already open");
    assert!(matches!(err, SetupError::State(_)), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A topic whose signed params select an in-guest runner measures its
/// baseline in **one dedicated experiment VM** — created for the `Baseline`
/// job, sized under the lock ceilings, carrying the pinned pack, destroyed
/// afterwards — while the RLM's rule proposal still runs in the topic VM.
#[tokio::test]
async fn an_experiment_topic_measures_its_baseline_in_a_dedicated_vm() {
    let root = tmp_root("setup-experiment");
    let key = root.join("owner_key");
    std::fs::write(&key, "not-a-real-secret\n").unwrap();
    let pin = pin_with_topic_key();
    let orchestrator = FakeOrchestrator::new(0.61);
    let rlm_store: Arc<MemoryRlmStore> = Arc::new(MemoryRlmStore::new());
    let mut draft = topic();
    draft.status = TopicStatus::Draft;
    draft.holdout_commitment = holdout_commitment(&synthetic_holdout(STRATUM_SIZE, 1));
    draft.baseline.script_sha256 = "11".repeat(32);
    draft.baseline.metrics_commitment.clear();
    draft.constraints.params.insert(
        proof_experiment::PARAM_RUNNER.into(),
        "placeholder_in_guest_runner".into(),
    );
    draft.constraints.params.insert(
        proof_experiment::PARAM_PACK_DIGEST.into(),
        format!("sha256:{}", "ee".repeat(32)),
    );
    draft
        .constraints
        .params
        .insert(proof_experiment::PARAM_MEM_MIB.into(), "16384".into());
    let setup = TopicSetup {
        orchestrator: orchestrator.clone(),
        store: rlm_store.clone(),
        template: pinned_template(),
        experiments: proof_rlm::ExperimentPolicy::default(),
        owner: Arc::new(StaticOwnerHook(OwnerDecision::Approve)),
        keys: Arc::new(FileKeysProbe::new(&key)),
        spend_cap_usd: None,
    };
    let out = setup.run(&draft, &pin, &offer()).await.expect("setup");
    assert!((out.baseline_primary - 0.61).abs() < 1e-12);
    assert_eq!(
        orchestrator.created(),
        2,
        "topic vm + one baseline experiment vm"
    );
    let specs = orchestrator.experiments();
    assert_eq!(specs.len(), 1);
    let exp = specs[0].experiment.as_ref().unwrap();
    assert_eq!(exp.runner, "placeholder_in_guest_runner");
    assert_eq!(exp.pack.digest, format!("sha256:{}", "ee".repeat(32)));
    assert_eq!(exp.disk_mib, 32_768);
    assert_eq!(
        (specs[0].template.vcpus, specs[0].template.mem_mib),
        (16, 16_384)
    );
    let runs = orchestrator.runs();
    assert!(
        matches!(runs[0], (ref vm, VmJob::ProposeRules { .. }) if vm == &out.vm.vm_id),
        "rules are proposed in the topic vm"
    );
    assert!(
        matches!(runs[1], (ref vm, VmJob::Baseline { .. }) if vm != &out.vm.vm_id),
        "the baseline ran in the experiment vm"
    );
    let downs = orchestrator.teardowns();
    assert_eq!(
        downs.len(),
        1,
        "the experiment vm is destroyed after the baseline"
    );
    assert_eq!(downs[0].1, proof_rlm::RetainPolicy::Destroy);
    assert_eq!(orchestrator.vms().len(), 1, "the topic vm stays");
    let baseline = rlm_store.baseline(&draft.id).await.unwrap().unwrap();
    assert!((baseline.primary_value - 0.61).abs() < 1e-12);

    // A baseline whose experiment VM is not confirmed destroyed is not a
    // baseline: the setup fails, names the VM, and seals nothing.
    orchestrator.set_teardown(Ok(false));
    let mut leaky = draft.clone();
    leaky.id = "topic-b".into();
    let err = setup
        .run(&leaky, &pin, &offer())
        .await
        .expect_err("unconfirmed destroy withholds the baseline");
    assert!(
        matches!(
            err,
            SetupError::Vm(proof_rlm::VmError::TeardownUnconfirmed { .. })
        ),
        "{err}"
    );
    assert!(err.to_string().contains("not confirmed destroyed"), "{err}");
    assert!(
        rlm_store.baseline(&leaky.id).await.unwrap().is_none(),
        "no baseline row from a run whose vm may still hold capacity"
    );
    assert_eq!(
        orchestrator.experiments().len(),
        2,
        "the second experiment vm was created for the job"
    );
    assert_eq!(
        orchestrator.vms().len(),
        3,
        "and is still alive on the fake host"
    );
    let _ = std::fs::remove_dir_all(&root);
}
