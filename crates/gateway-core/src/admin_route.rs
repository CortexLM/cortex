//! The one admin route the gateway **forwards**, and the bearer floor it holds.
//!
//! Every `/v1/admin/*` path is master-local: the gateway refuses it with a 403
//! so an operator surface is never on the public miner path. The operator
//! publish is the exception, because it is the route the install CLI calls
//! from wherever the operator runs it, and the alternative was an ephemeral
//! rewrite proxy in front of the gateway — a hop that had to be stood up, kept
//! alive, and trusted, to reach one route.
//!
//! The rule lives here rather than in `gateway::proxy` for the reason the rest
//! of this crate does: `proxy.rs` is at the repository's per-crate LOC cap, and
//! a rule this security-relevant is better off in a module whose whole subject
//! is that rule — with its own tests — than squeezed into a proxy that also
//! handles round-robin, ejection, and viewer lockdown.
//!
//! # What still holds
//!
//! Forwarding one route does **not** open the admin surface:
//!
//! - the challenge id must be the **Proof challenge**, so
//!   `/challenge/{topic_id}/v1/admin/…` keeps its 403 (a topic's own routes
//!   are never a way in);
//! - the path must be the publish route **exactly**, after the same
//!   normalization the 403 gate uses, so `v1/admin/proof/queue/drain` and
//!   every future admin route stay master-local until one is named here;
//! - the method is pinned to `POST` (a read of the publish route has no
//!   meaning — the document is in `proof_topic_version`);
//! - a bearer must be **present**. The gateway never holds the operator token
//!   and must not learn it: the challenge compares the hash. This is the cheap
//!   floor that keeps an anonymous `POST` from even costing a hop.

use axum::http::{header, HeaderMap, Method};

/// The challenge whose topics publish their own routes, and whose operator
/// surface carries the one forwarded route.
pub const PROOF_CHALLENGE_ID: &str = "proof";

/// The publish route, relative to the challenge (no leading slash): the path
/// `proof_topic_bundle::PUBLISH_PATH` carries after the challenge prefix.
pub const PUBLISH_ADMIN_PATH: &str = "v1/admin/proof/topics";

/// Whether `path` is an admin path, after normalization.
///
/// Match after path normalization: raw `v1/./admin/…` must not bypass the gate
/// when the HTTP client collapses `.` before dialing the challenge upstream.
#[must_use]
pub fn is_admin_path(path: &str) -> bool {
    let n = crate::proxy_detach::normalize_proxy_path(path);
    n.starts_with("v1/admin/") || n == "v1/admin"
}

/// The **one** admin route the gateway forwards: the operator publish.
///
/// See the module docs for the four gates that still hold.
#[must_use]
pub fn is_forwardable_admin_route(method: &Method, challenge_id: &str, rest: &str) -> bool {
    *method == Method::POST
        && challenge_id == PROOF_CHALLENGE_ID
        && crate::proxy_detach::normalize_proxy_path(rest) == PUBLISH_ADMIN_PATH
}

/// Whether the request carries a well-formed operator bearer.
///
/// Presence only: the gateway does not hold the operator token and must not
/// learn it. The challenge compares the hash; this is the cheap floor that
/// keeps an anonymous `POST` from reaching the admin route.
///
/// The scheme is **required**: `Authorization: Bearer <token>`, with a
/// non-empty token after trimming. A bare value (`Authorization: <token>`) is
/// refused here even though the challenge's own `admin_ok` would accept it on
/// a master-local call — the gateway is the public edge, and the one header
/// form it forwards is the documented one. A client that sent a bare token
/// gets a 401 naming the scheme, not a silent pass-through.
#[must_use]
pub fn has_operator_bearer(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .is_some_and(|token| !token.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_paths_are_recognized_after_normalization() {
        assert!(is_admin_path("v1/admin/proof/topics"));
        assert!(is_admin_path("v1/admin"));
        assert!(is_admin_path("v1/./admin/rounds/1/winners"));
        assert!(is_admin_path("v1//admin/rounds/1/candidates"));
        assert!(is_admin_path("./v1/admin/rounds/1/winners"));
        assert!(is_admin_path("v1/admin/../admin/rounds/1/winners"));
        assert!(is_admin_path("foo/../v1/admin/rounds/1/winners"));
        assert!(!is_admin_path("v1/./harness"));
        assert!(!is_admin_path("v1/not-admin/rounds/1/winners"));
    }

    /// Exactly one admin route is forwarded, and only for the Proof
    /// challenge: the operator publish the install CLI calls.
    #[test]
    fn only_the_operator_publish_route_is_forwarded() {
        assert!(is_forwardable_admin_route(
            &Method::POST,
            "proof",
            "v1/admin/proof/topics"
        ));
        // Normalized the same way the 403 gate is, so a client that collapses
        // `.` cannot slip a different route past the allowlist.
        assert!(is_forwardable_admin_route(
            &Method::POST,
            "proof",
            "v1/./admin/proof/topics"
        ));
        assert!(is_forwardable_admin_route(
            &Method::POST,
            "proof",
            "v1/admin/../admin/proof/topics"
        ));

        // A topic id is never a way to reach the admin surface.
        assert!(!is_forwardable_admin_route(
            &Method::POST,
            "tb4",
            "v1/admin/proof/topics"
        ));
        // Another challenge's admin surface is not this route.
        assert!(!is_forwardable_admin_route(
            &Method::POST,
            "bounty",
            "v1/admin/proof/topics"
        ));
        // Every other admin route stays master-local.
        for rest in [
            "v1/admin",
            "v1/admin/proof/executor",
            "v1/admin/proof/queue/drain",
            "v1/admin/proof/vm-orchestrator",
            "v1/admin/proof/submissions/pf/score",
            "v1/admin/proof/topics/extra",
        ] {
            assert!(
                !is_forwardable_admin_route(&Method::POST, "proof", rest),
                "{rest:?} must stay master-local"
            );
        }
        // The publish route is a POST; a read of it is not forwarded either.
        assert!(!is_forwardable_admin_route(
            &Method::GET,
            "proof",
            "v1/admin/proof/topics"
        ));
    }

    /// The forwarded route needs a **well-formed** bearer: the gateway never
    /// holds the operator token, it only refuses a call that could not be one
    /// before the hop.
    #[test]
    fn the_operator_bearer_floor_requires_the_scheme() {
        use axum::http::HeaderValue;
        let mut headers = HeaderMap::new();
        assert!(!has_operator_bearer(&headers));
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static(""));
        assert!(!has_operator_bearer(&headers));
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer  "));
        assert!(!has_operator_bearer(&headers));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer operator-token"),
        );
        assert!(has_operator_bearer(&headers));
        // A bare value is **not** a bearer here, even though the challenge's
        // own `admin_ok` accepts one on a master-local call: the gateway is
        // the public edge and forwards the documented form only.
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("operator-token"),
        );
        assert!(!has_operator_bearer(&headers));
        // A different scheme is not this one either.
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic b3BlcmF0b3I="),
        );
        assert!(!has_operator_bearer(&headers));
        // Lowercase scheme is refused too: the contract names one spelling.
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bearer operator-token"),
        );
        assert!(!has_operator_bearer(&headers));
    }
}
