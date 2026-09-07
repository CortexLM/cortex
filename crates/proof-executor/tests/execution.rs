#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

#[path = "../../proof-autonomy-pg/tests/common/mod.rs"]
mod common;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use common::{consent, signed, Fixture, SEED};
use proof_autonomy::{
    commitment, CapabilityOperation, MachineQuote, ProvisionResult, ResourceGrant,
};
use proof_autonomy_pg::{ControllerLease, CreateExperiment};
use proof_eval::BaselineMeasurement;
use proof_executor::{DockerSandbox, DurableExecutor, EpochObservation, Failure, TrustedChain};
use proof_measure::DockerObserver;
use proof_research::{artifact_digest, ResearchStore, ScientificRecipe};
use proof_runtime::{ExecutionRequest, ExperimentExecutor};
use proof_task::{
    holdout_commitment, synthetic_holdout, HoldoutRecord, HoldoutSplit, InferenceConfig,
    InferenceMode, InferenceOffer, InferenceProvider, InferenceProviderKind, OfferStatus, ProofPin,
    TopicDocument, TopicStatus, STRATUM_SIZE,
};
use serde_json::{json, Value};
use uuid::Uuid;

const IMAGE: &str = "sha256:0104307df448338d8475c7cf8152e5e0655e211fd1c04b2bdc94e6758a7e7293";
const BASELINE: &[u8] = b"import os,sys\nx=1.0\nfor i in range(10000): x=x*1.00000001+0.000001\nprint(os.environ['PROOF_SEED'],sys.argv[1],x)\n";
const CANDIDATE: &[u8] = b"import os,sys,json\nx=1.0\nfor i in range(10000): x=x*1.00000002+0.000001\nprint(os.environ['PROOF_SEED'],sys.argv[1],x)\nprint(json.dumps({'metrics':{'holdout_nll':0.0},'flops_used':1}))\n";

/// TEST-ONLY observer image built from crates/proof-measure/tests/fixtures.
/// It is not the scoring image; it only honours the CLI contract.
const OBSERVER_IMAGE: &str = "cortex-test-observer:local-test";

fn test_offer() -> InferenceOffer {
    let config = InferenceConfig {
        mode: InferenceMode::Chat,
        model_ref: "local-test-only".into(),
        max_input_tokens: 1024,
        max_output_tokens: 64,
        temperature: None,
        top_p: None,
        timeout_ms: None,
    };
    let base_url = "http://127.0.0.1:1/v1".to_owned();
    InferenceOffer {
        offer_id: "local-test-only".into(),
        config_commitment: proof_task::inference_config_commitment(&config, &base_url),
        provider: InferenceProvider {
            kind: InferenceProviderKind::OpenaiCompatible,
            base_url,
        },
        config,
        status: OfferStatus::Open,
    }
}

/// Primed holdout store plus records; the directory is removed on drop.
struct HoldoutStore(PathBuf, Vec<HoldoutRecord>);
impl HoldoutStore {
    fn new() -> Self {
        let records = synthetic_holdout(STRATUM_SIZE, 1);
        let path = std::env::temp_dir().join(format!("cortex-exec-holdout-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        for record in &records {
            std::fs::write(path.join(&record.content_sha256), b"shard").unwrap();
        }
        Self(path, records)
    }
}
impl Drop for HoldoutStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn observer_image_id() -> String {
    docker_get(&format!("/images/{OBSERVER_IMAGE}/json"))
        .await
        .1["Id"]
        .as_str()
        .unwrap()
        .to_owned()
}

// Explicit local chain fixture, never a production executor constructor default.
struct TestChain(bool);
#[async_trait]
impl TrustedChain for TestChain {
    async fn finalized_epoch(&self) -> Result<EpochObservation, Failure> {
        if !self.0 {
            return Err(Failure::Chain);
        }
        Ok(EpochObservation {
            chain_epoch: 1,
            block: 123,
            hash: [7; 32],
        })
    }
}

struct Test {
    fixture: Fixture,
    executor: DurableExecutor,
    lease: ControllerLease,
    grant: ResourceGrant,
    recipe: ScientificRecipe,
}

impl Test {
    async fn new(baseline_source: &[u8], candidate_source: &[u8], chain_ready: bool) -> Self {
        Self::with_observer(baseline_source, candidate_source, chain_ready, None).await
    }

    /// `observer` is `(observer image id, holdout records)`; when set, the pin
    /// names that TEST-ONLY observer image and the executor gets a
    /// `DockerObserver` over it. The sandbox still runs the pinned kernel image.
    async fn with_observer(
        baseline_source: &[u8],
        candidate_source: &[u8],
        chain_ready: bool,
        observer: Option<(String, Vec<HoldoutRecord>, PathBuf)>,
    ) -> Self {
        let fixture = Fixture::new()
            .await
            .expect("DATABASE_URL required for ignored integration tests");
        let target = DockerSandbox::connect(Path::new("/var/run/docker.sock"), IMAGE)
            .await
            .expect("pinned local image");
        let eval_image = observer
            .as_ref()
            .map_or(IMAGE.to_owned(), |(image, _, _)| image.clone());
        let mut pin = ProofPin {
            topic_pubkey: common::miner(&SEED),
            eval_image_digest: eval_image.clone(),
            ..ProofPin::default()
        };
        pin.inference.model = "local-test-only".into();
        let holdout_commitment = observer.as_ref().map_or("c".repeat(64), |(_, records, _)| {
            holdout_commitment(records)
        });
        let baseline = BaselineMeasurement {
            eval_image_digest: eval_image,
            topic_id: "executor-test-only".into(),
            holdout_commitment,
            holdout_nll: 2.0,
            split_nll: HoldoutSplit::SCORED
                .iter()
                .map(|s| (s.as_str().into(), 2.0))
                .collect(),
            ..BaselineMeasurement::default()
        };
        let mut topic = TopicDocument {
            id: baseline.topic_id.clone(),
            statement: "Non-scientific CPU execution integration fixture".into(),
            status: TopicStatus::Open,
            holdout_commitment: baseline.holdout_commitment.clone(),
            ..TopicDocument::default()
        };
        topic.baseline.script_sha256 = artifact_digest(baseline_source);
        topic.baseline.metrics_commitment = baseline.commitment();
        topic.signature = topic.sign_with(&SEED).unwrap();
        let recipe = ScientificRecipe {
            schema_version: 1,
            topic,
            baseline,
            candidate_script_digest: artifact_digest(candidate_source),
            seeds: vec![5, 9123, u64::MAX],
            maximum_wall_ms: 3000,
        };
        let pool = fixture.database.app_pool().await.unwrap();
        let research = ResearchStore::new(pool.clone(), pin.clone());
        let digest = research.register_recipe(&recipe).await.unwrap();
        let request = CreateExperiment {
            id: Uuid::new_v4(),
            account_id: fixture.account.id,
            recipe_digest: digest,
        };
        let e = fixture
            .store
            .create_experiment(
                &request,
                &signed(&request, "/v2/experiments", fixture.now().await, &SEED),
            )
            .await
            .unwrap();
        let lease = fixture
            .store
            .acquire(e.id, Uuid::new_v4(), 120)
            .await
            .unwrap();
        let now = fixture.now().await;
        let quote = MachineQuote {
            schema_version: 1,
            id: Uuid::new_v4(),
            experiment_id: e.id,
            miner_hotkey: e.miner_hotkey.clone(),
            account_id: e.account_id,
            recipe_digest: e.recipe_digest.clone(),
            offer_id: "local-test-only".into(),
            gpu_type: "no-gpu-cpu-fixture".into(),
            // Quote schema requires a GPU; no real GPU was offered or used.
            gpu_count: 1,
            gpu_memory_mib: 100,
            ram_mib: 256,
            disk_gib: 1,
            image: pin.eval_image,
            image_digest: IMAGE.into(),
            hourly_total_microusd: 1,
            maximum_total_microusd: 1,
            lifetime_seconds: 3600,
            issued_at: now,
            expires_at: now + 120,
            provider_fingerprint: "d".repeat(64),
        };
        let e = fixture
            .store
            .publish_quote(&lease, 0, &quote)
            .await
            .unwrap();
        let e = fixture
            .store
            .consent(e.id, e.revision, &consent(&quote))
            .await
            .unwrap();
        let intent = fixture.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
        fixture
            .store
            .begin_provision(&lease, e.revision, intent)
            .await
            .unwrap();
        fixture
            .store
            .record_provision(
                &lease,
                intent,
                &ProvisionResult::Confirmed {
                    resource_id: "controller-owned-local-target".into(),
                },
            )
            .await
            .unwrap();
        fixture
            .store
            .adopt_resource(&lease, e.revision + 1)
            .await
            .unwrap();
        let grant = fixture
            .store
            .authorize_resource(
                &lease,
                "controller-owned-local-target",
                CapabilityOperation::Execute,
            )
            .await
            .unwrap();
        let mut executor =
            DurableExecutor::new(pool, research, target, Arc::new(TestChain(chain_ready)));
        if let Some((image, records, store)) = observer {
            let observer = DockerObserver::connect(
                Path::new("/var/run/docker.sock"),
                &image,
                store,
                BTreeMap::from([(recipe.topic.id.clone(), records)]),
                test_offer(),
            )
            .await
            .expect("TEST-ONLY observer image present locally");
            executor = executor.with_observer(Arc::new(observer));
        }
        executor.bind_local_target(&lease, &grant).await.unwrap();
        Self {
            fixture,
            executor,
            lease,
            grant,
            recipe,
        }
    }

    async fn source(&self, baseline: &[u8], candidate: &[u8]) {
        self.executor.retain_script(baseline).await.unwrap();
        self.executor.retain_script(candidate).await.unwrap();
    }

    async fn state(&self) -> Option<String> {
        sqlx::query_scalar(
            "SELECT state FROM proof_execution_intent ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_optional(self.fixture.database.pool())
        .await
        .unwrap()
    }

    async fn wait_finished(&self) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if self
                    .state()
                    .await
                    .as_deref()
                    .is_some_and(|s| ["failed", "completed", "reconcile"].contains(&s))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
    }

    async fn wait_running(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let intents = self.executor.intents(&self.lease).await.unwrap();
                if let Some(intent) = intents.first() {
                    let response =
                        docker_get(&format!("/containers/base-proof-exec-{}-0/json", intent.id))
                            .await;
                    if response.0 == 200
                        && response.1.pointer("/State/Running") == Some(&json!(true))
                    {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }

    async fn absent(&self) {
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM proof_execution_intent")
            .fetch_all(self.fixture.database.pool())
            .await
            .unwrap();
        for id in ids {
            for i in 0..6 {
                assert_eq!(
                    docker_get(&format!("/containers/base-proof-exec-{id}-{i}/json"))
                        .await
                        .0,
                    404
                );
            }
        }
    }

    async fn failure(&self, expected: &str) {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_execution_observation WHERE observation->>'failure' = $1)",
        ).bind(expected).fetch_one(self.fixture.database.pool()).await.unwrap();
        assert!(found, "missing persisted failure {expected}");
    }

    async fn no_evidence(&self) {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_scientific_evidence")
            .fetch_one(self.fixture.database.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}

async fn docker_get(path: &str) -> (u16, Value) {
    let response = reqwest::Client::builder()
        .unix_socket("/var/run/docker.sock")
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!("http://localhost{path}"))
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

fn kernel(code: &str, timeout_ms: u32) -> ExecutionRequest {
    ExecutionRequest::Kernel {
        code: code.into(),
        timeout_ms,
    }
}

#[tokio::test]
async fn floating_image_and_relative_socket_fail_without_daemon() {
    assert!(
        DockerSandbox::connect(Path::new("/missing"), "python:latest")
            .await
            .is_err()
    );
    assert!(DockerSandbox::connect(Path::new("docker.sock"), IMAGE)
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL schemas and exact local Docker image; no network/rent"]
async fn paired_cpu_runs_retain_real_bytes_but_never_accept_claimed_measurements() {
    // Without a trusted observer the collect is refused in preflight: no
    // sandbox run happens, the failure is journaled, and nothing is admitted.
    let t = Test::new(BASELINE, CANDIDATE, true).await;
    t.source(BASELINE, CANDIDATE).await;
    assert!(t
        .executor
        .collect(&t.lease, &t.grant, &t.recipe)
        .await
        .is_err());
    assert_eq!(t.state().await.as_deref(), Some("failed"));
    t.failure("unobserved_measurements").await;
    let runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM proof_execution_observation WHERE observation->>'event' = 'run'",
    )
    .fetch_one(t.fixture.database.pool())
    .await
    .unwrap();
    assert_eq!(runs, 0);
    // Same committed operation may not execute again after either failure or takeover.
    assert!(t
        .executor
        .collect(&t.lease, &t.grant, &t.recipe)
        .await
        .is_err());
    assert_eq!(t.executor.intents(&t.lease).await.unwrap().len(), 1);
    t.no_evidence().await;
    t.absent().await;
    t.fixture.close().await;
}

#[tokio::test]
#[ignore = "requires local PostgreSQL and pinned Docker"]
async fn missing_source_or_dependency_persists_failure_without_fabricated_evidence() {
    let t = Test::new(BASELINE, CANDIDATE, true).await;
    t.executor.retain_script(BASELINE).await.unwrap();
    assert!(t
        .executor
        .collect(&t.lease, &t.grant, &t.recipe)
        .await
        .is_err());
    t.failure("missing_script").await;
    t.absent().await;
    t.no_evidence().await;
    t.fixture.close().await;
    // With an observer the sandbox runs; a failed dependency is retained, not measured.
    let store = HoldoutStore::new();
    let missing = b"import cortex_intentionally_absent_dependency\n";
    let t = Test::with_observer(
        missing,
        CANDIDATE,
        true,
        Some((observer_image_id().await, store.1.clone(), store.0.clone())),
    )
    .await;
    t.source(missing, CANDIDATE).await;
    assert!(t
        .executor
        .collect(&t.lease, &t.grant, &t.recipe)
        .await
        .is_err());
    t.failure("execution").await;
    let bytes: Vec<Vec<u8>> = sqlx::query_scalar("SELECT bytes FROM proof_execution_artifact")
        .fetch_all(t.fixture.database.pool())
        .await
        .unwrap();
    let observed = bytes
        .iter()
        .filter_map(|b| serde_json::from_slice::<proof_executor::RunObservation>(b).ok())
        .collect::<Vec<_>>();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].exit_code, Some(1));
    assert!(String::from_utf8_lossy(&observed[0].log).contains("ModuleNotFoundError"));
    t.absent().await;
    t.no_evidence().await;
    t.fixture.close().await;
}

#[tokio::test]
#[ignore = "requires local PostgreSQL and pinned Docker"]
async fn bounded_terminal_is_isolated_and_not_a_persistent_agent_kernel() {
    let t = Test::new(BASELINE, CANDIDATE, true).await;
    let code = "import os,socket\nassert os.getuid()==65532\nassert not os.path.exists('/var/run/docker.sock')\nassert not os.path.exists('/root/cortex')\ntry:\n os.kill(1,9)\nexcept PermissionError:\n print('supervisor-protected')\nelse:\n raise RuntimeError('supervisor writable')\ntry:\n open('/etc/proof-host-write','w')\nexcept OSError:\n print('read-only-root')\nelse:\n raise RuntimeError('root writable')\nprint('cpu-only')\n";
    let result = t
        .executor
        .execute(&t.lease, &t.grant, &kernel(code, 3000))
        .await;
    let observations: Vec<Value> =
        sqlx::query_scalar("SELECT observation FROM proof_execution_observation")
            .fetch_all(t.fixture.database.pool())
            .await
            .unwrap();
    assert!(result.is_ok(), "{observations:?}");
    let result = result.unwrap();
    assert!(result["log"]
        .as_str()
        .unwrap()
        .contains("supervisor-protected"));
    assert!(result["log"].as_str().unwrap().contains("read-only-root"));
    assert_eq!(result["scientific_measurement"], false);
    assert_eq!(t.state().await.as_deref(), Some("completed"));
    let terminal = ExecutionRequest::Terminal {
        argv: vec!["/bin/echo".into(), "actual-terminal".into()],
        timeout_ms: 3000,
    };
    let result = t
        .executor
        .execute(&t.lease, &t.grant, &terminal)
        .await
        .unwrap();
    assert_eq!(result["log"], "actual-terminal\n");
    t.absent().await;
    t.fixture.close().await;
}

#[tokio::test]
#[ignore = "requires local PostgreSQL and pinned Docker"]
async fn deadline_and_log_overflow_stop_entire_pid_namespace() {
    let t = Test::new(BASELINE, CANDIDATE, true).await;
    let descendants = "import os,time,signal\nif os.fork()==0:\n os.setsid()\n signal.signal(signal.SIGTERM,signal.SIG_IGN)\n while True: time.sleep(.01)\nwhile True: time.sleep(.01)\n";
    assert!(t
        .executor
        .execute(&t.lease, &t.grant, &kernel(descendants, 200))
        .await
        .is_err());
    t.absent().await;
    let flood = "import os\nwhile True: os.write(1,b'x'*4096)\n";
    assert!(t
        .executor
        .execute(&t.lease, &t.grant, &kernel(flood, 3000))
        .await
        .is_err());
    t.absent().await;
    t.fixture.close().await;
}

#[tokio::test]
#[ignore = "requires local PostgreSQL and pinned Docker"]
async fn dropped_caller_does_not_drop_cleanup_or_original_fence_failure() {
    let t = Test::new(BASELINE, CANDIDATE, true).await;
    let executor = t.executor.clone();
    let lease = t.lease;
    let grant = t.grant.clone();
    let task = tokio::spawn(async move {
        executor
            .execute(
                &lease,
                &grant,
                &kernel("import time\ntime.sleep(20)\n", 25000),
            )
            .await
    });
    t.wait_running().await;
    assert!(t
        .executor
        .execute(
            &t.lease,
            &t.grant,
            &kernel("print('blocked kernel must not retain new source')", 1000)
        )
        .await
        .is_err());
    let sources: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_execution_script")
        .fetch_one(t.fixture.database.pool())
        .await
        .unwrap();
    assert_eq!(
        sources, 1,
        "conflicting dispatch must not bypass source admission quota"
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    t.wait_finished().await;
    t.failure("interrupted").await;
    t.absent().await;
    let fences: Vec<i64> =
        sqlx::query_scalar("SELECT controller_fence FROM proof_execution_observation")
            .fetch_all(t.fixture.database.pool())
            .await
            .unwrap();
    assert!(!fences.is_empty() && fences.iter().all(|f| *f == t.lease.fence));
    t.fixture.close().await;
}

#[tokio::test]
#[ignore = "requires local PostgreSQL and pinned Docker"]
async fn takeover_and_revocation_cannot_lose_failures_or_rerun_work() {
    let t = Test::new(BASELINE, CANDIDATE, true).await;
    let request = kernel("import time\ntime.sleep(21)\n", 25000);
    let executor = t.executor.clone();
    let lease = t.lease;
    let grant = t.grant.clone();
    let task = tokio::spawn(async move { executor.execute(&lease, &grant, &request).await });
    t.wait_running().await;
    t.fixture.expire(&t.lease).await;
    let newer = t
        .fixture
        .store
        .acquire(t.lease.experiment_id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    assert!(task.await.unwrap().is_err());
    t.wait_finished().await;
    t.failure("interrupted").await;
    t.absent().await;
    assert!(t
        .executor
        .execute(
            &newer,
            &t.grant,
            &kernel("import time\ntime.sleep(21)\n", 25000)
        )
        .await
        .is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_execution_intent")
        .fetch_one(t.fixture.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let fences: Vec<i64> =
        sqlx::query_scalar("SELECT controller_fence FROM proof_execution_observation")
            .fetch_all(t.fixture.database.pool())
            .await
            .unwrap();
    assert!(fences.iter().all(|f| *f == t.lease.fence));
    sqlx::query("UPDATE proof_miner_account SET revoked=true")
        .execute(t.fixture.database.pool())
        .await
        .unwrap();
    assert!(t
        .executor
        .execute(&newer, &t.grant, &kernel("print('revoked')", 1000))
        .await
        .is_err());
    t.fixture.close().await;
}

#[tokio::test]
#[ignore = "requires local PostgreSQL and pinned Docker"]
async fn chain_failure_and_forged_grant_fail_before_execution() {
    let t = Test::new(BASELINE, CANDIDATE, false).await;
    t.source(BASELINE, CANDIDATE).await;
    let mut forged = t.grant.clone();
    forged.resource_id = "container-name-auth-bypass".into();
    assert!(t
        .executor
        .execute(&t.lease, &forged, &kernel("print(1)", 1000))
        .await
        .is_err());
    assert!(t.executor.intents(&t.lease).await.unwrap().is_empty());
    assert!(t
        .executor
        .collect(&t.lease, &t.grant, &t.recipe)
        .await
        .is_err());
    t.failure("chain").await;
    t.absent().await;
    t.no_evidence().await;
    t.fixture.close().await;
}

#[tokio::test]
#[ignore = "requires local PostgreSQL and pinned Docker"]
async fn ambiguous_intent_is_reconciled_without_dispatch_and_preserves_original_fence() {
    let t = Test::new(BASELINE, CANDIDATE, true).await;
    let plan = json!({"schema_version":1,"recipe_digest":null,"runs":[{"kind":"terminal","argv":["/bin/true"],"timeout_ms":1000}]});
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO proof_execution_intent (id,experiment_id,operation_key,controller_fence,owner_id,resource_id,engine_id,image_id,plan,deadline_ms) \
         SELECT $1,$2,$3,$4,$5,resource_id,engine_id,image_id,$6,(extract(epoch FROM clock_timestamp())*1000)::bigint+10000 \
         FROM proof_execution_target WHERE experiment_id=$2",
    ).bind(id).bind(t.lease.experiment_id).bind(commitment(&plan).unwrap()).bind(t.lease.fence)
        .bind(t.lease.owner_id).bind(sqlx::types::Json(plan)).execute(t.fixture.database.pool()).await.unwrap();
    t.fixture.expire(&t.lease).await;
    let newer = t
        .fixture
        .store
        .acquire(t.lease.experiment_id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    assert_eq!(
        t.executor.reconcile(&newer).await.unwrap_err(),
        Failure::Reconcile
    );
    assert_eq!(t.state().await.as_deref(), Some("reconcile"));
    sqlx::query("UPDATE proof_execution_intent SET deadline_ms = 1")
        .execute(t.fixture.database.pool())
        .await
        .unwrap();
    t.executor.reconcile(&newer).await.unwrap();
    assert_eq!(t.state().await.as_deref(), Some("failed"));
    t.absent().await;
    let fences: Vec<i64> =
        sqlx::query_scalar("SELECT controller_fence FROM proof_execution_observation")
            .fetch_all(t.fixture.database.pool())
            .await
            .unwrap();
    assert!(fences.iter().all(|f| *f == t.lease.fence));
    let app = t.fixture.database.app_pool().await.unwrap();
    assert!(
        sqlx::query("UPDATE proof_execution_intent SET deadline_ms=deadline_ms")
            .execute(&app)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE proof_execution_artifact SET bytes=bytes")
            .execute(&app)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM proof_execution_observation")
        .execute(&app)
        .await
        .is_err());
    t.fixture.close().await;
}

/// Workload scripts that leave a model artifact for the trusted observer. The
/// NLL the TEST-ONLY observer reports comes from `nll.txt`, so baseline (2.0,
/// matching the sealed baseline) and candidate (1.0) are steered here.
const OBSERVED_BASELINE: &[u8] = b"import os,sys\nopen(os.environ['PROOF_ARTIFACT_DIR']+'/nll.txt','w').write('2.0')\nopen(os.environ['PROOF_ARTIFACT_DIR']+'/weights.bin','wb').write(b'baseline'+os.environ['PROOF_SEED'].encode())\nprint(sys.argv[1])\n";
const OBSERVED_CANDIDATE: &[u8] = b"import os,sys\nopen(os.environ['PROOF_ARTIFACT_DIR']+'/nll.txt','w').write('1.0')\nopen(os.environ['PROOF_ARTIFACT_DIR']+'/weights.bin','wb').write(b'candidate'+os.environ['PROOF_SEED'].encode())\nprint(sys.argv[1])\n";

#[tokio::test]
#[ignore = "requires isolated PostgreSQL, pinned kernel image and the TEST-ONLY cortex-test-observer:local-test image"]
async fn trusted_observer_turns_paired_runs_into_evidence_and_no_observer_still_fails() {
    let store = HoldoutStore::new();
    let t = Test::with_observer(
        OBSERVED_BASELINE,
        OBSERVED_CANDIDATE,
        true,
        Some((observer_image_id().await, store.1.clone(), store.0.clone())),
    )
    .await;
    t.source(OBSERVED_BASELINE, OBSERVED_CANDIDATE).await;
    let (evidence, artifacts) = t
        .executor
        .collect(&t.lease, &t.grant, &t.recipe)
        .await
        .expect("observed collect yields evidence");
    assert_eq!(t.state().await.as_deref(), Some("completed"));
    assert_eq!(evidence.measurements.len(), t.recipe.seeds.len());
    assert_eq!(evidence.chain_epoch, 1);
    assert!(evidence.contamination_hits.is_empty());
    for (pair, seed) in evidence.measurements.iter().zip(&t.recipe.seeds) {
        assert_eq!(pair.baseline.seed, *seed);
        assert_eq!(pair.candidate.seed, *seed);
        assert_eq!(pair.baseline.flops_used, 1_000_000);
        assert!((pair.baseline.metrics.holdout_nll - 2.0).abs() < 1e-9);
        assert!((pair.candidate.metrics.holdout_nll - 1.0).abs() < 1e-9);
        assert!(artifacts.contains_key(&pair.candidate.log_digest));
    }
    // The same evaluate() the research store applies accepts this evidence.
    let summary = evidence.evaluate(&t.recipe, &artifacts).unwrap();
    assert!(summary.passed, "{summary:?}");
    assert!((summary.primary_mean - 1.0).abs() < 1e-9);
    let observed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM proof_execution_observation WHERE observation->>'event' = 'observed'",
    )
    .fetch_one(t.fixture.database.pool())
    .await
    .unwrap();
    assert_eq!(observed, 6);
    let artifacts_retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM proof_execution_observation WHERE observation->>'event' = 'artifact'",
    )
    .fetch_one(t.fixture.database.pool())
    .await
    .unwrap();
    assert_eq!(artifacts_retained, 6);
    t.absent().await;
    assert_eq!(
        docker_get(
            "/containers/json?all=true&filters=%7B%22name%22%3A%5B%22base-proof-measure-%22%5D%7D"
        )
        .await
        .1
        .as_array()
        .map(Vec::len),
        Some(0)
    );
    // Evidence admission stays with the research store; collect only observes.
    t.no_evidence().await;
    t.fixture.close().await;

    // Same scripts, default NoObserver: preflight refuses before any run.
    let t = Test::new(OBSERVED_BASELINE, OBSERVED_CANDIDATE, true).await;
    t.source(OBSERVED_BASELINE, OBSERVED_CANDIDATE).await;
    assert!(t
        .executor
        .collect(&t.lease, &t.grant, &t.recipe)
        .await
        .is_err());
    assert_eq!(t.state().await.as_deref(), Some("failed"));
    t.failure("unobserved_measurements").await;
    let runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM proof_execution_observation WHERE observation->>'event' = 'run'",
    )
    .fetch_one(t.fixture.database.pool())
    .await
    .unwrap();
    assert_eq!(runs, 0);
    t.absent().await;
    t.no_evidence().await;
    t.fixture.close().await;
}
