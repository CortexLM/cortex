//! Bounty emission is only as real as the backend feed behind it.
//!
//! Three properties are load-bearing and none is visible from a unit test of
//! the scorer:
//!
//! 1. A host that *can* read `{BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/*`
//!    turns published rows into scored leaves for metagraph hotkeys, which is
//!    how a validator ever sees bounty weight.
//! 2. A host that cannot read it pays **nobody** — every leaf is
//!    `NoScore(ChallengeInternal)`, so the challenge share burns to uid 0 —
//!    while still covering `E`, because a paid challenge with no leaves fails
//!    D24 and takes every other challenge's seal down with it.
//! 3. A feed outage inside an already-scored epoch does not take back the
//!    scores the backend really did publish.
//! 4. The leaderboard and the reports are separate GETs, so a snapshot is only
//!    signed once the feed holds still across both of them.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use bounty_challenge::{
    fetch_public_snapshot, BackendError, BountyEmitter, EmitOutcome, GatewayClient,
    GatewayClientConfig,
};
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

/// One justified, priced finding from `CHAMPION`.
fn champion_valid_row(i: usize, problem: &str) -> serde_json::Value {
    serde_json::json!({
        "id": format!("valid-{i}"),
        "hotkey": hex::encode(CHAMPION),
        "status": "valid",
        "severity": "major",
        "problem_found": problem,
        "adjudicator": "bounty-adjudicator@cortex",
        "justification": "reproduced on master",
        "adjudicated_at": "2026-08-30T00:00:00Z",
        "created_at": "2026-08-29T00:00:00Z",
    })
}

fn reports_json() -> serde_json::Value {
    let mut items = Vec::new();
    for (i, problem) in ["seal 500 on empty bundle", "proxy 502", "health flap"]
        .iter()
        .enumerate()
    {
        items.push(champion_valid_row(i, problem));
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

/// Request counter + health switch for [`spawn_flaky_backend`].
type Flaky = (Arc<AtomicUsize>, Arc<AtomicBool>);

/// A backend that answers, but with 500s — the shape of an outage. Flip
/// `healthy` to let the same base URL recover mid-epoch.
async fn spawn_flaky_backend() -> (String, Arc<AtomicUsize>, Arc<AtomicBool>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let healthy = Arc::new(AtomicBool::new(false));
    let state: Flaky = (Arc::clone(&hits), Arc::clone(&healthy));
    let app = Router::new()
        .route(
            "/v1/bounty/public/leaderboard",
            get(|State((h, ok)): State<Flaky>| async move {
                h.fetch_add(1, Ordering::Relaxed);
                json_or_500(&ok, leaderboard_json())
            }),
        )
        .route(
            "/v1/bounty/public/reports",
            get(|State((h, ok)): State<Flaky>| async move {
                h.fetch_add(1, Ordering::Relaxed);
                json_or_500(&ok, reports_json())
            }),
        )
        .with_state(state);
    (serve(app).await, hits, healthy)
}

fn json_or_500(healthy: &AtomicBool, body: serde_json::Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    if healthy.load(Ordering::Relaxed) {
        Json(body).into_response()
    } else {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

/// Request counter + the request index after which the feed stops moving.
type Shifting = (Arc<AtomicUsize>, usize);

/// A backend that publishes a new revision on every request until it has
/// served `freeze_after` of them, then holds still. Each revision credits
/// `CHAMPION` one more valid finding, and the leaderboard row moves with it —
/// so reading the two routes once apiece can only ever mix revisions.
async fn spawn_shifting_backend(freeze_after: usize) -> String {
    let state: Shifting = (Arc::new(AtomicUsize::new(0)), freeze_after);
    let app = Router::new()
        .route(
            "/v1/bounty/public/leaderboard",
            get(|State((served, freeze)): State<Shifting>| async move {
                let rev = next_revision(&served, freeze);
                Json(serde_json::json!({
                    "items": [{ "hotkey": hex::encode(CHAMPION), "valid_count": 3 + rev }]
                }))
            }),
        )
        .route(
            "/v1/bounty/public/reports",
            get(|State((served, freeze)): State<Shifting>| async move {
                let rev = next_revision(&served, freeze);
                let items: Vec<_> = (0..(3 + rev))
                    .map(|i| champion_valid_row(i, &format!("regression {i}")))
                    .collect();
                Json(serde_json::json!({ "items": items }))
            }),
        )
        .with_state(state);
    serve(app).await
}

fn next_revision(served: &AtomicUsize, freeze_after: usize) -> usize {
    served.fetch_add(1, Ordering::Relaxed).min(freeze_after)
}

/// Leaderboard always publishes three valids; reports always publishes one.
/// The mixed pair is identical on every request, so consecutive re-reads
/// agree — and must still be refused.
async fn spawn_stable_torn_backend() -> String {
    let app = Router::new()
        .route(
            "/v1/bounty/public/leaderboard",
            get(|| async {
                Json(serde_json::json!({
                    "items": [{ "hotkey": hex::encode(CHAMPION), "valid_count": 3 }]
                }))
            }),
        )
        .route(
            "/v1/bounty/public/reports",
            get(|| async { Json(serde_json::json!({ "items": [champion_valid_row(0, "one")] })) }),
        );
    serve(app).await
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
    for hotkey in [CHAMPION, MALICIOUS, SILENT] {
        let leaf = leaf_for(accepted, hotkey);
        assert_eq!(
            leaf["score_or_absence"]["no_score"]["reason"], 6,
            "a host that read nothing must pay nobody: {leaf}"
        );
        assert!(
            leaf["score_or_absence"].get("score").is_none(),
            "a burn leaf must carry no score: {leaf}"
        );
    }
}

/// The happy path a validator depends on: published backend rows become
/// signed leaves for metagraph hotkeys, with the fabricator burned and the
/// hotkey that filed nothing left explicitly unscored.
#[tokio::test]
async fn mock_backend_rows_become_scored_leaves_for_metagraph_hotkeys() {
    let backend = spawn_backend().await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(backend), &gateway);

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
        other => panic!("a readable feed must score: {other:?}"),
    };
    assert_eq!(epoch, chain::fake_defaults::SUBNET_EPOCH_INDEX);
    assert_eq!(em.scored_epoch(), epoch);
    assert_eq!(accepted_count(&accepted), 3);

    let champion = leaf_for(&accepted, CHAMPION);
    assert_eq!(champion["challenge_id"], "bounty");
    assert_eq!(champion["epoch"], epoch);
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

/// No feed configured pays nobody. It still has to cover `E`: bounty holds a
/// paid trust-root row, and a paid challenge with no leaves makes
/// `POST /v1/admin/seal` answer 409 for the whole bundle — an unconfigured
/// bounty host would take relearn's weights down with it.
#[tokio::test]
async fn an_unset_backend_url_burns_without_paying_anyone() {
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(None, &gateway);

    match em.tick().await.expect("burn covers E") {
        EmitOutcome::Burned {
            epoch,
            participants,
            reason,
        } => {
            assert_eq!(epoch, chain::fake_defaults::SUBNET_EPOCH_INDEX);
            assert_eq!(participants, 3);
            assert!(reason.contains("BOUNTY_BACKEND_PUBLIC_URL"), "{reason}");
        }
        other => panic!("unset feed must burn, not score: {other:?}"),
    }
    assert_eq!(accepted_count(&accepted), 3);
    assert_burn_covers_e(&accepted);
    assert_eq!(
        em.scored_epoch(),
        0,
        "a burn is not a score and must not mark the epoch as scored"
    );
}

/// A blank value is the shape of an unset compose variable
/// (`BOUNTY_BACKEND_PUBLIC_URL: "${BOUNTY_BACKEND_PUBLIC_URL:-}"`).
#[tokio::test]
async fn a_blank_backend_url_burns_without_paying_anyone() {
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some("   ".to_owned()), &gateway);

    assert!(matches!(
        em.tick().await.expect("burn covers E"),
        EmitOutcome::Burned { .. }
    ));
    assert_burn_covers_e(&accepted);
}

/// A backend outage is not "the champion earned this". Nobody is paid until
/// the feed answers, and then the gateway supersedes the burn with the real
/// scores for the same epoch.
#[tokio::test]
async fn a_failing_backend_burns_until_it_recovers() {
    let (backend, hits, healthy) = spawn_flaky_backend().await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(backend), &gateway);

    assert!(matches!(
        em.tick().await.expect("burn covers E"),
        EmitOutcome::Burned { .. }
    ));
    assert!(
        hits.load(Ordering::Relaxed) >= 1,
        "the feed was really asked"
    );
    assert_burn_covers_e(&accepted);

    healthy.store(true, Ordering::Relaxed);
    assert!(matches!(
        em.tick().await.expect("recovered"),
        EmitOutcome::Scored { paid: 1, .. }
    ));
    assert!(
        leaf_for(&accepted, CHAMPION)["score_or_absence"]["score"]["value"]
            .as_u64()
            .unwrap_or_default()
            > 0,
        "the recovered tick must supersede the burn with the published score"
    );
}

/// `/leaderboard` and `/reports` are separate GETs, so reading each once would
/// let a publish landing between them be signed as a single snapshot. That is
/// not cosmetic: every tally comes from `/reports`, so a stale half can
/// under-count a miner's valid rows or drop it to `NotAttempted` — a verdict
/// the backend never published — while `/leaderboard` decides the champion
/// walk order. A feed that never holds still must therefore pay nobody rather
/// than sign a revision it cannot pin.
#[tokio::test]
async fn a_feed_that_never_holds_still_is_never_signed_as_one_snapshot() {
    let backend = spawn_shifting_backend(usize::MAX).await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(backend.clone()), &gateway);

    let err = fetch_public_snapshot(Some(&backend))
        .await
        .expect_err("a moving feed cannot be pinned to one revision");
    assert!(matches!(err, BackendError::Inconsistent), "{err}");

    match em.tick().await.expect("burn covers E") {
        EmitOutcome::Burned { reason, .. } => {
            assert!(reason.contains("changed under every read"), "{reason}");
        }
        other => panic!("a torn feed must pay nobody: {other:?}"),
    }
    assert_burn_covers_e(&accepted);
    assert_eq!(
        em.scored_epoch(),
        0,
        "a snapshot that could not be pinned is not a score"
    );
}

/// A feed that always serves leaderboard revision A beside reports revision B
/// is stable under consecutive re-reads. Signing it would pay from a state the
/// backend never published atomically.
#[tokio::test]
async fn a_stable_torn_pair_is_never_signed_as_one_snapshot() {
    let backend = spawn_stable_torn_backend().await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(backend.clone()), &gateway);

    let err = fetch_public_snapshot(Some(&backend))
        .await
        .expect_err("leaderboard A + reports B is not one snapshot");
    assert!(matches!(err, BackendError::Mismatched), "{err}");

    match em.tick().await.expect("burn covers E") {
        EmitOutcome::Burned { reason, .. } => {
            assert!(reason.contains("do not agree"), "{reason}");
        }
        other => panic!("a stable torn feed must pay nobody: {other:?}"),
    }
    assert_burn_covers_e(&accepted);
    assert_eq!(em.scored_epoch(), 0);
}

/// Re-reading is a retry, not a refusal: a feed that settles after a publish
/// is scored at the revision it settled on, so ordinary backend activity does
/// not cost miners an epoch.
#[tokio::test]
async fn a_feed_that_settles_is_scored_at_the_revision_it_settled_on() {
    let backend = spawn_shifting_backend(1).await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(backend), &gateway);

    assert!(matches!(
        em.tick().await.expect("scored"),
        EmitOutcome::Scored { paid: 1, .. }
    ));
    assert!(
        leaf_for(&accepted, CHAMPION)["score_or_absence"]["score"]["value"]
            .as_u64()
            .unwrap_or_default()
            > 0,
        "the settled revision must pay the champion"
    );
}

/// The reverse direction: once an epoch is scored, an outage inside it must
/// not take the score back, or a backend hiccup would decide the epoch.
#[tokio::test]
async fn an_outage_after_a_scored_epoch_holds_instead_of_burning_it() {
    let (backend, _hits, healthy) = spawn_flaky_backend().await;
    healthy.store(true, Ordering::Relaxed);
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(backend), &gateway);

    assert!(matches!(
        em.tick().await.expect("scored"),
        EmitOutcome::Scored { paid: 1, .. }
    ));
    let after_scored = accepted_count(&accepted);

    healthy.store(false, Ordering::Relaxed);
    match em.tick().await.expect("hold") {
        EmitOutcome::Held { epoch, .. } => {
            assert_eq!(epoch, chain::fake_defaults::SUBNET_EPOCH_INDEX);
        }
        other => panic!("an outage must not overwrite a scored epoch: {other:?}"),
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
