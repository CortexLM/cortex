#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Opt-in real Docker/kernel/process test. Chain, science and inference are
//! explicitly synthetic; all HTTP endpoints are ephemeral literal loopback.
#[path = "headless_delivery/fixtures.rs"]
mod fixtures;
#[path = "../../gateway-proof/tests/common/mod.rs"]
mod ingress;
#[path = "../../proof-rounds/tests/common/mod.rs"]
mod rounds;

use bundle::{LocalTrustRoot, ScoreOrAbsence};
use fixtures::*;
use gateway::{ChallengesBody, GatewayState, Registry, RegistryConfig, Stores};
use parity_scale_codec::Encode;
use proof_atlas_worker::*;
use proof_rounds::{DecidedRound, RoundError};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
use tokio::sync::{watch, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable DATABASE_URL, Docker, pinned local kernel and installed Atlas runtime; no provider calls"]
async fn real_headless_kernel_decision_to_strict_gateway_seal_and_served_vector() {
    let f = rounds::pg::Fixture::new()
        .await
        .expect("explicit disposable DATABASE_URL");
    let (research, summary) = rounds::completed(&f).await;
    let source = Arc::new(Source(rounds::source(&f).await));
    let root = tempfile::Builder::new()
        .prefix("atlas-delivery-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let model = Arc::new(Model::default());
    let model_server = Server::start(model.router()).await;
    let agent = Agent::new(root.path(), &model_server.url);
    let mut config = rounds::config();
    config.runtime_digest = agent.binding().unwrap();
    let store = AtlasStore::new(f.database.app_pool().await.unwrap(), research, config);
    store.ready().await.unwrap();
    let gateway = Gateway::new(&f.database, source.clone());
    let gateway_server = Server::start(gateway.app.clone()).await;
    let publisher = Arc::new(LostAcknowledgement {
        client: proof_publication::Client::new(&gateway_server.url, rounds::miner()).unwrap(),
        documents: Mutex::new(vec![]),
        lose_once: AtomicBool::new(true),
    });
    let worker = AtlasWorker::new(
        store.clone(),
        source,
        agent.clone(),
        publisher.clone(),
        rounds::pg::SEED,
        AtlasConfig::default(),
    )
    .unwrap();
    let decided = gateway
        .deliver(
            &worker,
            &store,
            &publisher,
            &agent,
            &model,
            f.database.pool(),
        )
        .await;
    assert_eq!(decided.decision.awards.len(), 1);
    assert_eq!(decided.decision.awards[0].units, 500_000);
    assert_eq!(
        decided.decision.awards[0].evidence_digests,
        [summary.evidence_digest]
    );
    assert!(agent.failures.lock().await.is_empty());
    let jobs = agent.jobs.lock().await;
    assert_eq!(jobs.len(), 1, "publication retry must not rerun the model");
    let job = &jobs[0];
    let budget = checkpoint(root.path(), job);
    assert_eq!(budget["state"]["deadline"], job.run.deadline_ms);
    assert_eq!(budget["state"]["calls"], 2);
    assert_eq!(budget["state"]["tokens"], 40960);
    assert_eq!(budget["state"]["microUsd"], 0);
    assert_eq!(budget["state"]["revoked"], true);
    assert_eq!(model.requests.lock().await.len(), 2);
    let phase: String = sqlx::query_scalar("SELECT phase FROM proof_atlas_runtime WHERE round=0")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(phase, "finished");
    let sealed = gateway.seal(&decided).await;
    assert_eq!(
        hex::encode(sealed.body.block_hash),
        job.frozen.snapshot.finalized_hash
    );
    gateway.verify(&sealed).await;
    drop(jobs);
    drop(gateway_server);
    drop(model_server);
    f.close().await;
}

struct Gateway {
    receiver: Arc<gateway_proof::Receiver>,
    stores: Stores,
    challenges: Arc<ChallengesBody>,
    app: axum::Router,
}

impl Gateway {
    fn new(database: &db::TestPool, source: Arc<Source>) -> Self {
        let challenges = Arc::new(ingress::challenges());
        let chain = Arc::new(validator_sync::SyncChain::new(rounds::chain()));
        let url = ingress::schema_url(database);
        let receiver = gateway_proof::Receiver::connect(
            &url,
            source,
            chain.clone(),
            ingress::config(),
            (*challenges).clone(),
        )
        .unwrap();
        let stores = receiver.stores();
        let state = GatewayState::with_parts_seal(
            Registry::shared(RegistryConfig::default()),
            chain,
            challenges.clone(),
            stores.0.clone(),
            stores.1.clone(),
            [8; 32],
            1,
        )
        .unwrap();
        let app = receiver
            .router()
            .merge(gateway::weights_router(state.clone()))
            .merge(gateway::bundle_router(state));
        Self {
            receiver,
            stores,
            challenges,
            app,
        }
    }

    async fn deliver(
        &self,
        worker: &AtlasWorker,
        store: &AtlasStore,
        publisher: &LostAcknowledgement,
        agent: &Agent,
        model: &Model,
        pool: &sqlx::PgPool,
    ) -> DecidedRound {
        let (_keep, shutdown) = watch::channel(false);
        let first = tokio::time::timeout(Duration::from_mins(2), worker.tick(shutdown.clone()))
            .await
            .unwrap();
        let outputs: Vec<_> = model
            .requests
            .lock()
            .await
            .iter()
            .flat_map(|request| request["input"].as_array().into_iter().flatten())
            .filter(|item| item["type"] == "function_call_output")
            .map(|item| item["output"].clone())
            .collect();
        assert!(matches!(first, Err(AtlasError::Round(RoundError::Publication))),
        "expected lost acknowledgement after real delivery, got {first:?}; launcher failures: {:?}; kernel results: {outputs:?}",
        agent.failures.lock().await);
        let decided = store.rounds().decision(0).await.unwrap();
        let wire = decided.publication().unwrap().encode();
        assert_eq!(self.receiver.readback(0).unwrap(), wire);
        assert_eq!(
            ingress::request(&self.app, &format!("{}/0", proof_publication::ROUTE), None).await,
            (200, wire.clone())
        );
        let delivered: bool =
            sqlx::query_scalar("SELECT delivered FROM proof_atlas_publication WHERE round=0")
                .fetch_one(pool)
                .await
                .unwrap();
        assert!(!delivered);
        assert_eq!(
            worker.tick(shutdown).await.unwrap(),
            AtlasProgress::Published { round: 0 }
        );
        let attempts = publisher.documents.lock().await;
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].encode(), attempts[1].encode());
        drop(attempts);
        assert_eq!(self.receiver.readback(0).unwrap(), wire);
        decided
    }

    async fn seal(&self, decided: &DecidedRound) -> bundle::EpochBundleV1 {
        let mut corrupted = decided.publication().unwrap();
        corrupted.signature = "00".repeat(64);
        assert_eq!(
            ingress::request(
                &self.app,
                proof_publication::ROUTE,
                Some(corrupted.encode())
            )
            .await
            .0,
            401
        );
        let seal = gateway::SealParams {
            epoch: decided.frozen.snapshot.chain_epoch,
            netuid: 1,
            block_b: 360,
            gateway_secret: ingress::GATEWAY_SECRET,
            measurements_digest: [8; 32],
        };
        assert!(gateway::seal_epoch(
            &rounds::chain(),
            &self.challenges,
            self.stores.0.as_ref(),
            self.stores.1.as_ref(),
            &seal
        )
        .is_err());
        let unsealed: Value = serde_json::from_slice(
            &ingress::request(&self.app, "/v1/weights/latest", None)
                .await
                .1,
        )
        .unwrap();
        assert_eq!(unsealed["sealed"], false);
        self.bounty(decided).await;
        gateway::seal_epoch(
            &rounds::chain(),
            &self.challenges,
            self.stores.0.as_ref(),
            self.stores.1.as_ref(),
            &seal,
        )
        .unwrap()
    }

    async fn bounty(&self, decided: &DecidedRound) {
        let bounty_values: BTreeMap<_, _> = decided
            .frozen
            .expected()
            .into_iter()
            .map(|key| {
                (
                    key,
                    ScoreOrAbsence::Score {
                        value: u64::from(key == [3; 32]),
                    },
                )
            })
            .collect();
        let bounty = challenge_common::emit_signed_leaf_set(
            &rounds::pg::SEED,
            b"bounty",
            decided.frozen.snapshot.chain_epoch,
            &decided.frozen.expected(),
            &bounty_values,
        )
        .unwrap();
        for leaf in bounty.values() {
            assert_eq!(
                ingress::request(
                    &self.app,
                    "/v1/weights/raw",
                    Some(serde_json::to_vec(&ingress::wire(leaf)).unwrap())
                )
                .await
                .0,
                202
            );
        }
    }

    async fn verify(&self, sealed: &bundle::EpochBundleV1) {
        bundle::verify_bundle(
            sealed,
            &rounds::chain(),
            &LocalTrustRoot {
                challenges: (*self.challenges).clone(),
                measurements_digest: [8; 32],
            },
        )
        .unwrap();
        assert_eq!(sealed.body.block_b, 360);
        let independent = validator::independent_python_weights(&sealed.body).unwrap();
        assert_eq!(sealed.body.final_vector, independent.final_vector);
        assert_eq!(independent.floats.uids, [0, 1, 2]);
        for (actual, expected) in independent.floats.weights.iter().zip([0.4_f64, 0.4, 0.2]) {
            assert!((actual - expected).abs() < 1e-12);
        }
        let latest: Value = serde_json::from_slice(
            &ingress::request(&self.app, "/v1/weights/latest", None)
                .await
                .1,
        )
        .unwrap();
        assert_eq!(latest["sealed"], true);
        assert_eq!(latest["uids"], json!([0, 1, 2]));
        let weights: Vec<f64> = serde_json::from_value(latest["weights"].clone()).unwrap();
        assert_eq!(
            weights.iter().map(|w| w.to_bits()).collect::<Vec<_>>(),
            independent
                .floats
                .weights
                .iter()
                .map(|w| w.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            self.stores.1.get_by_epoch(sealed.body.epoch).unwrap(),
            sealed.encode_bytes()
        );
    }
}
