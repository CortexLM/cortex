//! Reading the routes a topic registered: the dynamic mux's read side.
//!
//! An install **writes** the routes a topic claims into `proof_topic_api`
//! ([`crate::install`]). This module is the other half: the challenge reads
//! that table to answer `/challenge/{topic_id}/…`, so the routes a topic
//! exposes are the ones its install recorded — never a compiled-in list.
//!
//! A stored path is **relative** to the topic's own prefix
//! ([`proof_topic_authoring::is_relative_api_path`]), so the resolver owns the
//! prefix and a row cannot carry an absolute path out of its topic's
//! namespace.
//!
//! # The cache, and how an install invalidates it
//!
//! A request must not pay a table read, and it must not be answered from a
//! table an install has since changed. An install is a **different process**
//! (the operator's `proof-admin`), so no in-process signal can carry it: the
//! cache is therefore keyed by a **generation** — the sum of
//! `proof_topic_route_revision` — and a cached topic is used only while the
//! generation it was read under still holds.
//!
//! The revision is what makes a **replacement** visible, and a row count is
//! not: when an RLM-authored install supersedes a bundle-authored set, the
//! install deletes the routes the new set does not claim and inserts its own,
//! and a count can be unchanged by that (delete three, insert three). The
//! revision is monotonic per topic and bumped in the same transaction as the
//! reconciliation, so it moves on any change to the route table — addition or
//! replacement. [`TopicRouteMux::invalidate`] is the in-process form of the
//! same thing.
//!
//! # What a resolution means
//!
//! [`TopicRouteMux::resolve`] answers whether the topic registered the path,
//! and for which method. It does **not** decide what a route *does*: the
//! table records a claim (`path`, `method`, `summary`), and the install
//! section is explicit that nothing here interprets a topic's API. A path the
//! topic did not claim resolves to [`Resolved::NotRegistered`], so the
//! challenge can answer 404 rather than invent a route.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use async_trait::async_trait;
use sqlx::PgPool;

use crate::InstallError;
use proof_topic_authoring::ApiRoute;

/// Whether `id` has the shape of a topic id.
///
/// The shape is the shared database's own: `proof_topic_api.topic_id ~
/// '^[a-z0-9][a-z0-9-]{1,62}$'` (migration `0025`). An id outside it cannot
/// be in the table, so it is refused without a query — which is what keeps a
/// stray request from costing a database round trip.
#[must_use]
pub fn is_topic_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    let rest = chars.as_str();
    (1..=62).contains(&rest.chars().count())
        && rest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// What the mux reads a topic's routes from.
///
/// A trait rather than a `PgPool` so the resolver can be exercised without a
/// database, and so a host with no database can say so instead of answering
/// from a table it never read.
#[async_trait]
pub trait TopicRouteSource: Send + Sync {
    /// Every route `topic_id` registered, in a stable order.
    ///
    /// # Errors
    ///
    /// [`InstallError::Db`] when the read fails.
    async fn routes(&self, topic_id: &str) -> Result<Vec<ApiRoute>, InstallError>;

    /// A value that changes whenever an install writes a route row.
    ///
    /// # Errors
    ///
    /// [`InstallError::Db`] when the read fails.
    async fn generation(&self) -> Result<i64, InstallError>;
}

/// The Postgres read side: the table an install writes.
pub struct PgTopicRoutes {
    /// Pool over the shared challenge database.
    pub pool: PgPool,
}

impl PgTopicRoutes {
    /// Read `proof_topic_api` through `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TopicRouteSource for PgTopicRoutes {
    async fn routes(&self, topic_id: &str) -> Result<Vec<ApiRoute>, InstallError> {
        crate::install::topic_routes(&self.pool, topic_id).await
    }

    /// Sum of the per-topic route revisions.
    ///
    /// A revision is bumped in the same transaction as the route
    /// reconciliation (migration `0028`), so the sum moves on **any** change
    /// to the route table — an addition and a replacement alike. A row count
    /// cannot see a replacement, which is why this is not one. See the module
    /// docs.
    async fn generation(&self) -> Result<i64, InstallError> {
        // `sum` is NUMERIC in Postgres; the cast keeps the read an `i64` and
        // the empty-table case a `0` rather than a decode error.
        let rows: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(revision), 0)::bigint FROM proof_topic_route_revision",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| InstallError::Db(e.to_string()))?;
        Ok(rows)
    }
}

/// What one lookup found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// The topic registered this path for this method (or for `*`).
    Route(ApiRoute),
    /// The topic registered this path, but not for the method asked.
    MethodNotAllowed,
    /// Nothing is registered for this topic and path.
    NotRegistered,
}

/// The dynamic route mux: a route table read behind a generation-keyed cache.
pub struct TopicRouteMux {
    source: Arc<dyn TopicRouteSource>,
    cache: RwLock<Cache>,
}

/// Routes by topic, valid only while `generation` still holds.
#[derive(Debug, Default)]
struct Cache {
    /// The generation the rows were read under, when the cache holds any.
    generation: Option<i64>,
    /// A topic with an empty vector is a topic the table has no rows for.
    topics: BTreeMap<String, Vec<ApiRoute>>,
}

impl TopicRouteMux {
    /// Read routes through `source`.
    #[must_use]
    pub fn new(source: Arc<dyn TopicRouteSource>) -> Self {
        Self {
            source,
            cache: RwLock::new(Cache::default()),
        }
    }

    /// Answer one request against the routes `topic_id` registered.
    ///
    /// The method matches a route's own method or a route registered for
    /// `*`; the path is compared as stored (relative, no leading slash). The
    /// topic id is compared **as it arrives**: it is the table's key, and an
    /// id the CHECK cannot hold is refused rather than trimmed into one that
    /// resolves.
    ///
    /// A path inside the challenge's admin namespace
    /// ([`proof_topic_authoring::is_reserved_api_path`]) is never served, whatever
    /// the table holds: the install refuses to record one, and this is the
    /// read-side half for a row that predates that rule.
    ///
    /// # Errors
    ///
    /// [`InstallError::Db`] when the registry cannot be read. The caller
    /// answers **503**: an unreadable registry is not "no such route".
    pub async fn resolve(
        &self,
        topic_id: &str,
        method: &str,
        path: &str,
    ) -> Result<Resolved, InstallError> {
        if !is_topic_id(topic_id) {
            return Ok(Resolved::NotRegistered);
        }
        let path = path.trim().trim_matches('/');
        if proof_topic_authoring::is_reserved_api_path(path) {
            return Ok(Resolved::NotRegistered);
        }
        let routes = self.routes(topic_id).await?;
        let method = method.trim().to_ascii_uppercase();
        match routes.iter().find(|r| r.path == path) {
            None => Ok(Resolved::NotRegistered),
            Some(route) if route.method == "*" || route.method == method => {
                Ok(Resolved::Route(route.clone()))
            }
            Some(_) => Ok(Resolved::MethodNotAllowed),
        }
    }

    /// Drop every cached row, so the next lookup reads the table again.
    ///
    /// The cross-process form of the same thing is the generation probe (see
    /// the module docs): an install in another process is seen on the next
    /// request without this call.
    pub fn invalidate(&self) {
        let mut cache = self.cache.write().unwrap_or_else(PoisonError::into_inner);
        *cache = Cache::default();
    }

    /// The topic's routes: from the cache when the registry has not moved,
    /// from the table otherwise.
    async fn routes(&self, topic_id: &str) -> Result<Vec<ApiRoute>, InstallError> {
        let generation = self.source.generation().await?;
        if let Some(hit) = self.cached(topic_id, generation) {
            return Ok(hit);
        }
        let routes = self.source.routes(topic_id).await?;
        let mut cache = self.cache.write().unwrap_or_else(PoisonError::into_inner);
        if cache.generation != Some(generation) {
            cache.topics.clear();
            cache.generation = Some(generation);
        }
        cache.topics.insert(topic_id.to_owned(), routes.clone());
        Ok(routes)
    }

    /// The cached routes for `topic_id`, when the cache is still current.
    fn cached(&self, topic_id: &str, generation: i64) -> Option<Vec<ApiRoute>> {
        let cache = self.cache.read().unwrap_or_else(PoisonError::into_inner);
        if cache.generation != Some(generation) {
            return None;
        }
        cache.topics.get(topic_id).cloned()
    }
}
