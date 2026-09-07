#![allow(clippy::expect_used, clippy::unwrap_used)]

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
use proof_autonomy::{commitment, MachineQuote, ProvisionResult, ResourceGrant};
use proof_autonomy_pg::{ControllerLease, CreateExperiment};
use proof_eval::BaselineMeasurement;
use proof_research::{
    artifact_digest, Measurement, PairedMeasurement, ResearchStore, RetainedArtifacts,
    ScientificEvidence, ScientificRecipe,
};
use proof_runtime::*;
use proof_score::{AgentVerdict, HarnessMetrics, ProofKind};
use proof_task::{HoldoutSplit, ProofPin, TopicDocument, TopicStatus};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

fn metrics(nll: f64) -> HarnessMetrics {
    HarnessMetrics {
        holdout_nll: nll,
        split_nll: HoldoutSplit::SCORED
            .iter()
            .map(|s| (s.as_str().into(), nll))
            .collect(),
        ..HarnessMetrics::default()
    }
}

fn recipe() -> (ProofPin, ScientificRecipe, RetainedArtifacts) {
    let mut pin = ProofPin {
        topic_pubkey: common::miner(&SEED),
        eval_image_digest: format!("sha256:{}", "b".repeat(64)),
        ..ProofPin::default()
    };
    pin.inference.model = "local-faux".into();
    let baseline_script = b"print('synthetic baseline')".to_vec();
    let candidate_script = b"print('synthetic candidate')".to_vec();
    let baseline = BaselineMeasurement {
        eval_image_digest: pin.eval_image_digest.clone(),
        topic_id: "bridge-fixture".into(),
        holdout_commitment: "c".repeat(64),
        holdout_nll: 2.0,
        split_nll: metrics(2.0).split_nll,
        ..BaselineMeasurement::default()
    };
    let mut topic = TopicDocument {
        id: baseline.topic_id.clone(),
        statement: "Synthetic bridge fixture".into(),
        status: TopicStatus::Open,
        holdout_commitment: baseline.holdout_commitment.clone(),
        ..TopicDocument::default()
    };
    topic.baseline.script_sha256 = artifact_digest(&baseline_script);
    topic.baseline.metrics_commitment = baseline.commitment();
    topic.signature = topic.sign_with(&SEED).unwrap();
    let recipe = ScientificRecipe {
        schema_version: 1,
        topic,
        baseline,
        candidate_script_digest: artifact_digest(&candidate_script),
        seeds: vec![1, 2, 3],
        maximum_wall_ms: 1000,
    };
    (
        pin,
        recipe,
        RetainedArtifacts::from([
            (artifact_digest(&baseline_script), baseline_script),
            (artifact_digest(&candidate_script), candidate_script),
        ]),
    )
}

struct FauxExecutor {
    artifacts: RetainedArtifacts,
    slow: bool,
}
#[async_trait]
impl ExperimentExecutor for FauxExecutor {
    async fn execute(
        &self,
        _: &ControllerLease,
        grant: &ResourceGrant,
        _: &ExecutionRequest,
    ) -> Result<Value, RuntimeError> {
        assert_eq!(grant.resource_id, "bridge-pod");
        if self.slow {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
        Ok(json!({ "output": "local synthetic execution" }))
    }

    async fn collect(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        recipe: &ScientificRecipe,
    ) -> Result<(ScientificEvidence, RetainedArtifacts), RuntimeError> {
        let mut artifacts = self.artifacts.clone();
        let measurements = recipe
            .seeds
            .iter()
            .map(|seed| {
                let mut observation = |script: &str, nll: f64| {
                    let log = format!("synthetic measured fixture {seed} {nll}").into_bytes();
                    let digest = artifact_digest(&log);
                    artifacts.insert(digest.clone(), log);
                    Measurement {
                        seed: *seed,
                        script_digest: script.into(),
                        log_digest: digest,
                        exit_code: 0,
                        wall_ms: 50,
                        flops_used: 100,
                        metrics: metrics(nll),
                    }
                };
                PairedMeasurement {
                    baseline: observation(&recipe.topic.baseline.script_sha256, 2.0),
                    candidate: observation(&recipe.candidate_script_digest, 1.9),
                }
            })
            .collect();
        Ok((
            ScientificEvidence {
                schema_version: 1,
                experiment_id: lease.experiment_id,
                chain_epoch: 0,
                recipe_digest: commitment(recipe).unwrap(),
                resource_id: grant.resource_id.clone(),
                measurements,
                contamination_hits: vec![],
                verdict: AgentVerdict {
                    verdict: ProofKind::Clean,
                    reproduced: true,
                    claim_holds_public: true,
                    contamination: false,
                    canary_hit: false,
                    flops_used: 100,
                    flops_budget: recipe.topic.flops_budget,
                    cheat_codes: vec![],
                    rationale: "Private synthetic fixture".into(),
                    topic_id: recipe.topic.id.clone(),
                    family: recipe.topic.metric.family,
                },
            },
            artifacts,
        ))
    }
}

async fn setup(f: &Fixture, slow: bool) -> (Arc<ExperimentOperations>, ControllerLease) {
    let (pin, recipe, artifacts) = recipe();
    let research = ResearchStore::new(f.database.app_pool().await.unwrap(), pin.clone());
    let request = CreateExperiment {
        id: Uuid::new_v4(),
        account_id: f.account.id,
        recipe_digest: research.register_recipe(&recipe).await.unwrap(),
    };
    let e = f
        .store
        .create_experiment(
            &request,
            &signed(&request, "/v2/experiments", f.now().await, &SEED),
        )
        .await
        .unwrap();
    let lease = f.store.acquire(e.id, Uuid::new_v4(), 300).await.unwrap();
    let now = f.now().await;
    let quote = MachineQuote {
        schema_version: 1,
        id: Uuid::new_v4(),
        experiment_id: e.id,
        miner_hotkey: e.miner_hotkey.clone(),
        account_id: e.account_id,
        recipe_digest: e.recipe_digest.clone(),
        offer_id: "fixture".into(),
        gpu_type: "fixture".into(),
        gpu_count: 1,
        gpu_memory_mib: 100,
        ram_mib: 100,
        disk_gib: 10,
        image: pin.eval_image,
        image_digest: pin.eval_image_digest,
        hourly_total_microusd: 100,
        maximum_total_microusd: 100,
        lifetime_seconds: 3600,
        issued_at: now,
        expires_at: now + 120,
        provider_fingerprint: "d".repeat(64),
    };
    let e = f.store.publish_quote(&lease, 0, &quote).await.unwrap();
    let e = f
        .store
        .consent(e.id, e.revision, &consent(&quote))
        .await
        .unwrap();
    let intent = f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
    f.store
        .begin_provision(&lease, e.revision, intent)
        .await
        .unwrap();
    f.store
        .record_provision(
            &lease,
            intent,
            &ProvisionResult::Confirmed {
                resource_id: "bridge-pod".into(),
            },
        )
        .await
        .unwrap();
    f.store
        .adopt_resource(&lease, e.revision + 1)
        .await
        .unwrap();
    let operations = ExperimentOperations::bind(
        f.database.app_pool().await.unwrap(),
        research,
        lease,
        "bridge-pod".into(),
        Arc::new(FauxExecutor { artifacts, slow }),
    )
    .await
    .unwrap();
    (Arc::new(operations), lease)
}

fn call(scope: &RuntimeScope, operation: &str, arguments: Value) -> RuntimeCall {
    RuntimeCall {
        schema_version: 1,
        scope: scope.clone(),
        operation: operation.into(),
        arguments,
    }
}

async fn post(router: &Router, request: &RuntimeCall) -> (StatusCode, Vec<u8>) {
    let response = router
        .clone()
        .oneshot(
            Request::post("/call")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    (
        response.status(),
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

#[tokio::test]
async fn private_bridge_collects_only_controller_evidence_and_retains_reports_separately() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (operations, _) = setup(&f, false).await;
    let router = private_router(operations.clone());
    for (operation, arguments) in [
        ("quote", json!({})),
        (
            "execute",
            json!({"kind":"terminal","argv":["python","candidate.py"],"timeout_ms":1000}),
        ),
        (
            "kernel",
            json!({"kind":"kernel","code":"print(42)","timeout_ms":1000}),
        ),
        (
            "report",
            json!({"text":"Agent says this is successful, but it is not evidence."}),
        ),
        ("collect", json!({})),
        ("read_evidence", json!({})),
    ] {
        let (status, body) = post(&router, &call(operations.scope(), operation, arguments)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{operation}: {}",
            String::from_utf8_lossy(&body)
        );
        assert!(!String::from_utf8_lossy(&body).contains("credential_ref"));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_scientific_evidence")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_agent_report")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.close().await;
}

#[tokio::test]
async fn bridge_denies_scope_changes_raw_evidence_and_stale_owners() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (operations, lease) = setup(&f, false).await;
    let router = private_router(operations.clone());
    for (operation, arguments) in [
        ("submit_decision", json!({})),
        ("collect", json!({"measurements":[]})),
        (
            "execute",
            json!({"kind":"terminal","argv":["true"],"timeout_ms":1000,"resource_id":"foreign"}),
        ),
        ("report", json!({"text":"claim", "passed":true})),
    ] {
        assert_eq!(
            post(&router, &call(operations.scope(), operation, arguments))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    let mut wrong = call(operations.scope(), "quote", json!({}));
    wrong.scope.id = Uuid::new_v4().to_string();
    assert_eq!(post(&router, &wrong).await.0, StatusCode::FORBIDDEN);
    f.expire(&lease).await;
    f.store
        .acquire(lease.experiment_id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    assert_eq!(
        post(&router, &call(operations.scope(), "quote", json!({})))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.close().await;
}

#[tokio::test]
async fn controller_rechecks_ownership_during_slow_execution() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (operations, lease) = setup(&f, true).await;
    let request = call(
        operations.scope(),
        "kernel",
        json!({"kind":"kernel","code":"print(42)","timeout_ms":25000}),
    );
    let running = tokio::spawn(async move { operations.call(request).await });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    f.expire(&lease).await;
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
    f.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn socket_requires_a_private_parent_and_never_replaces_an_existing_path() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("controller.sock");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(bind_private_socket(&path).is_err());
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = bind_private_socket(&path).unwrap();
    assert!(bind_private_socket(&path).is_err());
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(listener);
}

#[cfg(unix)]
#[tokio::test]
async fn typescript_runtime_round_trip_uses_real_postgres_and_private_ipc() {
    use std::os::unix::fs::PermissionsExt;
    if std::env::var_os("CORTEX_TEST_RUNTIME_BRIDGE").is_none() {
        eprintln!("Set CORTEX_TEST_RUNTIME_BRIDGE=1 for the Node/kernel/Postgres integration");
        return;
    }
    let f = Fixture::new()
        .await
        .expect("bridge integration requires disposable Postgres");
    let (operations, _) = setup(&f, false).await;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("controller.sock");
    let listener = bind_private_socket(&path).unwrap();
    let (shutdown, stopping) = tokio::sync::oneshot::channel();
    let router = private_router(operations.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopping.await;
            })
            .await
            .unwrap();
    });
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../agents/atlas");
    let mut command = tokio::process::Command::new(root.join("node_modules/.bin/tsx"));
    command
        .arg(root.join("packages/coding-agent/test/fixtures/cortex-service-roundtrip.ts"))
        .current_dir(root.join("packages/coding-agent"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", directory.path())
        .env("CORTEX_BRIDGE_SOCKET", &path)
        .env(
            "CORTEX_BRIDGE_SCOPE",
            serde_json::to_string(operations.scope()).unwrap(),
        )
        .kill_on_drop(true);
    for key in [
        "PRIME_AGENT_KERNEL_PYTHON",
        "PYTHONPATH",
        "CORTEX_TEST_KERNEL_IMAGE",
    ] {
        command.env(
            key,
            std::env::var_os(key).expect("local kernel prerequisite"),
        );
    }
    let output = tokio::time::timeout(std::time::Duration::from_secs(90), command.output()).await;
    let _ = shutdown.send(());
    server.await.unwrap();
    let output = output.unwrap().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_scientific_evidence")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.close().await;
}
