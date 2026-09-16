//! The full Bounty reward linkage: a paired miner's report becomes weight.
//!
//! Every other test in this crate stops at one link. This one walks the whole
//! chain, because each link can pass on its own while the chain pays nobody:
//!
//! ```text
//!   miner hotkey ──pair──▶ Chat account ──report──▶ operator adjudicate
//!        │                                              │
//!        │                                     CortexLM/backend publishes
//!        │                                              │
//!        └──metagraph `E`──◀── leaf emitter ◀──public feed
//!                                  │
//!                        POST /v1/weights/raw
//!                                  │
//!                            admin seal ──▶ GET /v1/weights/latest (sealed)
//! ```
//!
//! Two properties are the point of the file:
//!
//! 1. **The pairing is not decorative.** The hotkey that signs the pairing
//!    challenge is the hotkey the published row credits, and it is the hotkey
//!    that gets the positive leaf. A miner who cannot pair, or whose report
//!    lands under a different key, is never paid — so the test signs a real
//!    sr25519 challenge and drives the real HTTP routes.
//! 2. **Bounty cannot take the subnet down with it.** Bounty holds a paid
//!    trust-root row, so an epoch where it produces no weight must still cover
//!    `E`; otherwise D24 fails and `POST /v1/admin/seal` 409s for *every*
//!    challenge, including the one that did score. The seal at the end of this
//!    file proves both directions in one bundle: bounty pays its champion, and
//!    a challenge that read nothing still seals.
//!
//! Nothing here reads the real CortexLM/backend. The feed is a stand-in that
//! serves the two public routes; that is exactly what the operator points
//! `BOUNTY_BACKEND_PUBLIC_URL` at, and the DTO is the published contract.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use bounty_challenge::{
    bounty_router, hash_admin_token, AppState, BountyEmitter, BountyStore, EmitOutcome,
    EmitterOutcomeKind, GatewayClient, GatewayClientConfig, ScoringBackend,
};
use bounty_challenge_task::{
    hotkey_hex, hotkey_ss58, pairing_code, public_from_mini_secret, sign_pair_challenge,
    PairChallenge,
};
use bundle::{
    build_sealed_bundle, make_signed_leaf, verify_bundle, LeafV1, LocalTrustRoot,
    NoScoreReasonCode, ScoreOrAbsence, SealParams,
};
use chain::{ChainClient, ChainError, FakeChain, FakeChainConfig, Metagraph};
use challenge_common::{emit_signed_leaf_set, public_key_from_secret, verify_leaf_sig};
use http_body_util::BodyExt;
use tokio::net::TcpListener;
use tower::ServiceExt;
use trustroot::{
    measurements_digest, ChallengeEntry, ChallengesBody, MeasurementsBody, ParticipantPolicy,
    BPS_DENOM,
};

/// Netuid the fake chain serves.
const NETUID: u16 = 541;

/// The miner's hotkey mini-secret. The *hotkey* is its sr25519 public key, so
/// the pairing signature, the published row, and the metagraph entry are all
/// the same key — which is the property this file exists to check.
const MINER_SECRET: [u8; 32] = [0xA1; 32];

/// A metagraph hotkey that filed nothing. It still has to appear in `E`.
const SILENT: [u8; 32] = [0xC3; 32];

/// The miner's public hotkey, as the metagraph and the pairing both see it.
fn miner_hotkey() -> [u8; 32] {
    public_from_mini_secret(&MINER_SECRET).expect("miner pk")
}

/// Challenge leaf-signing keys (throwaway, in-test only).
const BOUNTY_SK: [u8; 32] = [0x11; 32];
const PROOF_SK: [u8; 32] = [0x22; 32];
const GATEWAY_SK: [u8; 32] = [0x33; 32];

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
    delegate!(fn axon(&self, netuid: u16, hotkey: &[u8]) -> Result<Option<chain::AxonInfo>, ChainError>);
    delegate!(fn axons(&self, netuid: u16) -> Result<Vec<(Vec<u8>, chain::AxonInfo)>, ChainError>);
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
        payload: chain::WeightsTlockPayload,
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

fn fake_chain() -> FakeChain {
    FakeChain::new(FakeChainConfig {
        netuid: NETUID,
        hotkeys: vec![miner_hotkey().to_vec(), SILENT.to_vec()],
        ..FakeChainConfig::default()
    })
}

// --- the miner side: pair, report, adjudicate (real HTTP routes) -------------

/// A pairing payload signed by the miner's own hotkey, over the canonical
/// challenge string. `ctx bounty pair` builds exactly this shape.
fn signed_pair_payload(secret: &[u8; 32], account_id: &str) -> serde_json::Value {
    let pk = public_from_mini_secret(secret).expect("pk");
    let challenge = PairChallenge {
        account_id: account_id.into(),
        nonce: "0123456789abcdef".into(),
        exp: 2_000_000_000,
    };
    let encoded = challenge.encode().expect("encode");
    let sig = sign_pair_challenge(secret, &encoded).expect("sign");
    // The code a miner pastes into Chat carries the same signature.
    let code = pairing_code(&encoded, &hex::encode(sig), &hotkey_ss58(&pk));
    assert!(code.contains(&hotkey_ss58(&pk)));
    serde_json::json!({
        "account_id": challenge.account_id,
        "hotkey": hotkey_ss58(&pk),
        "nonce": challenge.nonce,
        "exp": challenge.exp,
        "signature": hex::encode(sig),
        "terms_accepted": true,
    })
}

/// A report body that clears the substance floor.
fn report_body(session: &str) -> serde_json::Value {
    serde_json::json!({
        "session": session,
        "title": "seal returns 500 on an empty bundle",
        "body": "POST /v1/admin/seal answers 500 when the bundle has no leaves, \
                 instead of the documented 400. Observed on master at commit tip.",
        "repro_steps": "curl the seal route with no leaves posted and watch it 500",
    })
}

async fn json_req(
    app: &Router,
    method: &str,
    uri: &str,
    body: serde_json::Value,
    auth: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(a) = auth {
        b = b.header(axum::http::header::AUTHORIZATION, format!("Bearer {a}"));
    }
    let req = b
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("req");
    let resp = app.clone().oneshot(req).await.expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
    (status, v)
}

/// Drive the real challenge routes: pair a hotkey, file a report, adjudicate it
/// `valid` at `severity`. Returns the adjudicated report body.
async fn pair_report_adjudicate(operator_token: &str) -> serde_json::Value {
    let app = bounty_router(AppState {
        store: BountyStore::new(),
        session_secret: Arc::new(b"test-session-secret".to_vec()),
        scoring: ScoringBackend::BackendPublic,
        admin_hashes: Arc::new(vec![hash_admin_token(operator_token)]),
        emitter: None,
    });

    let (st, paired) = json_req(
        &app,
        "POST",
        "/v1/pair",
        signed_pair_payload(&MINER_SECRET, "acct-miner-rewards"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "pair: {paired}");
    // The binding is the miner's own hotkey, not something the host invented.
    assert_eq!(paired["miner_hotkey"], hotkey_hex(&miner_hotkey()));
    let session = paired["session"].as_str().expect("session");

    let (st, created) = json_req(&app, "POST", "/v1/reports", report_body(session), None).await;
    assert_eq!(st, StatusCode::CREATED, "report: {created}");
    assert_eq!(created["state"], "pending");

    let (st, adjudicated) = json_req(
        &app,
        "POST",
        "/v1/admin/adjudicate",
        serde_json::json!({
            "report_id": created["id"],
            "verdict": "valid",
            "severity": "major",
        }),
        Some(operator_token),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "adjudicate: {adjudicated}");
    assert_eq!(adjudicated["adjudication"], "valid");
    assert_eq!(adjudicated["severity"], "major");
    adjudicated
}

// --- the backend side: the published feed ------------------------------------

/// The backend's published view of the adjudicated report above.
///
/// This is the handoff the whole challenge depends on: Cortex adjudicates
/// internally, CortexLM/backend publishes, and this host reads the published
/// rows. If the published row names a different hotkey than the one that
/// paired, the miner is paid nothing — which is why the assertion at the end
/// of the test is on `MINER`'s leaf rather than on "a leaf scored".
fn published_row(severity: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "report-1",
        "hotkey": hotkey_ss58(&miner_hotkey()),
        "status": "valid",
        "severity": severity,
        "problem_found": "seal returns 500 on an empty bundle",
        "adjudicator": "bounty-adjudicator@cortex",
        "justification": "reproduced on master at commit tip",
        "adjudicated_at": "2026-09-01T00:00:00Z",
        "created_at": "2026-09-01T00:00:00Z",
    })
}

/// One justified, priced finding so the miner clears `MIN_HOLDOUT_DECIDED`.
fn payable_rows() -> Vec<serde_json::Value> {
    (1..=3)
        .map(|i| {
            let mut row = published_row("major");
            row["id"] = serde_json::json!(format!("report-{i}"));
            row["problem_found"] = serde_json::json!(format!("regression {i} on the seal path"));
            row
        })
        .collect()
}

/// Stand-in for `{BOUNTY_BACKEND_PUBLIC_URL}`: the two public routes.
///
/// The leaderboard row carries the real `valid_count` for the published
/// reports. A count that did not match would be refused as a torn pair before
/// it could ever score, so the stand-in has to publish one coherent revision —
/// which is also what the live backend does.
async fn spawn_public_feed(rows: Vec<serde_json::Value>) -> String {
    let rows = Arc::new(rows);
    let leaderboard = Arc::clone(&rows);
    let reports = Arc::clone(&rows);
    let app = Router::new()
        .route(
            "/v1/bounty/public/leaderboard",
            get(move || {
                let rows = Arc::clone(&leaderboard);
                async move {
                    let valid = rows
                        .iter()
                        .filter(|r| r["status"] == serde_json::Value::String("valid".to_owned()))
                        .count();
                    let items = if rows.is_empty() {
                        Vec::new()
                    } else {
                        vec![serde_json::json!({
                            "hotkey": rows[0]["hotkey"],
                            "valid_count": valid,
                        })]
                    };
                    Json(serde_json::json!({ "items": items }))
                }
            }),
        )
        .route(
            "/v1/bounty/public/reports",
            get(move || {
                let rows = Arc::clone(&reports);
                async move { Json(serde_json::json!({ "items": *rows })) }
            }),
        );
    serve(app).await
}

/// A feed that answers 503 on both routes — the shape of a backend outage.
async fn spawn_down_feed() -> String {
    let app = Router::new().fallback(|| async { StatusCode::SERVICE_UNAVAILABLE });
    serve(app).await
}

async fn serve(app: Router) -> String {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Leaves the mock gateway accepted, in arrival order.
type Accepted = Arc<Mutex<Vec<serde_json::Value>>>;

/// Stand-in for the master gateway's `POST /v1/weights/raw`.
async fn spawn_gateway() -> (String, Accepted) {
    let accepted: Accepted = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/weights/raw",
            post(
                |State(seen): State<Accepted>, Json(body): Json<serde_json::Value>| async move {
                    seen.lock().expect("lock").push(body);
                    StatusCode::ACCEPTED
                },
            ),
        )
        .with_state(Arc::clone(&accepted));
    (serve(app).await, accepted)
}

/// Rebuild the `LeafV1` the gateway received, so the seal below operates on
/// the exact leaves that were posted rather than a parallel re-derivation.
fn leaf_from_accepted(v: &serde_json::Value) -> LeafV1 {
    let soa = match &v["score_or_absence"] {
        s if s.get("score").is_some() => ScoreOrAbsence::Score {
            value: s["score"]["value"].as_u64().expect("score value"),
        },
        s => {
            let reason = u8::try_from(s["no_score"]["reason"].as_u64().expect("reason"))
                .expect("reason fits u8");
            let reason = match reason {
                0 => NoScoreReasonCode::NotAttempted,
                1 => NoScoreReasonCode::Timeout,
                2 => NoScoreReasonCode::InvalidResponse,
                3 => NoScoreReasonCode::AttestationNotVerified,
                4 => NoScoreReasonCode::MinerError,
                5 => NoScoreReasonCode::RateLimited,
                6 => NoScoreReasonCode::ChallengeInternal,
                other => panic!("unexpected absence reason {other}"),
            };
            ScoreOrAbsence::NoScore { reason }
        }
    };
    let mut miner_hotkey = [0u8; 32];
    miner_hotkey.copy_from_slice(
        &hex::decode(v["miner_hotkey"].as_str().expect("miner_hotkey")).expect("hex"),
    );
    let mut challenge_sig = [0u8; 64];
    challenge_sig.copy_from_slice(
        &hex::decode(v["challenge_sig"].as_str().expect("challenge_sig")).expect("hex"),
    );
    LeafV1 {
        challenge_id: v["challenge_id"].as_str().expect("challenge_id").into(),
        miner_hotkey,
        epoch: v["epoch"].as_u64().expect("epoch"),
        score_or_absence: soa,
        challenge_sig,
    }
}

fn emitter(backend: Option<String>, gateway_url: &str) -> BountyEmitter<LockedFake> {
    let gateway = Arc::new(
        GatewayClient::new(GatewayClientConfig {
            base_url: gateway_url.to_owned(),
            ..GatewayClientConfig::default()
        })
        .expect("gateway client"),
    );
    BountyEmitter::new(
        LockedFake(Mutex::new(fake_chain())),
        gateway,
        BOUNTY_SK,
        NETUID,
        backend,
    )
}

/// The trust root a validator loads from disk: bounty 2000, proof 8000.
fn trust_root() -> LocalTrustRoot {
    LocalTrustRoot {
        challenges: ChallengesBody {
            challenges: vec![
                ChallengeEntry {
                    id: b"bounty".to_vec(),
                    public_key: public_key_from_secret(&BOUNTY_SK).expect("bounty pk"),
                    emission_share_bps: 2000,
                    policy: ParticipantPolicy::AllMetagraphHotkeys,
                },
                ChallengeEntry {
                    id: b"proof".to_vec(),
                    public_key: public_key_from_secret(&PROOF_SK).expect("proof pk"),
                    emission_share_bps: 8000,
                    policy: ParticipantPolicy::AllMetagraphHotkeys,
                },
            ],
        },
        measurements_digest: measurements_digest(&MeasurementsBody::default()),
    }
}

/// Cover `E` for a challenge that produced no weight, exactly as that
/// challenge's own emitter does when it cannot score.
fn cover_with_noscore(
    sk: &[u8; 32],
    challenge_id: &[u8],
    epoch: u64,
    hotkeys: &BTreeSet<[u8; 32]>,
    reason: NoScoreReasonCode,
) -> BTreeMap<[u8; 32], LeafV1> {
    let scores: BTreeMap<[u8; 32], ScoreOrAbsence> = hotkeys
        .iter()
        .map(|h| (*h, ScoreOrAbsence::NoScore { reason }))
        .collect();
    emit_signed_leaf_set(sk, challenge_id, epoch, hotkeys, &scores).expect("cover E")
}

fn expected_set() -> BTreeSet<[u8; 32]> {
    let mut e = BTreeSet::new();
    e.insert(miner_hotkey());
    e.insert(SILENT);
    e
}

// --- the tests ---------------------------------------------------------------

/// The happy path, end to end: pair → report → adjudicate → publish → leaves.
///
/// The assertion that matters is that `MINER` — the hotkey that signed the
/// pairing challenge — is the one holding a positive leaf. Every intermediate
/// step could be green while the credit lands on a different key, and this is
/// the only place that catches it.
#[tokio::test]
async fn a_paired_miners_adjudicated_report_becomes_a_scored_leaf() {
    // 1. The miner pairs and files; an operator adjudicates.
    let adjudicated = pair_report_adjudicate("op-token").await;
    assert_eq!(adjudicated["miner_hotkey"], hotkey_hex(&miner_hotkey()));

    // 2. The backend publishes that adjudication on the public feed.
    let feed = spawn_public_feed(payable_rows()).await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(feed), &gateway);

    // 3. The emitter turns the published rows into signed leaves for `E`.
    let epoch = match em.tick().await.expect("tick") {
        EmitOutcome::Scored {
            epoch,
            participants,
            paid,
            ..
        } => {
            assert_eq!(participants, 2, "every hotkey in E needs a leaf");
            assert_eq!(paid, 1, "exactly the adjudicated miner was paid");
            epoch
        }
        other => panic!("a readable, payable feed must score: {other:?}"),
    };
    let view = em.status().view();
    assert_eq!(view.last_outcome, EmitterOutcomeKind::Scored);
    assert_eq!(view.last_paid, 1);
    assert!(view.last_feed_read);

    // 4. The leaf the gateway received verifies under the trust-root key, and
    //    it is `MINER`'s — the hotkey that signed the pairing challenge.
    let bounty_pk = public_key_from_secret(&BOUNTY_SK).expect("pk");
    let rows = accepted.lock().expect("lock").clone();
    assert_eq!(rows.len(), 2);
    let miner_leaf = rows
        .iter()
        .find(|v| v["miner_hotkey"] == serde_json::Value::String(hotkey_hex(&miner_hotkey())))
        .expect("a leaf for the paired miner");
    let leaf = leaf_from_accepted(miner_leaf);
    verify_leaf_sig(&leaf, &bounty_pk).expect("the sealed leaf must verify under the trust root");
    assert_eq!(leaf.challenge_id, b"bounty");
    assert_eq!(leaf.epoch, epoch);
    assert!(
        matches!(leaf.score_or_absence, ScoreOrAbsence::Score { value } if value > 0),
        "the adjudicated report must pay: {leaf:?}"
    );

    // 5. The hotkey that filed nothing is explicit, never silently omitted —
    //    an omission here is what fails D24 at the seal.
    let silent_leaf = rows
        .iter()
        .find(|v| v["miner_hotkey"] == serde_json::Value::String(hotkey_hex(&SILENT)))
        .expect("a leaf for the silent hotkey");
    assert_eq!(
        silent_leaf["score_or_absence"]["no_score"]["reason"], 0,
        "NotAttempted: the challenge did invoke E, this hotkey just has no rows"
    );
}

/// The bundle a validator actually fetches: bounty's paid leaf and proof's
/// no-score cover seal together.
///
/// This is the cross-challenge property. Bounty holds a paid trust-root row, so
/// if its emitter ever left `E` uncovered the seal would 409 for *proof* too.
/// The test asserts both halves at once — the paid vector and the fact that a
/// challenge with nothing to pay is not a reason to fail the epoch.
#[tokio::test]
async fn a_paid_bounty_seals_alongside_a_challenge_that_scored_nothing() {
    let feed = spawn_public_feed(payable_rows()).await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(feed), &gateway);
    let epoch = match em.tick().await.expect("tick") {
        EmitOutcome::Scored { epoch, .. } => epoch,
        other => panic!("expected a score: {other:?}"),
    };

    let e = expected_set();
    let mut leaves: Vec<LeafV1> = accepted
        .lock()
        .expect("lock")
        .iter()
        .map(leaf_from_accepted)
        .collect();
    // Proof had nothing to score, so it covers `E` the same way bounty does on
    // an outage. Bounty's leaves are already in the set.
    leaves.extend(
        cover_with_noscore(
            &PROOF_SK,
            b"proof",
            epoch,
            &e,
            NoScoreReasonCode::ChallengeInternal,
        )
        .into_values(),
    );

    let trust = trust_root();
    let chain = fake_chain();
    let block_b = chain::fake_defaults::LAST_EPOCH_BLOCK;
    let bundle = build_sealed_bundle(
        &chain,
        &trust,
        leaves,
        &SealParams {
            epoch,
            netuid: NETUID,
            block_b,
            gateway_secret: GATEWAY_SK,
        },
    )
    .expect("D24 completeness holds: both paid challenges covered E");

    // A validator's own verification path, against the same trust root.
    verify_bundle(&bundle, &chain, &trust).expect("the bundle a validator fetches must verify");

    // Emission shares are the two live challenges, at the configured split.
    let shares: Vec<(String, u16)> = bundle
        .body
        .emission_shares
        .iter()
        .map(|(id, bps)| (String::from_utf8_lossy(id).into_owned(), *bps))
        .collect();
    assert_eq!(
        shares,
        vec![("bounty".to_owned(), 2000), ("proof".to_owned(), 8000)],
        "the sealed split is the trust root's, not a default"
    );

    // Bounty's 2000 bps went to the miner who filed the report; proof's 8000
    // bps burned to uid 0 because it had nothing payable. The point is that the
    // vector exists at all: an uncovered `E` would have 409'd before this.
    let miner_uid = bundle
        .body
        .uid_map
        .iter()
        .find(|(h, _)| *h == miner_hotkey())
        .map(|(_, uid)| *uid)
        .expect("the miner is in the sealed metagraph");
    let miner_weight = bundle
        .body
        .final_vector
        .iter()
        .find(|(uid, _)| *uid == miner_uid)
        .map(|(_, w)| *w)
        .expect("the miner has a weight");
    assert!(
        miner_weight > 0,
        "the adjudicated miner must hold weight: {:?}",
        bundle.body.final_vector
    );
    let silent_uid = bundle
        .body
        .uid_map
        .iter()
        .find(|(h, _)| *h == SILENT)
        .map(|(_, uid)| *uid)
        .expect("the silent hotkey is in the sealed metagraph");
    assert!(
        !bundle
            .body
            .final_vector
            .iter()
            .any(|(uid, w)| *uid == silent_uid && *w > 0),
        "a hotkey that filed nothing must not be paid: {:?}",
        bundle.body.final_vector
    );
}

/// The fail-closed direction, at the seal: a bounty host that cannot read the
/// feed still seals the epoch, and still pays nobody.
///
/// Both halves are required. Paying nobody is the honest outcome; covering `E`
/// is what keeps the 409 off proof's seal. A host that did neither would take
/// the whole subnet's weights down with it.
#[tokio::test]
async fn an_unreadable_feed_pays_nobody_without_breaking_the_seal() {
    let down = spawn_down_feed().await;
    let (gateway, accepted) = spawn_gateway().await;
    let em = emitter(Some(down), &gateway);

    let epoch = match em.tick().await.expect("cover E") {
        EmitOutcome::Burned {
            epoch,
            participants,
            reason,
        } => {
            assert_eq!(participants, 2);
            assert!(reason.contains("503"), "{reason}");
            epoch
        }
        other => panic!("a down feed must burn, not score: {other:?}"),
    };
    let view = em.status().view();
    assert_eq!(view.last_outcome, EmitterOutcomeKind::Burned);
    assert!(!view.last_feed_read, "an outage is not a read feed");
    assert_eq!(view.last_paid, 0);
    assert_eq!(em.scored_epoch(), 0);

    // Every leaf is the ChallengeInternal cover: nobody is paid.
    let posted = accepted.lock().expect("lock").clone();
    assert_eq!(posted.len(), 2);
    for leaf in &posted {
        assert_eq!(
            leaf["score_or_absence"]["no_score"]["reason"], 6,
            "an unreadable feed must cover E with ChallengeInternal: {leaf}"
        );
    }

    let e = expected_set();
    let mut leaves: Vec<LeafV1> = posted.iter().map(leaf_from_accepted).collect();
    leaves.extend(
        cover_with_noscore(
            &PROOF_SK,
            b"proof",
            epoch,
            &e,
            NoScoreReasonCode::ChallengeInternal,
        )
        .into_values(),
    );

    let trust = trust_root();
    let chain = fake_chain();
    let bundle = build_sealed_bundle(
        &chain,
        &trust,
        leaves,
        &SealParams {
            epoch,
            netuid: NETUID,
            block_b: chain::fake_defaults::LAST_EPOCH_BLOCK,
            gateway_secret: GATEWAY_SK,
        },
    )
    .expect("a burn still covers E, so the epoch seals for every challenge");
    verify_bundle(&bundle, &chain, &trust).expect("verify");

    assert!(
        bundle.body.final_vector.iter().all(|(uid, _)| *uid == 0),
        "a burn epoch must sink to uid 0: {:?}",
        bundle.body.final_vector
    );
    let weights: u32 = bundle
        .body
        .final_vector
        .iter()
        .map(|(_, w)| u32::from(*w))
        .sum();
    assert!(weights > 0, "the burn vector must still be sealable");
}

/// A leaf signed by the wrong key does not verify. This is the guard on the
/// assertion above: `verify_leaf_sig` passing is evidence, not a tautology.
#[tokio::test]
async fn a_leaf_from_another_challenge_key_does_not_verify() {
    let trust = trust_root();
    let bounty_pk = trust
        .challenges
        .get(b"bounty")
        .expect("bounty row")
        .public_key;
    let proof_pk = trust
        .challenges
        .get(b"proof")
        .expect("proof row")
        .public_key;
    assert_ne!(bounty_pk, proof_pk, "the two rows carry distinct keys");

    let leaf = make_signed_leaf(
        &PROOF_SK,
        b"bounty",
        miner_hotkey(),
        1,
        ScoreOrAbsence::Score { value: 1 },
    )
    .expect("leaf");
    assert!(
        verify_leaf_sig(&leaf, &bounty_pk).is_err(),
        "a proof-key signature must not verify as a bounty leaf"
    );
    verify_leaf_sig(&leaf, &proof_pk).expect("it verifies under its own key");
}

/// `BPS_DENOM` is what `ChallengesBody::validate` enforces; the test trust root
/// above must satisfy it or the fixture is not the shape validators load.
#[test]
fn the_test_trust_root_is_a_valid_split() {
    let trust = trust_root();
    trust.challenges.validate().expect("valid split");
    let total: u32 = trust
        .challenges
        .challenges
        .iter()
        .map(|c| u32::from(c.emission_share_bps))
        .sum();
    assert_eq!(total, u32::from(BPS_DENOM));
}
