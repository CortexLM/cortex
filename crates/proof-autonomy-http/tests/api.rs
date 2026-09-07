#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../proof-autonomy-pg/tests/common/mod.rs"]
mod common;

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use common::{consent, signed, Fixture, SEED};
use http_body_util::BodyExt;
use proof_autonomy::{commitment, DeletionResult, ExperimentState, MachineQuote, ProvisionResult};
use proof_autonomy_pg::{
    CancelExperiment, Experiment, MinerAccount, PgStore, Resource, ViewExperiment,
};
use proof_broker::{Broker, MinerProvider, ProviderError};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

async fn post(app: &Router, path: &str, value: Value) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

struct LocalProvider {
    account: Uuid,
    calls: Arc<AtomicUsize>,
}

fn quote_for(e: &Experiment, now: u64) -> MachineQuote {
    MachineQuote {
        schema_version: 1,
        id: Uuid::new_v4(),
        experiment_id: e.id,
        miner_hotkey: e.miner_hotkey.clone(),
        account_id: e.account_id,
        recipe_digest: e.recipe_digest.clone(),
        offer_id: "local-offer".into(),
        gpu_type: "local-gpu".into(),
        gpu_count: 1,
        gpu_memory_mib: 1024,
        ram_mib: 1024,
        disk_gib: 10,
        image: "invalid.example/local".into(),
        image_digest: format!("sha256:{}", "b".repeat(64)),
        hourly_total_microusd: 1_000,
        maximum_total_microusd: 1_000,
        lifetime_seconds: 3600,
        issued_at: now,
        expires_at: now + 120,
        provider_fingerprint: "c".repeat(64),
    }
}

#[async_trait]
impl MinerProvider for LocalProvider {
    async fn preflight(
        &self,
        account: &MinerAccount,
        quote: &MachineQuote,
    ) -> Result<String, ProviderError> {
        assert_eq!(account.id, self.account);
        Ok(commitment(quote).unwrap())
    }
    async fn rent(&self, _: &MinerAccount, _: &MachineQuote, _: Uuid) -> ProvisionResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ProvisionResult::Confirmed {
            resource_id: "local-proof-pod".into(),
        }
    }
    async fn reconcile(&self, _: &MinerAccount, _: &MachineQuote, _: Uuid) -> ProvisionResult {
        ProvisionResult::Uncertain
    }
    async fn delete(
        &self,
        account: &MinerAccount,
        resource: &Resource,
    ) -> Result<(), ProviderError> {
        assert_eq!(account.id, self.account);
        assert_eq!(resource.resource_id, "local-proof-pod");
        Ok(())
    }
    async fn deletion_status(&self, _: &MinerAccount, _: &Resource) -> DeletionResult {
        DeletionResult::Confirmed
    }
}

#[tokio::test]
async fn signed_http_submission_consent_rent_cancel_and_verified_cleanup() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let app = proof_autonomy_http::router(f.store.clone()).await.unwrap();
    let request = f.request();
    let create = json!({"request": request, "authorization": signed(&request, "/v2/experiments", f.now().await, &SEED)});
    let (status, body) = post(&app, "/v2/experiments", create.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let e: Experiment = serde_json::from_str(&body).unwrap();
    assert_eq!(
        post(&app, "/v2/experiments", create).await.0,
        StatusCode::CONFLICT
    );
    let lease = f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap();
    let now = f.now().await;
    let quote = quote_for(&e, now);
    let e = f
        .store
        .publish_quote(&lease, e.revision, &quote)
        .await
        .unwrap();
    let path = format!("/v2/experiments/{}/view", e.id);
    let view = ViewExperiment {
        experiment_id: e.id,
    };
    let (status, body) = post(
        &app,
        &path,
        json!({"request": view, "authorization": signed(&view, &path, now, &SEED)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["quote"]["id"],
        quote.id.to_string()
    );
    assert!(!body.contains("credential_ref"));
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = Broker::new(
        f.store.clone(),
        LocalProvider {
            account: f.account.id,
            calls: calls.clone(),
        },
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(f
        .store
        .intents(e.id, &e.miner_hotkey)
        .await
        .unwrap()
        .is_empty());
    let path = format!("/v2/experiments/{}/consent", e.id);
    let approved = json!({"revision": e.revision, "consent": consent(&quote)});
    let (status, body) = post(&app, &path, approved.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(post(&app, &path, approved).await.0, StatusCode::CONFLICT);
    let e: Experiment = serde_json::from_str(&body).unwrap();
    let intent = f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
    broker.provision(&lease, e.revision, intent).await.unwrap();
    let e = f
        .store
        .adopt_resource(&lease, e.revision + 1)
        .await
        .unwrap();
    let path = format!("/v2/experiments/{}/cancel", e.id);
    let cancel = CancelExperiment {
        experiment_id: e.id,
        revision: e.revision,
    };
    let (status, body) = post(
        &app,
        &path,
        json!({"request": cancel, "authorization": signed(&cancel, &path, now, &SEED)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Experiment>(&body).unwrap().state,
        ExperimentState::Cancelling
    );
    assert!(broker.cleanup(&lease, "local-proof-pod").await.is_err());
    let cleanup = f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap();
    assert_eq!(
        broker.cleanup(&cleanup, "local-proof-pod").await.unwrap(),
        DeletionResult::Confirmed
    );
    assert_eq!(
        f.store.finish_cleanup(&cleanup).await.unwrap().state,
        ExperimentState::Cancelled
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn forged_identity_wrong_path_and_privileged_routes_are_rejected() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let app = proof_autonomy_http::router(f.store.clone()).await.unwrap();
    let e = f.create().await;
    let path = format!("/v2/experiments/{}/view", e.id);
    let view = ViewExperiment {
        experiment_id: e.id,
    };
    let (status, _) = post(
        &app,
        &path,
        json!({"request": view, "authorization": signed(&view, &path, f.now().await, &[9;32])}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = post(
        &app,
        &path,
        json!({"request": view, "authorization": signed(&view, "/wrong", f.now().await, &SEED)}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for suffix in [
        "provision",
        "record_provision",
        "quote",
        "acquire",
        "evidence",
        "decision",
    ] {
        assert_eq!(
            post(
                &app,
                &format!("/v2/experiments/{}/{suffix}", e.id),
                json!({})
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let marker = "synthetic-private-payload";
    let (status, response) = post(
        &app,
        "/v2/experiments",
        json!({"request": marker, "authorization": {}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!response.contains(marker));
    assert_eq!(
        post(
            &app,
            "/v2/experiments",
            json!({"padding": "x".repeat(40_000)})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    f.close().await;
}

#[tokio::test]
async fn startup_refuses_an_owner_database_connection() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    assert!(
        proof_autonomy_http::router(PgStore::new(f.database.pool().clone()))
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn intake_quota_is_atomic_and_cancellation_is_never_quota_blocked() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let app = proof_autonomy_http::router(f.store.clone()).await.unwrap();
    let first = f.create().await;
    for _ in 0..6 {
        f.create().await;
    }
    let a = f.request();
    let b = f.request();
    let now = f.now().await;
    let ra = json!({"request": a, "authorization": signed(&a, "/v2/experiments", now, &SEED)});
    let rb = json!({"request": b, "authorization": signed(&b, "/v2/experiments", now, &SEED)});
    let (a, b) = tokio::join!(
        post(&app, "/v2/experiments", ra),
        post(&app, "/v2/experiments", rb)
    );
    let mut codes = [a.0.as_u16(), b.0.as_u16()];
    codes.sort_unstable();
    assert_eq!(codes, [201, 429]);
    let path = format!("/v2/experiments/{}/cancel", first.id);
    let request = CancelExperiment {
        experiment_id: first.id,
        revision: first.revision,
    };
    assert_eq!(
        post(
            &app,
            &path,
            json!({"request": request, "authorization": signed(&request, &path, now, &SEED)})
        )
        .await
        .0,
        StatusCode::OK
    );
    f.close().await;
}
