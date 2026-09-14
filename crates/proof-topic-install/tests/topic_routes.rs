//! The dynamic route mux: what the challenge answers `/challenge/{topic_id}/…`
//! with, and how an install invalidates its cache.
//!
//! These tests drive the resolver through a fake source, so they pin the
//! behavior that matters without a database: a path is served only when the
//! topic registered it, an install is visible on the next request, and an
//! unreadable registry is an error rather than a 404.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use proof_topic_install::routes::{is_topic_id, Resolved};
use proof_topic_install::{ApiRoute, InstallError, TopicRouteMux, TopicRouteSource};

/// A route table an install can "write" to, with the generation probe the
/// Postgres source exposes as a row count.
#[derive(Default)]
struct FakeRegistry {
    /// Rows, as `proof_topic_api` holds them: relative path, method, summary.
    rows: Mutex<Vec<(String, ApiRoute)>>,
    /// The generation probe's answer.
    generation: AtomicI64,
    /// How many times the routes themselves were read.
    route_reads: AtomicUsize,
    /// When set, every read fails.
    broken: Mutex<bool>,
}

impl FakeRegistry {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// An install: append rows and move the generation, exactly as the
    /// append-only table does.
    fn install(self: &Arc<Self>, topic_id: &str, routes: Vec<(&str, &str)>) {
        let mut rows = self.rows.lock().unwrap();
        for (path, method) in routes {
            rows.push((
                topic_id.to_owned(),
                ApiRoute {
                    path: path.to_owned(),
                    method: method.to_owned(),
                    summary: format!("{topic_id} {path}"),
                },
            ));
        }
        drop(rows);
        self.generation.store(
            i64::try_from(self.rows.lock().unwrap().len()).unwrap(),
            Ordering::SeqCst,
        );
    }

    fn break_it(self: &Arc<Self>) {
        *self.broken.lock().unwrap() = true;
    }
}

#[async_trait]
impl TopicRouteSource for FakeRegistry {
    async fn routes(&self, topic_id: &str) -> Result<Vec<ApiRoute>, InstallError> {
        self.route_reads.fetch_add(1, Ordering::SeqCst);
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
        Ok(self.generation.load(Ordering::SeqCst))
    }
}

/// A route a topic registered is served; a route it did not register is not
/// invented, and a method it did not claim is a 405 rather than a 200.
#[tokio::test]
async fn only_a_registered_route_resolves() {
    let registry = FakeRegistry::new();
    registry.install("tb4", vec![("status", "GET"), ("runs", "*")]);
    let mux = TopicRouteMux::new(registry.clone());

    assert_eq!(
        mux.resolve("tb4", "GET", "status").await.expect("resolve"),
        Resolved::Route(ApiRoute {
            path: "status".into(),
            method: "GET".into(),
            summary: "tb4 status".into(),
        })
    );
    // `*` is any method.
    assert!(matches!(
        mux.resolve("tb4", "POST", "runs").await.expect("resolve"),
        Resolved::Route(_)
    ));
    // A path registered for another method is not a route for this one.
    assert_eq!(
        mux.resolve("tb4", "POST", "status").await.expect("resolve"),
        Resolved::MethodNotAllowed
    );
    // A path nobody registered is not a route.
    assert_eq!(
        mux.resolve("tb4", "GET", "admin").await.expect("resolve"),
        Resolved::NotRegistered
    );
    // Another topic's routes are not this topic's.
    assert_eq!(
        mux.resolve("tb9", "GET", "status").await.expect("resolve"),
        Resolved::NotRegistered
    );
    // A topic id the table's own CHECK cannot hold is refused without a read.
    for bad in ["TB4", "tb", "tb4 ", "tb_4", "tb4/x", "", "9"] {
        assert_eq!(
            mux.resolve(bad, "GET", "status").await.expect("resolve"),
            Resolved::NotRegistered,
            "{bad:?}"
        );
    }
}

/// **The regression this cache exists for:** an install in another process
/// writes the table, and the next request sees it — with no signal beyond the
/// generation probe and no restart.
#[tokio::test]
async fn an_install_is_visible_on_the_next_request() {
    let registry = FakeRegistry::new();
    let mux = TopicRouteMux::new(registry.clone());
    assert_eq!(
        mux.resolve("tb4", "GET", "status").await.expect("resolve"),
        Resolved::NotRegistered,
        "nothing is registered yet"
    );

    registry.install("tb4", vec![("status", "GET")]);
    assert!(
        matches!(
            mux.resolve("tb4", "GET", "status").await.expect("resolve"),
            Resolved::Route(_)
        ),
        "the install's route must be served without an explicit invalidate"
    );

    // And the other way round: a topic whose routes are replaced by a later
    // install (append-only: the new rows are what the table now holds).
    registry.install("tb4", vec![("v2/runs", "POST")]);
    assert!(matches!(
        mux.resolve("tb4", "POST", "v2/runs")
            .await
            .expect("resolve"),
        Resolved::Route(_)
    ));
}

/// A request that finds the cache current does not read the table again, and
/// a generation that moved refills it.
#[tokio::test]
async fn the_cache_holds_until_the_registry_moves() {
    let registry = FakeRegistry::new();
    registry.install("tb4", vec![("status", "GET")]);
    let mux = TopicRouteMux::new(registry.clone());

    mux.resolve("tb4", "GET", "status").await.expect("first");
    let after_first = registry.route_reads.load(Ordering::SeqCst);
    mux.resolve("tb4", "GET", "status").await.expect("second");
    mux.resolve("tb4", "GET", "status").await.expect("third");
    assert_eq!(
        registry.route_reads.load(Ordering::SeqCst),
        after_first,
        "a current cache must not re-read the table"
    );

    // An install moves the generation: the next lookup reads again, and an
    // in-process caller can drop the cache outright.
    registry.install("tb4", vec![("runs", "GET")]);
    mux.resolve("tb4", "GET", "runs")
        .await
        .expect("after install");
    assert!(registry.route_reads.load(Ordering::SeqCst) > after_first);
    mux.invalidate();
    mux.resolve("tb4", "GET", "status")
        .await
        .expect("after invalidate");
    assert!(registry.route_reads.load(Ordering::SeqCst) > after_first + 1);
}

/// An unreadable registry is an error the caller answers 503 from — never a
/// 404, which would read as "this topic exposes nothing".
#[tokio::test]
async fn an_unreadable_registry_is_an_error_not_a_missing_route() {
    let registry = FakeRegistry::new();
    registry.install("tb4", vec![("status", "GET")]);
    let mux = TopicRouteMux::new(registry.clone());
    mux.resolve("tb4", "GET", "status").await.expect("warm");
    registry.break_it();
    assert!(mux.resolve("tb4", "GET", "status").await.is_err());
    assert!(mux.resolve("tb4", "GET", "other").await.is_err());
}

/// A topic id the table's own CHECK cannot hold never reaches the table.
#[test]
fn a_topic_id_is_the_shape_the_table_holds() {
    // The database's own constraint is `'^[a-z0-9][a-z0-9-]{1,62}$'`, so an
    // id outside it cannot be in `proof_topic_api` and is refused without a
    // query.
    for good in ["tb4", "tb4-topic", "t9", "a-b-c", "tb4-"] {
        assert!(is_topic_id(good), "{good:?} must be a topic id");
    }
    for bad in [
        "",
        "t",
        "TB4",
        "tb4_",
        "tb_4",
        "tb4/status",
        "tb4 status",
        "-tb4",
    ] {
        assert!(!is_topic_id(bad), "{bad:?} must not be a topic id");
    }
    assert!(
        is_topic_id(&"a".repeat(63)),
        "63 chars is the CHECK's limit"
    );
    assert!(!is_topic_id(&"a".repeat(64)));
}
