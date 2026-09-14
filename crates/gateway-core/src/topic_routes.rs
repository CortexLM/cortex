//! Topic routes at the gateway: `/challenge/{topic_id}/…`.
//!
//! A topic publishes the routes it exposes itself. They live in the shared
//! database's `proof_topic_api` table, and the **Proof challenge** serves
//! them (`/challenge/{topic_id}/…`, resolved against that table). The
//! gateway's registry, however, knows only *challenges*: it picks a backend
//! by challenge id (`proof`, `bounty`), so a request addressed to a **topic
//! id** has no backend of its own.
//!
//! This module is the rule that bridges the two, and it is deliberately
//! small and pure so it can be reasoned about without a registry:
//!
//! - A topic-shaped id the registry does not know is forwarded to the
//!   **Proof** backend, with the whole `/challenge/{topic_id}/…` path — the
//!   topic id is the resolver's key, so it is *not* stripped the way a
//!   challenge id is.
//! - The challenge is the gate: it looks the id up in `proof_topic_api` and
//!   answers **404** for a topic it does not hold, so a forwarded id that is
//!   not a topic costs a lookup and nothing else.
//! - An id that is **not** topic-shaped keeps the registry's own answer
//!   (`no healthy backends for challenge_id=…`), so a mistyped challenge id
//!   is not silently re-addressed to Proof.
//!
//! The shape is the shared database's own constraint on
//! `proof_topic_api.topic_id` (`'^[a-z0-9][a-z0-9-]{1,62}$'`, migration
//! `0025`), repeated here because the gateway must decide before it has a
//! database. It is also the shape `proof_topic_install::is_topic_id` uses;
//! the constraint is frozen in a migration, so the two cannot drift without
//! a new migration.

#![forbid(unsafe_code)]

use gateway_registry::{Backend, Registry};

/// The challenge whose topics publish their own routes.
pub const PROOF_CHALLENGE_ID: &str = "proof";

/// Whether `id` is shaped like a **topic id**.
///
/// See the module docs: this is the database's own CHECK, repeated for the
/// one decision the gateway has to make without a database.
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

/// The Proof backend and the path to forward, when `challenge_id` is a
/// **topic id** rather than a registered challenge.
///
/// `None` for an id that is not topic-shaped, and on a host with no Proof
/// backend: both keep the registry's own `no healthy backends` answer. This
/// is the whole decision the proxy needs, in one call.
#[must_use]
pub fn topic_route(
    registry: &Registry,
    challenge_id: &str,
    rest: &str,
) -> Option<(Backend, String)> {
    let backend = topic_route_backend(registry, challenge_id)?;
    Some((backend, topic_route_path(challenge_id, rest)))
}

/// The Proof backend, when `challenge_id` is a **topic id** rather than a
/// registered challenge.
///
/// `None` for an id that is not topic-shaped, and on a host with no Proof
/// backend: both keep the registry's own `no healthy backends` answer.
#[must_use]
pub fn topic_route_backend(registry: &Registry, challenge_id: &str) -> Option<Backend> {
    if !is_topic_id(challenge_id) {
        return None;
    }
    registry.pick(PROOF_CHALLENGE_ID).ok()
}

/// The upstream path for a topic route: the whole `/challenge/{topic_id}/…`
/// path, without the leading slash.
///
/// The topic id is kept because the challenge resolves the route *by* it; a
/// challenge id is stripped instead (the backend's own routes are relative to
/// its challenge).
#[must_use]
pub fn topic_route_path(topic_id: &str, rest: &str) -> String {
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        format!("challenge/{topic_id}")
    } else {
        format!("challenge/{topic_id}/{rest}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape is the migration's CHECK: `[a-z0-9]` then 1–62 of
    /// `[a-z0-9-]`.
    #[test]
    fn a_topic_id_is_the_shape_the_table_holds() {
        for good in ["tb4", "tbench", "a-b", "t9", &"a".repeat(63)] {
            assert!(is_topic_id(good), "{good:?}");
        }
        for bad in [
            "",
            "t",
            "TB4",
            "tb_4",
            "tb4/",
            "/tb4",
            "tb4 status",
            "-tb4",
            &"a".repeat(64),
        ] {
            assert!(!is_topic_id(bad), "{bad:?}");
        }
    }

    /// A challenge id is stripped; a topic id is not.
    #[test]
    fn a_topic_route_keeps_its_topic_id_in_the_path() {
        assert_eq!(topic_route_path("tb4", "status"), "challenge/tb4/status");
        assert_eq!(
            topic_route_path("tb4", "/v1/runs/7"),
            "challenge/tb4/v1/runs/7"
        );
        assert_eq!(topic_route_path("tb4", ""), "challenge/tb4");
        assert_eq!(topic_route_path("tb4", "/"), "challenge/tb4");
    }
}
