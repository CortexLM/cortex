//! Reverse proxy for `/challenge/{id}/*` with registry round-robin (D20 path).
//!
//! Forwards method, filtered headers, and body to the selected upstream.
//! Transport errors and 5xx responses increment fail counters and may passively
//! eject a backend after N failures.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use bytes::Bytes;
use gateway_core::proxy_detach::{detach_proof, is_proof_submit};

use crate::api::GatewayState;
use gateway_registry::RegistryError;

/// Challenge proxy body cap: 16 MiB so a 5 MiB artefact upload plus
/// multipart JSON fields always pass through to Proof.
const PROXY_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const _: () = assert!(PROXY_MAX_BODY_BYTES >= 5 * 1024 * 1024 + 512 * 1024);

/// Hop-by-hop headers that must not be forwarded (RFC 7230).
fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}

/// Mount challenge reverse-proxy routes.
pub fn proxy_router(state: GatewayState) -> Router {
    Router::new()
        .route("/challenge/{challenge_id}", any(proxy_root))
        .route("/challenge/{challenge_id}/{*rest}", any(proxy_rest))
        .with_state(state)
}

async fn proxy_root(
    State(st): State<GatewayState>,
    Path(challenge_id): Path<String>,
    req: Request,
) -> Response {
    let query = req.uri().query().map(str::to_owned);
    proxy_inner(st, challenge_id, String::new(), query, req).await
}

async fn proxy_rest(
    State(st): State<GatewayState>,
    Path((challenge_id, rest)): Path<(String, String)>,
    req: Request,
) -> Response {
    let query = req.uri().query().map(str::to_owned);
    proxy_inner(st, challenge_id, rest, query, req).await
}

async fn proxy_inner(
    st: GatewayState,
    challenge_id: String,
    rest: String,
    query: Option<String>,
    req: Request,
) -> Response {
    let method = req.method().clone();
    let headers = req.headers().clone();
    if let Some(refusal) = admin_gate(&method, &challenge_id, &rest, &headers) {
        return refusal;
    }
    if is_blocked_report_read(&method, &rest) {
        return (
            StatusCode::FORBIDDEN,
            "report reads are not exposed via gateway; use master-local challenge port",
        )
            .into_response();
    }
    let body = match axum::body::to_bytes(req.into_body(), PROXY_MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")).into_response();
        }
    };

    let detach = is_proof_submit(&challenge_id, &method, &rest);
    let hop = async move {
        let mut last_status = StatusCode::BAD_GATEWAY;
        let mut last_msg = String::from("no upstream attempt");
        let mut last_error_resp: Option<Response> = None;
        let mut attempted = Vec::new();

        for _ in 0..2 {
            let (backend, upstream_path) = match st.registry.pick(&challenge_id) {
                Ok(b) => (b, rest.clone()),
                // An id the registry does not know may be a **topic id**: its
                // routes live in `proof_topic_api` and the Proof challenge
                // serves them (see `gateway_core::topic_routes`).
                Err(RegistryError::NoBackends(_)) => {
                    let topic =
                        gateway_core::topic_routes::topic_route(&st.registry, &challenge_id, &rest);
                    match topic {
                        Some(pair) => pair,
                        None => {
                            return (
                                StatusCode::SERVICE_UNAVAILABLE,
                                format!("no healthy backends for challenge_id={challenge_id}"),
                            )
                                .into_response();
                        }
                    }
                }
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            };

            if attempted.contains(&backend.id) {
                break;
            }
            attempted.push(backend.id);

            let url = upstream_url(&backend.base_url, &upstream_path, query.as_deref());
            match forward(&st.client, method.clone(), &url, &headers, body.clone()).await {
                ForwardResult::Ok(mut upstream_resp) => {
                    let status = upstream_resp.status();
                    if status.is_server_error() {
                        st.registry.record_failure(backend.id);
                        last_status = status;
                        last_msg = format!("upstream {status}");
                        // Lock down before retain: a viewer 5xx is still
                        // miner-controlled (cookies / CSP / cache).
                        if is_view_path(&rest) {
                            apply_view_lockdown(
                                &mut upstream_resp,
                                &st.view_frame_ancestors,
                                &rest,
                            );
                        }
                        // Keep the challenge body: miners need the JSON error
                        // (`artifact_uri must be https://`, …), not a synthetic
                        // `upstream 503 Service Unavailable` string.
                        last_error_resp = Some(upstream_resp);
                        continue;
                    }
                    st.registry.record_success(backend.id);
                    if is_view_path(&rest) {
                        apply_view_lockdown(&mut upstream_resp, &st.view_frame_ancestors, &rest);
                    }
                    return upstream_resp;
                }
                ForwardResult::Err(msg) => {
                    st.registry.record_failure(backend.id);
                    last_status = StatusCode::BAD_GATEWAY;
                    last_msg = msg;
                }
            }
        }

        if let Some(mut error_response) = last_error_resp {
            // Same floor as 2xx: a miner-controlled viewer 5xx must not carry
            // Set-Cookie / weak CSP / public cache through the gateway.
            if is_view_path(&rest) {
                apply_view_lockdown(&mut error_response, &st.view_frame_ancestors, &rest);
            }
            return error_response;
        }
        (last_status, last_msg).into_response()
    };
    if detach {
        detach_proof(hop).await
    } else {
        hop.await
    }
}

/// The admin gate: `None` when the request may proceed, a refusal otherwise.
///
/// Every `/v1/admin/*` path is master-local except the one operator route
/// [`is_forwardable_admin_route`] names (the publish the install CLI calls),
/// which additionally needs a bearer **at the gateway**: the challenge
/// compares the token hash, but an anonymous `POST` should not even cost a
/// hop. A topic id never reaches the admin surface either way.
fn admin_gate(
    method: &Method,
    challenge_id: &str,
    rest: &str,
    headers: &HeaderMap,
) -> Option<Response> {
    if !is_admin_path(rest) {
        return None;
    }
    if !is_forwardable_admin_route(method, challenge_id, rest) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                "admin API is not exposed via gateway; use master-local challenge port",
            )
                .into_response(),
        );
    }
    if !has_operator_bearer(headers) {
        return Some(
            (
                StatusCode::UNAUTHORIZED,
                "the operator publish route needs an `authorization: Bearer <token>` header",
            )
                .into_response(),
        );
    }
    None
}

/// Join base URL, remaining path, and optional query.
#[must_use]
pub fn upstream_url(base: &str, rest: &str, query: Option<&str>) -> String {
    let base = base.trim_end_matches('/');
    let mut url = if rest.is_empty() {
        format!("{base}/")
    } else {
        let rest = rest.trim_start_matches('/');
        format!("{base}/{rest}")
    };
    if let Some(q) = query {
        if !q.is_empty() {
            url.push('?');
            url.push_str(q);
        }
    }
    url
}

enum ForwardResult {
    Ok(Response),
    Err(String),
}

async fn forward(
    client: &reqwest::Client,
    method: Method,
    url: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> ForwardResult {
    let method = match reqwest::Method::from_bytes(method.as_str().as_bytes()) {
        Ok(m) => m,
        Err(e) => return ForwardResult::Err(format!("bad method: {e}")),
    };

    let mut rb = client.request(method, url);
    for (name, value) in headers {
        if is_hop_by_hop(name) || name == header::CONTENT_LENGTH {
            continue;
        }
        if let Ok(v) = value.to_str() {
            rb = rb.header(name.as_str(), v);
        }
    }

    if !body.is_empty() {
        rb = rb.body(body);
    }

    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => return ForwardResult::Err(format!("upstream error: {e}")),
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out_headers = HeaderMap::new();
    for (name, value) in resp.headers() {
        if is_hop_by_hop(name) {
            continue;
        }
        if let (Ok(n), Ok(v)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            out_headers.insert(n, v);
        }
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => return ForwardResult::Err(format!("upstream body: {e}")),
    };

    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    *response.headers_mut() = out_headers;
    ForwardResult::Ok(response)
}

/// Collapse `.` / empty / `..` segments the same way `url`/`reqwest` will before
/// the upstream request — used so gateway gates cannot be skipped via `v1/./admin`.
pub use gateway_core::proxy_detach::normalize_proxy_path;

/// Operator admin surfaces are master-local only (not on the public miner path).
///
/// Match after path normalization: raw `v1/./admin/…` must not bypass the gate
/// when the HTTP client collapses `.` before dialing the challenge upstream.
#[must_use]
pub fn is_admin_path(rest: &str) -> bool {
    let n = normalize_proxy_path(rest);
    n.starts_with("v1/admin/") || n == "v1/admin"
}

/// The **one** admin route the gateway forwards: the operator publish.
///
/// `proof-admin topic install` publishes a signed document with
/// `POST /challenge/proof/v1/admin/proof/topics` (`proof_topic_bundle::
/// PUBLISH_PATH`). The route is operator-authenticated at the challenge
/// (`admin_hashes`, the same bearer the master-local routes use), so the
/// gateway forwards it instead of refusing it — otherwise an operator on
/// staging needs a rewrite proxy in front of the gateway, which is exactly
/// the hop this replaces. Two gates still hold here:
///
/// - the challenge id must be the **Proof challenge**, not a topic id, so
///   `/challenge/{topic_id}/v1/admin/…` keeps its 403 (a topic's own routes
///   are never a way to reach the admin surface); and
/// - the path must be the publish route **exactly**, after the same
///   normalization [`is_admin_path`] uses, so `v1/admin/proof/queue/drain`
///   and every future admin route stay master-local until one is named here.
///
/// The method is pinned to `POST`: a read of the publish route has no
/// meaning (the document is in `proof_topic_version`), and a `GET` stays a
/// 403 like the rest of the admin surface.
#[must_use]
pub fn is_forwardable_admin_route(method: &Method, challenge_id: &str, rest: &str) -> bool {
    *method == Method::POST
        && challenge_id == gateway_core::topic_routes::PROOF_CHALLENGE_ID
        && normalize_proxy_path(rest) == PUBLISH_ADMIN_PATH
}

/// The publish route, relative to the challenge (no leading slash): the path
/// `proof_topic_bundle::PUBLISH_PATH` carries after the challenge prefix.
pub const PUBLISH_ADMIN_PATH: &str = "v1/admin/proof/topics";

/// Whether the request carries an operator bearer at all.
///
/// Presence only: the gateway does not hold the operator token and must not
/// learn it. The challenge compares the hash; this is the cheap floor that
/// keeps an anonymous `POST` from reaching the admin route.
#[must_use]
pub fn has_operator_bearer(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer ").or(Some(raw)))
        .is_some_and(|token| !token.trim().is_empty())
}

/// Report bodies are operator-local. POST submit stays on the miner path.
///
/// HEAD is a read: Axum would otherwise map it onto the GET handler and leak
/// status/headers (and whether a row exists) through the public gateway.
#[must_use]
pub fn is_blocked_report_read(method: &Method, rest: &str) -> bool {
    let n = normalize_proxy_path(rest);
    *method != Method::POST && (n == "v1/reports" || n.starts_with("v1/reports/"))
}

/// Miner-controlled viewer paths (`/challenge/{id}/v1/view/{run}/{page}`).
#[must_use]
pub fn is_view_path(rest: &str) -> bool {
    normalize_proxy_path(rest).starts_with("v1/view/")
}

/// Captured PNG screenshot under `/v1/view/{run}/{page}.png`.
#[must_use]
pub fn is_view_png_path(path: &str) -> bool {
    is_view_path(path)
        && std::path::Path::new(path.trim_start_matches('/'))
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
}

/// Re-apply the viewer header floor at the last serving layer (defense in
/// depth). Non-PNG paths get the full HTML lockdown (CSP `sandbox`, CORP
/// same-origin). PNG screenshots get [`crate::view_headers::screenshot_headers`]
/// (`CORP: cross-origin`) so joinbase.ai can load them with a direct absolute
/// URL and avoid proxying image bytes through Vercel. `Set-Cookie` is always
/// stripped.
fn apply_view_lockdown(resp: &mut Response, frame_ancestors: &str, view_path: &str) {
    let headers = resp.headers_mut();
    headers.remove(header::SET_COOKIE);
    let floor = if is_view_png_path(view_path) {
        // Drop HTML-only lockdown if a stale hop set them on a PNG response.
        headers.remove(header::CONTENT_SECURITY_POLICY);
        headers.remove(HeaderName::from_static("cross-origin-opener-policy"));
        headers.remove(HeaderName::from_static("permissions-policy"));
        crate::view_headers::screenshot_headers()
    } else {
        crate::view_headers::viewer_headers(frame_ancestors)
    };
    for (k, v) in floor {
        if let (Ok(name), Ok(val)) = (HeaderName::try_from(k), HeaderValue::try_from(v.as_str())) {
            headers.insert(name, val);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_by_hop_detects_connection() {
        assert!(is_hop_by_hop(&header::CONNECTION));
        assert!(is_hop_by_hop(&header::HOST));
        assert!(!is_hop_by_hop(&header::CONTENT_TYPE));
    }

    #[test]
    fn upstream_url_joins_path_and_query() {
        assert_eq!(
            upstream_url("http://127.0.0.1:9", "v1/score", Some("x=1")),
            "http://127.0.0.1:9/v1/score?x=1"
        );
        assert_eq!(
            upstream_url("http://127.0.0.1:9/", "/v1/score", None),
            "http://127.0.0.1:9/v1/score"
        );
    }

    #[test]
    fn only_proof_submit_post_detaches_after_the_body() {
        assert!(is_proof_submit("proof", &Method::POST, "v1/submissions"));
        assert!(is_proof_submit("proof", &Method::POST, "/v1/submissions"));
        assert!(!is_proof_submit("proof", &Method::GET, "v1/submissions"));
        assert!(!is_proof_submit("proof", &Method::POST, "v1/status"));
        assert!(!is_proof_submit("bounty", &Method::POST, "v1/submissions"));
        assert!(!is_proof_submit(
            "proof",
            &Method::POST,
            "v1/submissions/pf"
        ));
    }

    #[test]
    fn admin_paths_blocked_from_gateway() {
        assert!(is_admin_path("v1/admin/rounds/1/winners"));
        assert!(is_admin_path("/v1/admin/rounds/1/candidates"));
        assert!(!is_admin_path("v1/harness"));
        assert!(!is_admin_path("v1/runs/abc"));
    }

    #[test]
    fn report_reads_are_blocked_from_gateway_but_submit_is_not() {
        assert!(is_blocked_report_read(&Method::GET, "v1/reports"));
        assert!(is_blocked_report_read(&Method::HEAD, "v1/reports"));
        assert!(is_blocked_report_read(&Method::HEAD, "v1/reports/by_1"));
        assert!(is_blocked_report_read(&Method::GET, "v1/reports/by_1"));
        assert!(is_blocked_report_read(&Method::GET, "v1/./reports/by_1"));
        assert!(is_blocked_report_read(&Method::OPTIONS, "v1/reports"));
        assert!(!is_blocked_report_read(&Method::POST, "v1/reports"));
        assert!(!is_blocked_report_read(&Method::GET, "v1/status"));
        assert!(!is_blocked_report_read(&Method::HEAD, "v1/status"));
        assert!(!is_blocked_report_read(&Method::GET, "v1/pair"));
    }

    #[test]
    fn admin_paths_blocked_despite_dot_segment_confusion() {
        // axum preserves `./` in `{*rest}`; reqwest then collapses to /v1/admin/…
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
        // The CLI's path, as the gateway sees it (the challenge id is
        // stripped, so this is the challenge-relative form).
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

    /// The forwarded route still needs a bearer: the gateway never holds the
    /// operator token, it only refuses an anonymous call before the hop.
    #[test]
    fn the_operator_bearer_floor_is_presence_only() {
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
        // A raw token (no scheme) is what `admin_ok` accepts too.
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("operator-token"),
        );
        assert!(has_operator_bearer(&headers));
    }

    #[test]
    fn view_paths_detected() {
        assert!(is_view_path("v1/view/abc/index.html"));
        assert!(is_view_path("/v1/view/abc/pricing.html"));
        assert!(is_view_path("v1/./view/abc/index.html"));
        assert!(!is_view_path("v1/runs/abc"));
        assert!(!is_view_path("v1/viewx/abc"));
        assert!(!is_view_path("v1/admin/view"));
        assert!(is_view_png_path("v1/view/abc/index.png"));
        assert!(!is_view_png_path("v1/view/abc/index.html"));
    }

    #[test]
    fn view_lockdown_overwrites_weak_upstream_and_strips_cookie() {
        let mut resp = Response::new(Body::from("<html>miner</html>"));
        let h = resp.headers_mut();
        h.insert(header::SET_COOKIE, HeaderValue::from_static("session=evil"));
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src *"),
        );
        apply_view_lockdown(&mut resp, "'none'", "v1/view/abc/index.html");
        let h = resp.headers();
        assert!(h.get(header::SET_COOKIE).is_none());
        let csp = h
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(csp.starts_with("sandbox;"), "{csp}");
        assert!(!csp.contains("allow-scripts"), "{csp}");
        assert!(!csp.contains("allow-same-origin"), "{csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert_eq!(
            h.get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
    }

    #[test]
    fn png_view_lockdown_allows_cross_origin_img() {
        let mut resp = Response::new(Body::from(vec![0x89_u8, 0x50, 0x4e, 0x47]));
        let h = resp.headers_mut();
        h.insert(header::SET_COOKIE, HeaderValue::from_static("session=evil"));
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("sandbox; default-src 'none'"),
        );
        h.insert(
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("same-origin"),
        );
        apply_view_lockdown(&mut resp, "'none'", "v1/view/abc/index.png");
        let h = resp.headers();
        assert!(h.get(header::SET_COOKIE).is_none());
        assert!(h.get(header::CONTENT_SECURITY_POLICY).is_none());
        assert_eq!(
            h.get("cross-origin-resource-policy")
                .and_then(|v| v.to_str().ok()),
            Some("cross-origin")
        );
    }
}
