//! Proof emission is only as real as the in-process store behind it.
//!
//! Three properties are load-bearing and none is visible from a unit test of
//! `emission_scores` alone:
//!
//! 1. A host that holds a positive lattice turns it into scored leaves for
//!    metagraph hotkeys, which is how a validator ever sees proof weight.
//! 2. A host that has no positive lattice this tick pays **nobody** — every
//!    leaf is `NoScore(ChallengeInternal)`, so the challenge share burns to
//!    uid 0 — while still covering `E`, because a paid challenge with no
//!    leaves fails D24 and takes every other challenge's seal down with it.
//!    That cover is `ChallengeInternal`, not `NotAttempted`.
//! 3. An empty tick inside an already-scored epoch does not take back the
//!    scores the store really did publish.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use chain::{
    AxonInfo, ChainClient, ChainError, FakeChain, FakeChainConfig, Metagraph, WeightsTlockPayload,
};
use proof_challenge::{EmitOutcome, GatewayClient, GatewayClientConfig, MemoryStore, ProofEmitter};
use proof_score::MinerTopicRun;
use proof_task::{
    default_adamw, holdout_commitment, synthetic_holdout, PayoutMode, TopicDocument, TopicStatus,
    FLOPS_BUDGET_MAX, METRIC_TOKENS_PER_SEC,
};

/// `FakeChain` keeps its call log in a `RefCell`; the emitter needs `Sync`.
struct LockedFake(Mutex<FakeChain>);

macro_rules! delegate {
    (fn $name:ident(&self) -> $ret:ty) => {
        fn $name(&self) -> $ret {
            self.0.lock().expect("lock").$name()
        }
    };
    (fn $name:ident(&self, $($arg:ident : $t:ty),*) -> $ret:ty) => {
        fn $name(&self, $($arg: $t),*) -> $ret {
            self.0.lock().expect("lock").$name($($arg),*)
        }
    };
}

impl ChainClient for LockedFake {
    delegate!(fn current_block(&self) -> Result<u64, ChainError>);
    delegate!(fn block_hash(&self, n: u64) -> Result<[u8; 32], ChainError>);
    delegate!(fn metagraph_at(&self, block_hash: &[u8; 32]) -> Result<Metagraph, ChainError>);
    delegate!(fn subnet_owner_hotkey(&self, netuid: u16) -> Result<Vec<u8>, ChainError>);
    delegate!(fn axon(&self, netuid: u16, hotkey: &[u8]) -> Result<Option<AxonInfo>, ChainError>);
    delegate!(fn axons(&self, netuid: u16) -> Result<Vec<(Vec<u8>, AxonInfo)>, ChainError>);
    delegate!(fn commit_reveal_enabled(&self, netuid: u16) -> Result<bool, ChainError>);
    delegate!(fn commit_reveal_version(&self, netuid: u16) -> Result<u16, ChainError>);
    delegate!(fn tempo(&self, netuid: u16) -> Result<u64, ChainError>);
    delegate!(fn reveal_period_epochs(&self, netuid: u16) -> Result<u64, ChainError>);
    delegate!(fn block_time(&self) -> Result<u64, ChainError>);
    delegate!(fn last_epoch_block(&self, netuid: u16) -> Result<u64, ChainError>);
    delegate!(fn pending_epoch_at(&self, netuid: u16) -> Result<u64, ChainError>);
    delegate!(fn subnet_epoch_index(&self, netuid: u16) -> Result<u64, ChainError>);
    delegate!(fn blocks_since_last_step(&self, netuid: u16) -> Result<u64, ChainError>);
    delegate!(fn submit_timelocked_weights(
        &self,
        mecid: u8,
        payload: WeightsTlockPayload,
        reveal_round: u64
    ) -> Result<(), ChainError>);
    delegate!(fn set_weights(
        &self,
        netuid: u16,
        uids: Vec<u16>,
        values: Vec<u16>,
        version_key: u64
    ) -> Result<(), ChainError>);
}

const NETUID: u16 = 541;

/// Metagraph hotkeys the fake serves, in UID order.
const CHAMPION: [u8; 32] = [0xA1; 32];
const OTHER: [u8; 32] = [0xB2; 32];
const SILENT: [u8; 32] = [0xC3; 32];

fn fake_chain() -> LockedFake {
    LockedFake(Mutex::new(FakeChain::new(FakeChainConfig {
        netuid: NETUID,
        hotkeys: vec![CHAMPION.to_vec(), OTHER.to_vec(), SILENT.to_vec()],
        ..FakeChainConfig::default()
    })))
}

/// Leaves the mock gateway accepted, in arrival order.
type Accepted = Arc<Mutex<Vec<serde_json::Value>>>;

async fn spawn_gateway() -> (String, Accepted) {
    let accepted: Accepted = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/weights/raw",
            post(
                |State(seen): State<Accepted>, Json(body): Json<serde_json::Value>| async move {
                    seen.lock().expect("lock").push(body);
                    axum::http::StatusCode::ACCEPTED
                },
            ),
        )
        .with_state(Arc::clone(&accepted));
    (serve(app).await, accepted)
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn emitter(store: MemoryStore, gateway_url: &str) -> ProofEmitter<LockedFake> {
    let gateway = Arc::new(
        GatewayClient::new(GatewayClientConfig {
            base_url: gateway_url.to_owned(),
            ..GatewayClientConfig::default()
        })
        .expect("gateway client"),
    );
    ProofEmitter::new(fake_chain(), gateway, [7u8; 32], NETUID, store)
}

fn leaf_for(accepted: &Accepted, hotkey: [u8; 32]) -> serde_json::Value {
    let hex_key = hex::encode(hotkey);
    accepted
        .lock()
        .expect("lock")
        .iter()
        .rfind(|v| v["miner_hotkey"] == serde_json::Value::String(hex_key.clone()))
        .cloned()
        .unwrap_or_else(|| panic!("no leaf for {hex_key}"))
}

fn accepted_count(accepted: &Accepted) -> usize {
    accepted.lock().expect("lock").len()
}

/// Assert every metagraph hotkey got a leaf carrying `no_score.reason`.
/// Reason 6 = `ChallengeInternal` (`BUNDLE_SPEC` §3.3.1).
fn assert_burn_covers_e(accepted: &Accepted) {
    for hotkey in [CHAMPION, OTHER, SILENT] {
        let leaf = leaf_for(accepted, hotkey);
        assert_eq!(
            leaf["score_or_absence"]["no_score"]["reason"], 6,
            "a host that scored nobody must pay nobody: {leaf}"
        );
        assert!(
            leaf["score_or_absence"].get("score").is_none(),
            "a burn leaf must carry no score: {leaf}"
        );
    }
}

fn sealed_baseline() -> proof_task::Baseline {
    let mut b = default_adamw(FLOPS_BUDGET_MAX);
    b.script_sha256 = "11".repeat(32);
    b.metrics_commitment = "22".repeat(32);
    b
}

fn open_wta_topic(id: &str) -> TopicDocument {
    TopicDocument {
        id: id.into(),
        statement: "Beat sealed AdamW.".into(),
        payout_mode: PayoutMode::Wta,
        baseline: sealed_baseline(),
        holdout_commitment: holdout_commitment(&synthetic_holdout(24, 1)),
        status: TopicStatus::Open,
        metric: proof_task::MetricSpec {
            family: proof_task::MetricFamily::Throughput,
            primary: METRIC_TOKENS_PER_SEC.into(),
            direction: proof_task::MetricDirection::Max,
            epsilon_rel: 0.05,
            quality_floor_nll: 0.02,
            wall_budget_s: 14_400,
            ..proof_task::MetricSpec::default()
        },
        ..TopicDocument::default()
    }
}

fn pass(primary: f64, digest: &str) -> MinerTopicRun {
    MinerTopicRun {
        pass: true,
        primary: Some(primary),
        artifact_digest: digest.into(),
        near_duplicate: false,
    }
}

fn seed_scored_store() -> MemoryStore {
    let store = MemoryStore::new();
    let topic = open_wta_topic("dt-no-ib-v0");
    store.put_topic(topic).expect("topic");
    store
        .record_topic_run(&hex::encode(CHAMPION), "dt-no-ib-v0", pass(200.0, "d1"))
        .expect("run");
    store
}

/// The happy path a validator depends on: a stored pass becomes signed leaves
/// for metagraph hotkeys, with the silent UIDs left explicitly unscored.
#[tokio::test]
async fn a_stored_pass_becomes_scored_leaves_for_metagraph_hotkeys() {
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(seed_scored_store(), &gateway);

    let epoch = match em.tick().await.expect("tick") {
        EmitOutcome::Scored {
            epoch,
            pin_block,
            participants,
            paid,
        } => {
            assert_eq!(participants, 3, "every hotkey in E needs a leaf");
            assert_eq!(paid, 1, "one champion was paid");
            assert_eq!(pin_block, chain::fake_defaults::LAST_EPOCH_BLOCK);
            epoch
        }
        other => panic!("a stored pass must score: {other:?}"),
    };
    assert_eq!(epoch, chain::fake_defaults::SUBNET_EPOCH_INDEX);
    assert_eq!(em.scored_epoch(), epoch);
    assert_eq!(accepted_count(&accepted), 3);

    let champion = leaf_for(&accepted, CHAMPION);
    assert_eq!(champion["challenge_id"], "proof");
    assert_eq!(champion["epoch"], epoch);
    assert!(
        champion["score_or_absence"]["score"]["value"]
            .as_u64()
            .unwrap_or_default()
            > 0,
        "a stored WTA pass must pay: {champion}"
    );
    // reason 0 = NotAttempted: a hotkey with no stored run is explicit,
    // never a silent omission that would break exact-E.
    assert_eq!(
        leaf_for(&accepted, SILENT)["score_or_absence"]["no_score"]["reason"],
        0
    );
    assert_eq!(
        leaf_for(&accepted, OTHER)["score_or_absence"]["no_score"]["reason"],
        0
    );
}

/// An empty store pays nobody. It still has to cover `E`: proof holds a paid
/// trust-root row, and a paid challenge with no leaves makes
/// `POST /v1/admin/seal` answer 409 for the whole bundle.
#[tokio::test]
async fn an_empty_store_burns_without_paying_anyone() {
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(MemoryStore::new(), &gateway);

    match em.tick().await.expect("burn covers E") {
        EmitOutcome::Burned {
            epoch,
            participants,
            reason,
        } => {
            assert_eq!(epoch, chain::fake_defaults::SUBNET_EPOCH_INDEX);
            assert_eq!(participants, 3);
            assert!(reason.contains("no positive scores"), "{reason}");
        }
        other => panic!("empty store must burn, not score: {other:?}"),
    }
    assert_eq!(accepted_count(&accepted), 3);
    assert_burn_covers_e(&accepted);
    assert_eq!(
        em.scored_epoch(),
        0,
        "a burn is not a score and must not mark the epoch as scored"
    );
}

/// Open topics with no winner must not emit `NotAttempted` (reason 0). That
/// would look like miners skipped work. The live emitter treats "nobody
/// scored" as a host-side cover — `ChallengeInternal` — the same way bounty
/// covers `E` when the feed is down.
#[tokio::test]
async fn open_topics_with_no_winner_cover_e_with_challenge_internal() {
    let store = MemoryStore::new();
    store
        .put_topic(open_wta_topic("dt-no-ib-v0"))
        .expect("topic");
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(store, &gateway);

    assert!(matches!(
        em.tick().await.expect("burn covers E"),
        EmitOutcome::Burned { .. }
    ));
    assert_burn_covers_e(&accepted);
    assert_eq!(em.scored_epoch(), 0);
}

/// A later empty tick must not take a score back, or a store hiccup would
/// decide the epoch.
#[tokio::test]
async fn an_empty_tick_after_a_scored_epoch_holds_instead_of_burning_it() {
    let store = seed_scored_store();
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(store.clone(), &gateway);

    assert!(matches!(
        em.tick().await.expect("scored"),
        EmitOutcome::Scored { paid: 1, .. }
    ));
    let after_scored = accepted_count(&accepted);

    store
        .record_topic_run(
            &hex::encode(CHAMPION),
            "dt-no-ib-v0",
            MinerTopicRun {
                pass: false,
                primary: None,
                artifact_digest: String::new(),
                near_duplicate: false,
            },
        )
        .expect("clear");
    match em.tick().await.expect("hold") {
        EmitOutcome::Held { epoch, .. } => {
            assert_eq!(epoch, chain::fake_defaults::SUBNET_EPOCH_INDEX);
        }
        other => panic!("an empty tick must not overwrite a scored epoch: {other:?}"),
    }
    assert_eq!(
        accepted_count(&accepted),
        after_scored,
        "holding must post nothing at all"
    );
    assert!(
        leaf_for(&accepted, CHAMPION)["score_or_absence"]["score"]["value"]
            .as_u64()
            .unwrap_or_default()
            > 0,
        "the champion's score must still stand"
    );
}

/// A burn is superseded once the store actually holds a pass.
#[tokio::test]
async fn a_later_pass_supersedes_the_burn_for_the_same_epoch() {
    let store = MemoryStore::new();
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(store.clone(), &gateway);

    assert!(matches!(
        em.tick().await.expect("burn covers E"),
        EmitOutcome::Burned { .. }
    ));
    assert_burn_covers_e(&accepted);

    store
        .put_topic(open_wta_topic("dt-no-ib-v0"))
        .expect("topic");
    store
        .record_topic_run(&hex::encode(CHAMPION), "dt-no-ib-v0", pass(200.0, "d1"))
        .expect("run");
    assert!(matches!(
        em.tick().await.expect("recovered"),
        EmitOutcome::Scored { paid: 1, .. }
    ));
    assert!(
        leaf_for(&accepted, CHAMPION)["score_or_absence"]["score"]["value"]
            .as_u64()
            .unwrap_or_default()
            > 0,
        "the recovered tick must supersede the burn with the stored score"
    );
}
