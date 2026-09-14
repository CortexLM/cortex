//! The dynamic topic routes: `/challenge/{topic_id}/…`, answered from the
//! route table an install wrote.
//!
//! A topic's own routes are **topic data**. The install records the paths a
//! topic claims in `proof_topic_api` ([`proof_topic_install`]), and this
//! module is what serves them, so no list of topic routes is compiled into
//! the challenge.
//!
//! | Answer | When |
//! |--------|------|
//! | **200** | the topic registered the path, for this method or for `*`; the body is the row the install wrote |
//! | **405** | the topic registered the path, for another method |
//! | **404** | nothing is registered for that topic and path — an unknown topic is this case |
//! | **503** | the route table could not be read, which is **not** a 404: a 404 would read as "this topic exposes nothing" |
//!
//! The registry is read through [`TopicRouteMux`], whose cache is keyed by the
//! table's generation, so an install in another process (the operator's
//! `proof-admin`) is visible on the next request.
//!
//! # Why the answer is the row, and not a handler
//!
//! The control plane does not interpret a topic's API. An install section
//! records `path`, `method`, and the topic's own `summary`
//! ([`proof_topic_install::section`]), and nothing in this repository decides
//! what a path *means* — a topic that needs a behavior writes it in its own
//! bundle. A registered route therefore answers with the row it resolved: the
//! challenge's public record of what the topic claims, and nothing invented
//! for a path whose semantics live in the topic's bundle.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use proof_http::{proof_router, AppState};
use proof_task::CHALLENGE_ID;
use proof_topic_install::routes::{Resolved, TopicRouteMux};

/// The table a topic's routes come from, named in every answer so a caller
/// can see the registry resolved this and not a compiled-in list.
pub const REGISTRY_TABLE: &str = "proof_topic_api";

/// Mount the routes a topic registered for itself.
///
/// The prefix is the topic's: a stored path is relative (no leading slash, no
/// `..`), so a row cannot carry a path out of its own namespace.
pub fn topic_route_router(mux: Arc<TopicRouteMux>) -> Router {
    Router::new()
        .route("/challenge/{topic_id}", any(topic_route_root))
        .route("/challenge/{topic_id}/{*path}", any(topic_route))
        .with_state(mux)
}

/// The challenge's whole HTTP surface: the Proof routes, plus the dynamic
/// routes the installs recorded.
///
/// `None` is a host that resolved no route table (no database): it serves the
/// Proof routes alone, and a topic route is a 404 from the base router rather
/// than an answer from a table that was never read.
pub fn challenge_router(state: AppState, topic_routes: Option<Arc<TopicRouteMux>>) -> Router {
    let app = proof_router(state);
    match topic_routes {
        Some(mux) => app.merge(topic_route_router(mux)),
        None => app,
    }
}

/// The install journal, read through `proof_topic_install`.
///
/// This is what the **publish gate** consults: an `open` document is refused
/// until the topic's newest install row is `applied`. The read is the same
/// one `proof-admin topic install-log` shows, so the operator and the route
/// cannot disagree about whether a topic is installed.
pub struct PgInstallJournal {
    /// Pool over the shared challenge database.
    pub pool: sqlx::PgPool,
}

impl PgInstallJournal {
    /// Read `proof_topic_install` through `pool`.
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl proof_http::InstallJournal for PgInstallJournal {
    async fn applied(&self, topic_id: &str) -> Result<bool, String> {
        proof_topic_install::applied_install(&self.pool, topic_id)
            .await
            .map_err(|e| e.to_string())
    }
}

/// One topic route.
async fn topic_route(
    State(mux): State<Arc<TopicRouteMux>>,
    Path((topic_id, path)): Path<(String, String)>,
    method: Method,
) -> Response {
    match mux.resolve(&topic_id, method.as_str(), &path).await {
        Ok(Resolved::Route(route)) => Json(serde_json::json!({
            "challenge_id": CHALLENGE_ID,
            "topic_id": topic_id,
            "path": route.path,
            "method": route.method,
            "summary": route.summary,
            "registry": REGISTRY_TABLE,
        }))
        .into_response(),
        Ok(Resolved::MethodNotAllowed) => refusal(
            StatusCode::METHOD_NOT_ALLOWED,
            "topic_route_method_not_registered",
            &topic_id,
            &path,
        ),
        Ok(Resolved::NotRegistered) => refusal(
            StatusCode::NOT_FOUND,
            "topic_route_not_registered",
            &topic_id,
            &path,
        ),
        Err(e) => {
            // The reason stays in the log: this answer is public, and a
            // database error string is operator detail.
            tracing::warn!(topic_id = %topic_id, path = %path, "topic route registry read failed: {e}");
            refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "topic_route_registry_unavailable",
                &topic_id,
                &path,
            )
        }
    }
}

/// The topic's prefix itself. A stored path is never empty, so nothing is
/// ever registered here.
async fn topic_route_root(Path(topic_id): Path<String>) -> Response {
    refusal(
        StatusCode::NOT_FOUND,
        "topic_route_not_registered",
        &topic_id,
        "",
    )
}

/// A refusal body: the status, the reason, and what the caller asked for.
fn refusal(status: StatusCode, error: &str, topic_id: &str, path: &str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": error,
            "topic_id": topic_id,
            "path": path,
            "registry": REGISTRY_TABLE,
            "hint": "a topic serves the routes its install recorded in proof_topic_api; a path \
                     it did not register is not served here",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use proof_eval::EvalBackend;
    use proof_http::executor_slot;
    use proof_store::MemoryStore;
    use proof_task::ProofPin;
    use proof_topic_install::{ApiRoute, InstallError, TopicRouteSource};
    use tower::ServiceExt;

    /// A route table, with the generation probe an install moves.
    #[derive(Default)]
    struct Fake {
        rows: Mutex<Vec<(String, ApiRoute)>>,
        broken: Mutex<bool>,
    }

    impl Fake {
        fn new(rows: Vec<(&str, &str, &str)>) -> Arc<Self> {
            let fake = Self::default();
            {
                let mut held = fake.rows.lock().unwrap();
                for (topic, path, method) in rows {
                    held.push((
                        topic.to_owned(),
                        ApiRoute {
                            path: path.to_owned(),
                            method: method.to_owned(),
                            summary: format!("{topic} {path}"),
                        },
                    ));
                }
            }
            Arc::new(fake)
        }
    }

    #[async_trait]
    impl TopicRouteSource for Fake {
        async fn routes(&self, topic_id: &str) -> Result<Vec<ApiRoute>, InstallError> {
            if *self.broken.lock().unwrap() {
                return Err(InstallError::Db("registry unavailable".into()));
            }
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|(t, _)| t == topic_id)
                .map(|(_, r)| r.clone())
                .collect())
        }

        async fn generation(&self) -> Result<i64, InstallError> {
            if *self.broken.lock().unwrap() {
                return Err(InstallError::Db("registry unavailable".into()));
            }
            Ok(i64::try_from(self.rows.lock().unwrap().len()).unwrap())
        }
    }

    fn mux(fake: Arc<Fake>) -> Arc<TopicRouteMux> {
        Arc::new(TopicRouteMux::new(fake))
    }

    /// The status and body of one request against `app`.
    async fn ask(app: Router, method: &str, uri: &str) -> (StatusCode, serde_json::Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    /// A registered route is served; a path the topic did not register is not
    /// invented, and a method it did not claim is a 405.
    #[tokio::test]
    async fn a_registered_route_is_served_and_an_unregistered_one_is_not() {
        let fake = Fake::new(vec![
            ("tb4", "status", "GET"),
            ("tb4", "runs", "*"),
            ("tb9", "status", "GET"),
        ]);
        let app = topic_route_router(mux(fake));

        let (status, body) = ask(app.clone(), "GET", "/challenge/tb4/status").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["topic_id"], "tb4");
        assert_eq!(body["path"], "status");
        assert_eq!(body["method"], "GET");
        assert_eq!(body["summary"], "tb4 status");
        assert_eq!(body["registry"], REGISTRY_TABLE);
        assert_eq!(body["challenge_id"], CHALLENGE_ID);

        // `*` answers any method; a method the route did not claim is a 405.
        let (status, _) = ask(app.clone(), "POST", "/challenge/tb4/runs").await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = ask(app.clone(), "POST", "/challenge/tb4/status").await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{body}");

        // Nothing registered: the path, another topic, the prefix itself, and
        // an id the table cannot hold are all 404.
        for uri in [
            "/challenge/tb4/nothing",
            "/challenge/tb9/runs",
            "/challenge/tb4",
            "/challenge/TB4/status",
            "/challenge/tb4/status/extra",
        ] {
            let (status, body) = ask(app.clone(), "GET", uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
            assert_eq!(body["error"], "topic_route_not_registered", "{uri}");
        }
    }

    /// An unreadable registry is a 503 — never a 404, which a miner would read
    /// as "this topic exposes nothing".
    #[tokio::test]
    async fn an_unreadable_registry_is_a_503_not_a_404() {
        let fake = Fake::new(vec![("tb4", "status", "GET")]);
        let app = topic_route_router(mux(fake.clone()));
        let (status, _) = ask(app.clone(), "GET", "/challenge/tb4/status").await;
        assert_eq!(status, StatusCode::OK);

        *fake.broken.lock().unwrap() = true;
        let (status, body) = ask(app, "GET", "/challenge/tb4/status").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["error"], "topic_route_registry_unavailable");
        assert!(
            !body.to_string().contains("registry unavailable"),
            "the database's own words stay in the log: {body}"
        );
    }

    /// The challenge's surface keeps every Proof route and adds the topic
    /// ones: merging must not shadow `/health`, `/v1/…`, or the topic prefix.
    #[tokio::test]
    async fn the_challenge_router_keeps_the_proof_routes_and_adds_the_topic_ones() {
        let fake = Fake::new(vec![("tb4", "status", "GET")]);
        let state = AppState {
            store: MemoryStore::new(),
            pin: ProofPin::default(),
            backend: EvalBackend::Sim,
            live_scorer: None,
            offer: None,
            executor: executor_slot(None),
            judge_api_key: None,
            admin_hashes: Arc::new(Vec::new()),
            vm_probe: None,
            // This file's subject is the route mux, not the publish gate.
            install_journal: None,
            epoch: 0,
        };
        let app = challenge_router(state.clone(), Some(mux(fake)));

        let (status, body) = ask(app.clone(), "GET", "/health").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["challenge_id"], CHALLENGE_ID);
        let (status, _) = ask(app.clone(), "GET", "/v1/status").await;
        assert_eq!(status, StatusCode::OK, "the Proof routes still answer");
        let (status, _) = ask(app.clone(), "GET", "/challenge/tb4/status").await;
        assert_eq!(status, StatusCode::OK, "and the topic route is served");

        // A host with no route table serves the Proof routes alone.
        let app = challenge_router(state, None);
        let (status, _) = ask(app.clone(), "GET", "/health").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = ask(app, "GET", "/challenge/tb4/status").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
