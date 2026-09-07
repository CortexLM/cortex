#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use common::{
    pg::{Fixture, SEED},
    *,
};
use proof_autonomy::{commitment, ContributionAward, DecayPlan};
use proof_research::ResearchStore;
use proof_rounds::*;
use proof_runtime::{RuntimeCall, RuntimeOperations};
use serde_json::json;
use uuid::Uuid;

async fn empty_store(f: &Fixture) -> RoundStore {
    let research = ResearchStore::new(f.database.app_pool().await.unwrap(), scientific().0);
    RoundStore::new(f.database.app_pool().await.unwrap(), research, config())
}

fn award(frozen: &FrozenRound, units: u64) -> ContributionAward {
    let (digest, contribution) = frozen.contributions.first_key_value().unwrap();
    ContributionAward {
        contribution_digest: digest.clone(),
        miner_hotkey: hex::encode(contribution.miner_hotkey),
        units,
        evidence_digests: contribution.evidence_digests.iter().cloned().collect(),
        rationale: "Synthetic discovery".into(),
        decay: DecayPlan {
            first_round: 0,
            initial_units: 500_000,
            retention_ppm: 500_000,
            expires_round: 10,
        },
        decay_revision: None,
    }
}

#[tokio::test]
async fn freeze_is_contiguous_idempotent_and_serializes_competing_schedulers() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let store = empty_store(&f).await;
    store.ready().await.unwrap();
    let chain = source(&f).await;
    assert!(store.freeze(&chain, 1).await.is_err());
    let (left, right) = tokio::join!(store.freeze(&chain, 0), store.freeze(&chain, 0));
    let frozen = left.unwrap();
    assert_eq!(frozen, right.unwrap());
    assert_eq!(frozen.snapshot.finalized_block, 360);
    assert!(frozen.evidence.is_empty());
    assert!(store.freeze(&chain, 1).await.is_err());
    let (left, right) = tokio::join!(
        store.acquire(0, Uuid::new_v4(), 60),
        store.acquire(0, Uuid::new_v4(), 60)
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let lease = left.or(right).unwrap();
    assert!(store
        .decide(&lease, &decision(&frozen), &[8; 32])
        .await
        .is_err());
    store
        .decide(&lease, &decision(&frozen), &SEED)
        .await
        .unwrap();
    assert!(store.freeze(&chain, 1).await.is_err());
    store.publish(0, &Publisher).await.unwrap();
    // Chain epochs and research rounds are different clocks. No dropped round.
    let next = store.freeze(&chain, 1).await.unwrap();
    assert_eq!(next.snapshot.chain_epoch, frozen.snapshot.chain_epoch);
    assert_eq!(next.snapshot.finalized_block, 720);
    let mut wrong = config();
    wrong.netuid = 2;
    let other = RoundStore::new(
        f.database.app_pool().await.unwrap(),
        ResearchStore::new(f.database.app_pool().await.unwrap(), scientific().0),
        wrong,
    );
    assert!(other.frozen(0).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn atlas_private_calls_validate_credit_and_replay_identical_signed_bytes() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (research, summary) = completed(&f).await;
    let store = RoundStore::new(f.database.app_pool().await.unwrap(), research, config());
    let chain = source(&f).await;
    let frozen = store.freeze(&chain, 0).await.unwrap();
    assert_eq!(frozen.evidence.len(), 1);
    assert_eq!(frozen.evidence[&summary.evidence_digest], summary);
    let lease = store.acquire(0, Uuid::new_v4(), 60).await.unwrap();
    let operations = AtlasOperations::bind(store.clone(), lease, SEED)
        .await
        .unwrap();
    let request = |operation: &str, arguments| RuntimeCall {
        schema_version: 1,
        scope: operations.scope().clone(),
        operation: operation.into(),
        arguments,
    };
    let page = operations
        .call(request("read_evidence", json!({"limit":1})))
        .await
        .unwrap();
    assert_eq!(page["total"], 1);
    assert!(operations
        .call(request("execute", json!({})))
        .await
        .is_err());
    assert!(operations
        .call(request("history", json!({"limit":100})))
        .await
        .is_err());
    let mut wrong_scope = request("history", json!({"limit":1}));
    wrong_scope.scope.role = "experiment".into();
    assert!(operations.call(wrong_scope).await.is_err());
    let mut proposed = decision(&frozen);
    proposed.awards = vec![award(&frozen, 500_000)];
    operations
        .call(request(
            "submit_decision",
            serde_json::to_value(&proposed).unwrap(),
        ))
        .await
        .unwrap();
    let first = store.decision(0).await.unwrap();
    let retry = store.decide(&lease, &proposed, &SEED).await.unwrap();
    assert_eq!(first.leaves, retry.leaves);
    proposed.awards[0].units = 499_999;
    assert!(store.decide(&lease, &proposed, &SEED).await.is_err());
    store.publish(0, &Publisher).await.unwrap();
    let next = store.freeze(&chain, 1).await.unwrap();
    let lease = store.acquire(1, Uuid::new_v4(), 60).await.unwrap();
    let mut proposed = decision(&next);
    proposed.awards = vec![award(&next, 500_000)];
    assert!(store.decide(&lease, &proposed, &SEED).await.is_err());
    // Omit the contribution: retain its zero award in subsequent history.
    proposed.awards.clear();
    let omitted = store.decide(&lease, &proposed, &SEED).await.unwrap();
    assert_eq!(
        omitted
            .history
            .values()
            .next()
            .unwrap()
            .previous
            .as_ref()
            .unwrap()
            .units,
        0
    );
    store.publish(1, &Publisher).await.unwrap();
    let third = store.freeze(&chain, 2).await.unwrap();
    let lease = store.acquire(2, Uuid::new_v4(), 60).await.unwrap();
    let mut revived = decision(&third);
    revived.awards = vec![award(&third, 1)];
    assert!(store.decide(&lease, &revived, &SEED).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn corpus_obeys_boundary_time_epoch_and_retained_bytes() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (research, summary) = completed(&f).await;
    let chain = source(&f).await;
    assert!(research.corpus(6, chain.cutoff).await.unwrap().is_empty());
    assert!(research.corpus(7, 1).await.unwrap().is_empty());
    let corpus = research.corpus(7, chain.cutoff).await.unwrap();
    assert_eq!(corpus.len(), 1);
    let recipe = research.recipe(&summary.recipe_digest).await.unwrap();
    assert_eq!(
        corpus[0].contribution_digest,
        commitment(&(
            &recipe.topic.id,
            &recipe.topic.baseline.metrics_commitment,
            &recipe.candidate_script_digest
        ))
        .unwrap()
    );
    let store = RoundStore::new(
        f.database.app_pool().await.unwrap(),
        research.clone(),
        config(),
    );
    let frozen = store.freeze(&chain, 0).await.unwrap();
    let lease = store.acquire(0, Uuid::new_v4(), 60).await.unwrap();
    sqlx::query("UPDATE proof_evidence_artifact SET bytes = '\\x00' WHERE evidence_digest = $1")
        .bind(&summary.evidence_digest)
        .execute(f.database.pool())
        .await
        .unwrap();
    assert!(research.corpus(7, chain.cutoff).await.is_err());
    let mut proposed = decision(&frozen);
    proposed.awards = vec![award(&frozen, 500_000)];
    assert!(store.decide(&lease, &proposed, &SEED).await.is_err());
    f.close().await;
}

struct ReceiptPublisher {
    calls: AtomicUsize,
    correct: bool,
}
#[async_trait]
impl RoundPublisher for ReceiptPublisher {
    async fn publish(&self, document: &RoundPublication) -> Result<String, RoundError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.correct {
            Ok(commitment(document)?)
        } else {
            Ok("f".repeat(64))
        }
    }
}

#[tokio::test]
async fn takeover_and_publication_require_current_fences_and_exact_receipts() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let store = empty_store(&f).await;
    let frozen = store.freeze(&source(&f).await, 0).await.unwrap();
    let old = store.acquire(0, Uuid::new_v4(), 60).await.unwrap();
    sqlx::query("UPDATE proof_atlas_lease SET expires_at = '-infinity'")
        .execute(f.database.pool())
        .await
        .unwrap();
    assert!(store.renew(&old, 60).await.is_err());
    let current = store.acquire(0, Uuid::new_v4(), 60).await.unwrap();
    assert!(current.fence > old.fence);
    assert!(store.decide(&old, &decision(&frozen), &SEED).await.is_err());
    store
        .decide(&current, &decision(&frozen), &SEED)
        .await
        .unwrap();
    let wrong = ReceiptPublisher {
        calls: AtomicUsize::new(0),
        correct: false,
    };
    assert!(store.publish(0, &wrong).await.is_err());
    let good = ReceiptPublisher {
        calls: AtomicUsize::new(0),
        correct: true,
    };
    store.publish(0, &good).await.unwrap();
    store.publish(0, &good).await.unwrap();
    assert_eq!(good.calls.load(Ordering::SeqCst), 1);
    let round = store.decision(0).await.unwrap();
    sqlx::query("UPDATE proof_atlas_decision SET leaves = '\\x00'")
        .execute(f.database.pool())
        .await
        .unwrap();
    assert!(store.decision(0).await.is_err());
    assert!(store.publish(0, &good).await.is_err());
    assert!(!round.leaves.is_empty());
    f.close().await;
}

struct WaitingPublisher {
    started: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl RoundPublisher for WaitingPublisher {
    async fn publish(&self, doc: &RoundPublication) -> Result<String, RoundError> {
        self.started.notify_one();
        self.resume.notified().await;
        Ok(commitment(doc)?)
    }
}

#[tokio::test]
async fn late_publication_cannot_acknowledge_a_taken_over_attempt() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let store = empty_store(&f).await;
    let frozen = store.freeze(&source(&f).await, 0).await.unwrap();
    let lease = store.acquire(0, Uuid::new_v4(), 60).await.unwrap();
    store
        .decide(&lease, &decision(&frozen), &SEED)
        .await
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let publisher = WaitingPublisher {
        started: started.clone(),
        resume: resume.clone(),
    };
    let pending = {
        let store = store.clone();
        tokio::spawn(async move { store.publish(0, &publisher).await })
    };
    started.notified().await;
    assert!(store.publish(0, &Publisher).await.is_err());
    sqlx::query("UPDATE proof_atlas_publication SET expires_at = '-infinity'")
        .execute(f.database.pool())
        .await
        .unwrap();
    store.publish(0, &Publisher).await.unwrap();
    resume.notify_one();
    assert!(pending.await.unwrap().is_err());
    f.close().await;
}

#[tokio::test]
async fn immutable_inputs_and_outbox_bindings_cannot_be_updated_by_app_role() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let pool = f.database.app_pool().await.unwrap();
    for table in ["proof_atlas_round", "proof_atlas_decision"] {
        assert!(!db::current_user_can_update(&pool, table).await.unwrap());
        assert!(sqlx::query(&format!("DELETE FROM {table}"))
            .execute(&pool)
            .await
            .is_err());
    }
    for table in ["proof_atlas_publication", "proof_atlas_lease"] {
        assert!(sqlx::query(&format!("UPDATE {table} SET round = 99"))
            .execute(&pool)
            .await
            .is_err());
        assert!(sqlx::query(&format!("DELETE FROM {table}"))
            .execute(&pool)
            .await
            .is_err());
    }
    let owner = RoundStore::new(
        f.database.pool().clone(),
        ResearchStore::new(pool, scientific().0),
        config(),
    );
    assert!(owner.ready().await.is_err());
    f.close().await;
}

async fn private_call(app: &axum::Router, call: &RuntimeCall) -> (u16, serde_json::Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/call")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(call).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn private_atlas_ipc_reads_bound_observations_and_bounded_artifact_pages() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (research, summary) = completed(&f).await;
    let store = RoundStore::new(f.database.app_pool().await.unwrap(), research, config());
    store.freeze(&source(&f).await, 0).await.unwrap();
    let lease = store.acquire(0, Uuid::new_v4(), 60).await.unwrap();
    let operations = Arc::new(AtlasOperations::bind(store, lease, SEED).await.unwrap());
    let mut call = RuntimeCall {
        schema_version: 1,
        scope: operations.scope().clone(),
        operation: "read_evidence".into(),
        arguments: json!({"evidence_digest":summary.evidence_digest}),
    };
    let app = proof_runtime::private_router(operations);
    let (status, body) = private_call(&app, &call).await;
    assert_eq!(status, 200);
    let record = &body["result"]["evidence"];
    assert_eq!(
        record["observations"]["measurements"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    call.arguments = json!({"evidence_digest":summary.evidence_digest,
        "artifact_digest":record["recipe"]["candidate_script_digest"], "offset":0, "limit":8});
    let (status, page) = private_call(&app, &call).await;
    assert_eq!(status, 200);
    assert_eq!(
        hex::decode(page["result"]["hex"].as_str().unwrap()).unwrap(),
        b"print('l"
    );
    call.arguments["limit"] = json!(16 * 1024 + 1);
    assert_eq!(private_call(&app, &call).await.0, 403);
    call.arguments = json!({"evidence_digest":"f".repeat(64)});
    assert_eq!(private_call(&app, &call).await.0, 403);
    sqlx::query("UPDATE proof_atlas_lease SET expires_at = '-infinity'")
        .execute(f.database.pool())
        .await
        .unwrap();
    call.arguments = json!({"limit":1});
    assert_eq!(private_call(&app, &call).await.0, 403);
    f.close().await;
}
