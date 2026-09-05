//! Axum handlers for `GET /v1/site/*`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::SiteState;
use crate::upstream::{self, BOUNTY, PROOF};
use site_data::map::{activity_from_lives, hydrate_arena};
use site_types::page_slice;
use site_types::{
    bounty_frame, coding_arena, proof_frame, ArenaSlug, Governance, LandingSummary,
    MetricsEmission, MetricsPassRate, MetricsPopulation, NetworkMetrics, NetworkStats,
    ResultsMatrix, Validator,
};

/// Mount marketing site routes under `/v1/site`.
pub fn site_router(state: SiteState) -> Router {
    Router::new()
        .route("/v1/site/network", get(get_network))
        .route("/v1/site/landing", get(get_landing))
        .route("/v1/site/arenas", get(get_arenas))
        .route("/v1/site/arenas/{slug}", get(get_arena))
        .route("/v1/site/arenas/{slug}/leaderboard", get(get_leaderboard))
        .route("/v1/site/arenas/{slug}/submissions", get(get_submissions))
        .route(
            "/v1/site/arenas/coding/results-matrix",
            get(get_results_matrix),
        )
        .route("/v1/site/validators", get(get_validators))
        .route("/v1/site/weights", get(get_site_weights))
        .route("/v1/site/activity", get(get_activity))
        .route("/v1/site/metrics", get(get_metrics))
        .route("/v1/site/governance", get(get_governance))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct PageQuery {
    page: Option<u32>,
    #[serde(rename = "pageSize")]
    page_size: Option<u32>,
    status: Option<String>,
    /// Hotkey / handle / prompt substring filter (SS58 or hex).
    q: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct MetricsQuery {
    range: Option<String>,
}

fn json_err(code: StatusCode, kind: &str, msg: &str) -> Response {
    (code, Json(json!({"error": kind, "message": msg}))).into_response()
}

fn now_iso() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
    site_data::map::ms_to_iso(ms)
}

async fn fetch_proof_status(st: &SiteState) -> Option<Value> {
    upstream::get_json_opt(st, PROOF, "/v1/status").await
}

async fn fetch_bounty_status(st: &SiteState) -> Option<Value> {
    upstream::get_json_opt(st, BOUNTY, "/v1/status").await
}

/// Every live arena with trust-root emission shares applied.
///
/// Bounty and Proof are the only paid challenges. Coding stays on the list as
/// paused so the emission column still sums to the trust root.
async fn live_arenas(st: &SiteState) -> Vec<site_types::Arena> {
    let bounty = fetch_bounty_status(st).await;
    let proof = fetch_proof_status(st).await;
    let mut arenas = vec![
        coding_arena(),
        hydrate_arena(bounty_frame(), bounty.as_ref()),
        hydrate_arena(proof_frame(), proof.as_ref()),
    ];
    for arena in &mut arenas {
        apply_emission(st, arena);
    }
    arenas
}

fn epoch_from_lives(bounty: Option<&Value>, proof: Option<&Value>, chain_epoch: u64) -> u64 {
    // Challenge payloads report 0 when their own chain read never hydrated —
    // treat 0 as "unknown" so the chain-derived epoch wins.
    bounty
        .and_then(|d| d.get("epoch"))
        .and_then(Value::as_u64)
        .filter(|e| *e > 0)
        .or_else(|| {
            proof
                .and_then(|p| p.get("epoch"))
                .and_then(Value::as_u64)
                .filter(|e| *e > 0)
        })
        .unwrap_or(chain_epoch)
}

fn chain_snapshot(st: &SiteState) -> (u64, u64, Vec<Validator>) {
    let Some(chain) = st.chain.as_ref() else {
        return (0, 0, Vec::new());
    };
    let block = chain.current_block().unwrap_or(0);
    let epoch = chain.subnet_epoch_index(st.netuid).unwrap_or(0);
    let validators = match chain.block_hash(block).and_then(|h| chain.metagraph_at(&h)) {
        Ok(mg) => mg
            .hotkeys
            .iter()
            .enumerate()
            .map(|(i, hk)| Validator {
                uid: u16::try_from(i).unwrap_or(u16::MAX),
                name: format!("uid-{i}"),
                hotkey: hex_hotkey(hk),
                stake: 0.0,
                trust: 0.0,
                vtrust: 0.0,
                version: "—".into(),
                updated_blocks_ago: 0,
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    (block, epoch, validators)
}

fn hex_hotkey(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

async fn network_stats(st: &SiteState) -> NetworkStats {
    let (block, chain_epoch, validators) = chain_snapshot(st);
    let bounty = fetch_bounty_status(st).await;
    let proof = fetch_proof_status(st).await;
    let arenas = live_arenas(st).await;
    let agents: u32 = arenas.iter().map(|a| a.agents).sum();
    let tao_price = site_data::price::tao_price_usd(&st.client, &st.tao_price).await;
    let arena_count = u32::try_from(arenas.len()).unwrap_or(0);
    NetworkStats {
        epoch: epoch_from_lives(bounty.as_ref(), proof.as_ref(), chain_epoch),
        agents,
        validators: u32::try_from(validators.len()).unwrap_or(0),
        arenas: arena_count,
        emission_per_day: 0.0,
        tao_price,
        block_height: block,
        updated_at: now_iso(),
        total_stake: None,
    }
}

/// Apply trust-root emission share + sealed-vector weight to one arena.
fn apply_emission(st: &SiteState, arena: &mut site_types::Arena) {
    let slug = arena.slug.as_str();
    let shares = site_data::weights::configured_shares(st.trust_root());
    if let Some((_, share)) = shares.iter().find(|(s, _)| s == slug) {
        arena.emission_share = *share;
    }
    arena.weight =
        site_data::weights::arena_weight(st.trust_root(), st.latest_sealed_bundle(), slug);
}

async fn get_network(State(st): State<SiteState>) -> impl IntoResponse {
    Json(network_stats(&st).await)
}

async fn get_landing(State(st): State<SiteState>) -> impl IntoResponse {
    let stats = network_stats(&st).await;
    let arenas = live_arenas(&st).await;
    Json(LandingSummary {
        stats,
        arenas,
        activity: activity_from_lives(10),
    })
}

async fn get_arenas(State(st): State<SiteState>) -> impl IntoResponse {
    Json(live_arenas(&st).await)
}

async fn get_arena(State(st): State<SiteState>, Path(slug): Path<String>) -> Response {
    let Some(slug) = ArenaSlug::parse(&slug) else {
        return json_err(StatusCode::NOT_FOUND, "not_found", "unknown arena");
    };
    let mut arena = match slug {
        ArenaSlug::Coding => coding_arena(),
        ArenaSlug::Bounty => hydrate_arena(bounty_frame(), fetch_bounty_status(&st).await.as_ref()),
        ArenaSlug::Proof => hydrate_arena(proof_frame(), fetch_proof_status(&st).await.as_ref()),
    };
    apply_emission(&st, &mut arena);
    Json(arena).into_response()
}

fn empty_leaderboard_json(page: u32, page_size: u32) -> Value {
    let empty = page_slice::<crate::LeaderboardRow>(&[], page, page_size);
    json!({
        "items": empty.items,
        "page": empty.page,
        "pageSize": empty.page_size,
        "total": empty.total,
        "pageCount": empty.page_count,
        "epoch": 0,
        "updatedAt": now_iso(),
    })
}

async fn get_leaderboard(Path(slug): Path<String>, Query(q): Query<PageQuery>) -> Response {
    let Some(_) = ArenaSlug::parse(&slug) else {
        return json_err(StatusCode::NOT_FOUND, "not_found", "unknown arena");
    };
    let page = q.page.unwrap_or(1);
    let page_size = q.page_size.unwrap_or(24);
    let _ = (q.status.as_deref(), q.q.as_deref());
    Json(empty_leaderboard_json(page, page_size)).into_response()
}

async fn get_submissions(Path(slug): Path<String>, Query(q): Query<PageQuery>) -> Response {
    let Some(_) = ArenaSlug::parse(&slug) else {
        return json_err(StatusCode::NOT_FOUND, "not_found", "unknown arena");
    };
    let page = q.page.unwrap_or(1);
    let page_size = q.page_size.unwrap_or(24);
    let _ = (q.status.as_deref(), q.q.as_deref());
    Json(page_slice::<crate::Submission>(&[], page, page_size)).into_response()
}

async fn get_results_matrix() -> impl IntoResponse {
    Json(ResultsMatrix {
        arena: ArenaSlug::Coding,
        tasks: Vec::new(),
        rows: Vec::new(),
    })
}

async fn get_validators(
    State(st): State<SiteState>,
    Query(q): Query<PageQuery>,
) -> impl IntoResponse {
    let (_, _, validators) = chain_snapshot(&st);
    let page = page_slice(&validators, q.page.unwrap_or(1), q.page_size.unwrap_or(24));
    Json(page)
}

/// Sealed weight vector + configured emission split (trust root + bundle).
async fn get_site_weights(State(st): State<SiteState>) -> impl IntoResponse {
    Json(site_data::weights::site_weights(
        st.netuid,
        st.trust_root(),
        st.latest_sealed_bundle(),
    ))
}

async fn get_activity(Query(q): Query<LimitQuery>) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(10).min(100) as usize;
    Json(activity_from_lives(limit))
}

async fn get_metrics(
    State(st): State<SiteState>,
    Query(q): Query<MetricsQuery>,
) -> impl IntoResponse {
    let range = q.range.unwrap_or_else(|| "30".into());
    let (block, chain_epoch, validators) = chain_snapshot(&st);
    let bounty = fetch_bounty_status(&st).await;
    let proof = fetch_proof_status(&st).await;
    let arenas = live_arenas(&st).await;
    let epoch = epoch_from_lives(bounty.as_ref(), proof.as_ref(), chain_epoch);
    let agents: u32 = arenas.iter().map(|a| a.agents).sum();
    let tao_price = site_data::price::tao_price_usd(&st.client, &st.tao_price).await;
    let weights =
        site_data::weights::site_weights(st.netuid, st.trust_root(), st.latest_sealed_bundle());

    Json(NetworkMetrics {
        range,
        epoch,
        kpis: site_data::metrics::kpis(agents, validators.len(), tao_price, block, &weights),
        emission: MetricsEmission {
            // No epoch-close history store yet — points stay empty rather than
            // invented; the configured split above is the honest current frame.
            points: Vec::new(),
            shares: site_data::metrics::emission_shares(&arenas),
            total_this_epoch: 0.0,
        },
        pass_rate: MetricsPassRate {
            points: Vec::new(),
            latest: Vec::new(),
        },
        population: MetricsPopulation {
            rows: site_data::metrics::population_rows(&arenas),
            new_this_epoch: 0,
        },
        ledger: Vec::new(),
    })
}

async fn get_governance(State(st): State<SiteState>) -> impl IntoResponse {
    let stats = network_stats(&st).await;
    Json(Governance {
        epoch: stats.epoch,
        open_for_voting: 0,
        next_close_in: "—".into(),
        stages: Vec::new(),
        proposals: Vec::new(),
        rules: Vec::new(),
        decisions: Vec::new(),
        decisions_sealed: 0,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_registry::{CreateBackend, Registry, RegistryConfig};
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    async fn setup() -> (MockServer, MockServer, SiteState) {
        let bounty = MockServer::start().await;
        let proof = MockServer::start().await;
        let registry = Registry::shared(RegistryConfig::default());
        registry
            .create(&CreateBackend {
                challenge_id: "bounty".into(),
                base_url: bounty.uri(),
                weight: 1,
            })
            .unwrap();
        registry
            .create(&CreateBackend {
                challenge_id: "proof".into(),
                base_url: proof.uri(),
                weight: 1,
            })
            .unwrap();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let st = SiteState::new(registry, client, None, 541);
        (bounty, proof, st)
    }

    async fn call(app: axum::Router, path: &str) -> (StatusCode, Value) {
        let res = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    async fn mount_status(bounty: &MockServer, proof: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/v1/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "epoch": 3,
                "champion_id": "bounty-champ"
            })))
            .mount(bounty)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "epoch": 4,
                "champion_id": "proof-champ"
            })))
            .mount(proof)
            .await;
    }

    /// Coding plus the two live challenges (Bounty, Proof), in listing order.
    fn assert_live_arena_list(v: &Value) {
        let slugs: Vec<&str> = v
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|a| a["slug"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(slugs, vec!["coding", "bounty", "proof"], "{v}");
        assert_eq!(v[1]["name"], "Bounty");
        assert_eq!(v[1]["scoring"], "precision-severity");
        assert_eq!(v[2]["name"], "Proof");
        assert_eq!(v[2]["scoring"], "reproduced");
    }

    #[tokio::test]
    async fn arenas_list_live_challenges_and_retired_slugs_404() {
        let (bounty, proof, st) = setup().await;
        mount_status(&bounty, &proof).await;
        let app = site_router(st);

        let (s, v) = call(app.clone(), "/v1/site/arenas").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_live_arena_list(&v);
        assert_eq!(v[1]["bestScore"], "bounty-champ");
        assert_eq!(v[2]["bestScore"], "proof-champ");

        for slug in [
            "design",
            "prism",
            "relearn",
            "relearn-image",
            "relearn-agent",
        ] {
            let (s, v) = call(app.clone(), &format!("/v1/site/arenas/{slug}")).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{slug}: {v}");
            let (s, v) = call(app.clone(), &format!("/v1/site/arenas/{slug}/leaderboard")).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{slug} leaderboard: {v}");
            let (s, v) = call(app.clone(), &format!("/v1/site/arenas/{slug}/submissions")).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{slug} submissions: {v}");
        }

        let (s, v) = call(app.clone(), "/v1/site/arenas/coding/submissions").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["total"], 0);

        let (s, v) = call(app.clone(), "/v1/site/arenas/bounty/leaderboard").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["total"], 0);

        let (s, v) = call(app.clone(), "/v1/site/arenas/proof/submissions").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["total"], 0);

        let (s, v) = call(app.clone(), "/v1/site/arenas/design/duels").await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
        let (s, v) = call(app.clone(), "/v1/site/arenas/prism/window").await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{v}");

        let (s, v) = call(app, "/v1/site/activity?limit=8").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(v.as_array().unwrap().is_empty(), "{v}");
    }

    #[tokio::test]
    async fn arenas_carry_trust_root_shares_and_weights_endpoint() {
        use std::sync::Arc;
        use trustroot::{ChallengeEntry, ChallengesBody, ParticipantPolicy};
        let (bounty, proof, st) = setup().await;
        mount_status(&bounty, &proof).await;
        let entry = |id: &str, bps: u16| ChallengeEntry {
            id: id.as_bytes().to_vec(),
            public_key: [7u8; 32],
            emission_share_bps: bps,
            policy: ParticipantPolicy::AllMetagraphHotkeys,
        };
        let st = st.with_weights(
            Arc::new(ChallengesBody {
                challenges: vec![entry("bounty", 2000), entry("proof", 8000)],
            }),
            Arc::new(|| None),
        );
        let app = site_router(st);

        let (s, v) = call(app.clone(), "/v1/site/arenas").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v[1]["slug"], "bounty");
        assert_eq!(v[1]["emissionShare"], 0.2);
        assert_eq!(v[2]["slug"], "proof");
        assert_eq!(v[2]["emissionShare"], 0.8);
        assert_eq!(v[1]["weight"], 0.0);
        assert_eq!(v[2]["weight"], 0.0);

        let (s, v) = call(app.clone(), "/v1/site/weights").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["sealed"], false);
        assert_eq!(v["burnShare"], 1.0);
        assert_eq!(v["emissionShares"][0]["arena"], "bounty");
        assert_eq!(v["emissionShares"][0]["share"], 0.2);
        assert_eq!(v["emissionShares"][1]["arena"], "proof");
        assert_eq!(v["emissionShares"][1]["share"], 0.8);
        assert!(v["hotkeyWeights"].as_array().unwrap().is_empty());
    }
}
