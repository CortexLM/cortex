#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use axum::{body::Body, http::Request, Router};
use bundle::{LeafV1, ScoreOrAbsence};
use chain::{ChainClient, FakeChain, FakeChainConfig};
use chain_live::FinalizedSnapshot;
use db::TestPool;
use gateway::{ChallengesBody, GatewayState, Registry, RegistryConfig, Stores};
use gateway_proof::{Config, Error, FinalizedSource, Receiver};
use http_body_util::BodyExt;
use parity_scale_codec::Encode;
use proof_publication::RoundPublication;
use serde_json::{json, Value};
use tower::ServiceExt;

pub const SECRET: [u8; 32] = [7; 32];
pub const GATEWAY_SECRET: [u8; 32] = [9; 32];
pub const EPOCH: u64 = 7;

pub fn chain() -> FakeChain {
    FakeChain::new(FakeChainConfig {
        hotkeys: vec![vec![1; 32], vec![2; 32], vec![3; 32]],
        current_block: 10_000,
        owner_hotkey: vec![1; 32],
        ..FakeChainConfig::default()
    })
}

#[derive(Default)]
pub struct Source {
    pub unavailable: AtomicBool,
}
impl FinalizedSource for Source {
    fn snapshot(&self, block: u64) -> Result<FinalizedSnapshot, Error> {
        if self.unavailable.load(Ordering::SeqCst) || block > 10_000 {
            return Err(Error::Unavailable);
        }
        let chain = chain();
        let hash = chain.block_hash(block).unwrap();
        Ok(FinalizedSnapshot {
            block,
            hash,
            chain_epoch: EPOCH,
            timestamp_ms: 123_456,
            metagraph: chain.metagraph_at(&hash).unwrap(),
        })
    }
}

pub fn config() -> Config {
    Config {
        netuid: 1,
        anchor_block: 0,
        proof_public_key: challenge_common::public_key_from_secret(&SECRET).unwrap(),
    }
}

pub fn challenges() -> ChallengesBody {
    ChallengesBody {
        challenges: [(b"bounty".to_vec(), 2000), (b"proof".to_vec(), 8000)]
            .into_iter()
            .map(|(id, emission_share_bps)| gateway::ChallengeEntry {
                id,
                emission_share_bps,
                public_key: config().proof_public_key,
                policy: gateway::ParticipantPolicy::AllMetagraphHotkeys,
            })
            .collect(),
    }
}

pub fn leaves(challenge: &[u8], score: u64) -> Vec<LeafV1> {
    let scores: BTreeMap<_, _> = [[1; 32], [2; 32], [3; 32]]
        .into_iter()
        .enumerate()
        .map(|(i, key)| {
            (
                key,
                ScoreOrAbsence::Score {
                    value: if i == 1 {
                        score
                    } else if i == 2 {
                        10
                    } else {
                        0
                    },
                },
            )
        })
        .collect();
    challenge_common::emit_signed_leaf_set(
        &SECRET,
        challenge,
        EPOCH,
        &scores.keys().copied().collect(),
        &scores,
    )
    .unwrap()
    .into_values()
    .collect()
}

pub fn document(round: u64, score: u64) -> RoundPublication {
    let block = (round + 1) * 360;
    let mut d = RoundPublication {
        round,
        chain_epoch: EPOCH,
        netuid: 1,
        block,
        block_hash: hex::encode(chain().block_hash(block).unwrap()),
        frozen_digest: "a".repeat(64),
        decision_digest: "b".repeat(64),
        leaves: leaves(b"proof", score).encode(),
        signature: String::new(),
    };
    d.sign(&SECRET).unwrap();
    d
}

pub fn wire(leaf: &LeafV1) -> Value {
    let score = match leaf.score_or_absence {
        ScoreOrAbsence::Score { value } => json!({"score": {"value":value}}),
        ScoreOrAbsence::NoScore { reason } => json!({"no_score": {"reason":reason as u8}}),
    };
    json!({"challenge_id": String::from_utf8(leaf.challenge_id.clone()).unwrap(),
        "epoch": leaf.epoch, "miner_hotkey": hex::encode(leaf.miner_hotkey),
        "challenge_sig": hex::encode(leaf.challenge_sig), "score_or_absence":score })
}

pub async fn request(app: &Router, path: &str, bytes: Option<Vec<u8>>) -> (u16, Vec<u8>) {
    let request = Request::builder()
        .method(if bytes.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header(
            "content-type",
            if path.starts_with("/v1/") {
                "application/json"
            } else {
                "application/octet-stream"
            },
        )
        .body(bytes.map_or_else(Body::empty, Body::from))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    (
        response.status().as_u16(),
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

pub struct Fixture {
    pub db: TestPool,
    pub url: String,
    pub receiver: Arc<Receiver>,
    pub source: Arc<Source>,
    pub app: Router,
    pub stores: Stores,
}

impl Fixture {
    pub async fn new() -> Option<Self> {
        std::env::var_os("DATABASE_URL")?;
        let db = db::test_pool()
            .await
            .expect("isolated migrated PostgreSQL schema");
        let url = schema_url(&db);
        let source = Arc::new(Source::default());
        let receiver = connect(&url, source.clone(), config());
        let stores = receiver.stores();
        let state = GatewayState::with_parts_seal(
            Registry::shared(RegistryConfig::default()),
            Arc::new(validator_sync::SyncChain::new(chain())),
            Arc::new(challenges()),
            stores.0.clone(),
            stores.1.clone(),
            [8; 32],
            1,
        )
        .unwrap();
        let app = receiver
            .router()
            .merge(gateway::weights_router(state.clone()))
            .merge(gateway::bundle_router(state.clone()))
            .merge(gateway::admin_seal_router(state));
        Some(Self {
            db,
            url,
            receiver,
            source,
            app,
            stores,
        })
    }

    pub fn seal(&self, block: u64) -> Result<bundle::EpochBundleV1, gateway::SealError> {
        gateway::seal_epoch(
            &chain(),
            &challenges(),
            self.stores.0.as_ref(),
            self.stores.1.as_ref(),
            &gateway::SealParams {
                epoch: EPOCH,
                netuid: 1,
                block_b: block,
                gateway_secret: GATEWAY_SECRET,
                measurements_digest: [8; 32],
            },
        )
    }

    pub async fn bounty(&self) {
        for leaf in leaves(b"bounty", 0) {
            assert_eq!(
                request(
                    &self.app,
                    "/v1/weights/raw",
                    Some(serde_json::to_vec(&wire(&leaf)).unwrap())
                )
                .await
                .0,
                202
            );
        }
    }

    pub async fn close(self) {
        self.db.drop_schema().await.unwrap();
    }
}

pub fn connect(url: &str, source: Arc<dyn FinalizedSource>, config: Config) -> Arc<Receiver> {
    Receiver::connect(
        url,
        source,
        Arc::new(validator_sync::SyncChain::new(chain())),
        config,
        challenges(),
    )
    .unwrap()
}

pub fn schema_url(db: &TestPool) -> String {
    let url = db::app_role_database_url(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    format!("{url}?options=-c%20search_path%3D{}", db.schema())
}
