//! Bounty emission is only as real as the backend feed behind it.
//!
//! Two properties are load-bearing and neither is visible from a unit test of
//! the scorer:
//!
//! 1. A host that cannot read `{BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/*`
//!    signs **no leaf at all** — not even an all-`NoScore` set, which would be
//!    a verdict ("nobody found anything") this host has no standing to reach.
//! 2. A host that *can* read it turns published rows into scored leaves for
//!    metagraph hotkeys, which is how a validator ever sees bounty weight.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use bounty_challenge::{BountyEmitter, EmitError, GatewayClient, GatewayClientConfig};
use chain::{
    AxonInfo, ChainClient, ChainError, FakeChain, FakeChainConfig, Metagraph, WeightsTlockPayload,
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
const MALICIOUS: [u8; 32] = [0xB2; 32];
const SILENT: [u8; 32] = [0xC3; 32];

fn fake_chain() -> LockedFake {
    LockedFake(Mutex::new(FakeChain::new(FakeChainConfig {
        netuid: NETUID,
        hotkeys: vec![CHAMPION.to_vec(), MALICIOUS.to_vec(), SILENT.to_vec()],
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

/// Published rows for the mock backend public feed. `CHAMPION` filed three
/// justified, priced findings; `MALICIOUS` fabricated three.
fn leaderboard_json() -> serde_json::Value {
    serde_json::json!({
        "items": [
            { "hotkey": hex::encode(CHAMPION), "valid_count": 3 },
            { "hotkey": hex::encode(MALICIOUS), "valid_count": 0 },
        ]
    })
}

fn reports_json() -> serde_json::Value {
    let mut items = Vec::new();
    for (i, problem) in ["seal 500 on empty bundle", "proxy 502", "health flap"]
        .iter()
        .enumerate()
    {
        items.push(serde_json::json!({
            "id": format!("valid-{i}"),
            "hotkey": hex::encode(CHAMPION),
            "status": "valid",
            "severity": "major",
            "problem_found": problem,
            "adjudicator": "bounty-adjudicator@cortex",
            "justification": "reproduced on master",
            "adjudicated_at": "2026-08-30T00:00:00Z",
            "created_at": "2026-08-29T00:00:00Z",
        }));
    }
    for i in 0..3 {
        items.push(serde_json::json!({
            "id": format!("bad-{i}"),
            "hotkey": hex::encode(MALICIOUS),
            "status": "invalid_malicious",
            "problem_found": "invented crash",
            "adjudicator": "bounty-adjudicator@cortex",
            "justification": "does not reproduce anywhere",
            "adjudicated_at": "2026-08-30T00:00:00Z",
            "created_at": "2026-08-29T00:00:00Z",
        }));
    }
    serde_json::json!({ "items": items })
}

async fn spawn_backend() -> String {
    let app = Router::new()
        .route(
            "/v1/bounty/public/leaderboard",
            get(|| async { Json(leaderboard_json()) }),
        )
        .route(
            "/v1/bounty/public/reports",
            get(|| async { Json(reports_json()) }),
        );
    serve(app).await
}

/// A backend that answers, but with 500s — the shape of an outage.
async fn spawn_broken_backend() -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/v1/bounty/public/{tail}",
            get(|State(h): State<Arc<AtomicUsize>>| async move {
                h.fetch_add(1, Ordering::Relaxed);
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            }),
        )
        .with_state(Arc::clone(&hits));
    (serve(app).await, hits)
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

fn emitter(backend: Option<String>, gateway_url: &str) -> BountyEmitter<LockedFake> {
    let gateway = Arc::new(
        GatewayClient::new(GatewayClientConfig {
            base_url: gateway_url.to_owned(),
            ..GatewayClientConfig::default()
        })
        .expect("gateway client"),
    );
    BountyEmitter::new(fake_chain(), gateway, [7u8; 32], NETUID, backend)
}

fn leaf_for(accepted: &Accepted, hotkey: [u8; 32]) -> serde_json::Value {
    let hex_key = hex::encode(hotkey);
    accepted
        .lock()
        .expect("lock")
        .iter()
        .find(|v| v["miner_hotkey"] == serde_json::Value::String(hex_key.clone()))
        .cloned()
        .unwrap_or_else(|| panic!("no leaf for {hex_key}"))
}

/// The happy path a validator depends on: published backend rows become
/// signed leaves for metagraph hotkeys, with the fabricator burned and the
/// hotkey that filed nothing left explicitly unscored.
#[tokio::test]
async fn mock_backend_rows_become_scored_leaves_for_metagraph_hotkeys() {
    let backend = spawn_backend().await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(backend), &gateway);

    let tick = em.tick().await.expect("tick");
    assert_eq!(tick.participants, 3, "every hotkey in E needs a leaf");
    assert_eq!(tick.paid, 1, "one champion was paid");
    assert_eq!(tick.epoch, chain::fake_defaults::SUBNET_EPOCH_INDEX);
    assert_eq!(tick.pin_block, chain::fake_defaults::LAST_EPOCH_BLOCK);
    assert_eq!(em.emitted_epoch(), tick.epoch);
    assert_eq!(accepted.lock().expect("lock").len(), 3);

    let champion = leaf_for(&accepted, CHAMPION);
    assert_eq!(champion["challenge_id"], "bounty");
    assert_eq!(champion["epoch"], tick.epoch);
    assert!(
        champion["score_or_absence"]["score"]["value"]
            .as_u64()
            .unwrap_or_default()
            > 0,
        "three justified, priced findings must pay: {champion}"
    );
    // reason 2 = InvalidResponse: a net-negative miner burns toward uid 0.
    assert_eq!(
        leaf_for(&accepted, MALICIOUS)["score_or_absence"]["no_score"]["reason"],
        2
    );
    // reason 0 = NotAttempted: a hotkey with no published rows is explicit,
    // never a silent omission that would break exact-E.
    assert_eq!(
        leaf_for(&accepted, SILENT)["score_or_absence"]["no_score"]["reason"],
        0
    );
}

/// No feed configured is a refusal. A skip here would leave the challenge
/// looking healthy while paying nobody, and (worse) let a later local scorer
/// mint weight no validator can reproduce.
#[tokio::test]
async fn an_unset_backend_url_emits_nothing() {
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(None, &gateway);

    let err = em.tick().await.expect_err("unset feed");
    assert!(
        matches!(err, EmitError::Backend(_)),
        "unset must fail on the feed, before any chain or gateway work: {err}"
    );
    assert!(
        err.to_string().contains("BOUNTY_BACKEND_PUBLIC_URL"),
        "{err}"
    );
    assert_eq!(em.emitted_epoch(), 0);
    assert!(
        accepted.lock().expect("lock").is_empty(),
        "an unreadable feed must not produce a leaf set"
    );
}

/// A blank value is the shape of an unset compose variable
/// (`BOUNTY_BACKEND_PUBLIC_URL: "${BOUNTY_BACKEND_PUBLIC_URL:-}"`).
#[tokio::test]
async fn a_blank_backend_url_emits_nothing() {
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some("   ".to_owned()), &gateway);

    assert!(matches!(
        em.tick().await.expect_err("blank feed"),
        EmitError::Backend(_)
    ));
    assert!(accepted.lock().expect("lock").is_empty());
}

/// A backend outage is not "everyone scored zero".
#[tokio::test]
async fn a_failing_backend_emits_nothing() {
    let (backend, hits) = spawn_broken_backend().await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(backend), &gateway);

    let err = em.tick().await.expect_err("backend 500");
    assert!(matches!(err, EmitError::Backend(_)), "{err}");
    assert!(
        hits.load(Ordering::Relaxed) >= 1,
        "the feed was really asked"
    );
    assert!(
        accepted.lock().expect("lock").is_empty(),
        "a 5xx feed must not become an all-NoScore verdict"
    );
}
