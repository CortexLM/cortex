//! Path predicates the proxy decides on before it dials an upstream.
//!
//! These are the gateway's **read** gates: which paths are operator-local
//! (report bodies, and the admin surface in [`crate::admin_route`]), and
//! which carry miner-controlled HTML the gateway must lock down before it
//! forwards a response.
//!
//! They live here rather than in `gateway::proxy` because that file is at the
//! repository's per-crate LOC cap, and because every one of them is a
//! decision made **before** a request leaves the host — the kind of rule that
//! deserves its own tests rather than being folded in with round-robin and
//! ejection.

use axum::http::Method;

use crate::proxy_detach::normalize_proxy_path;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_reads_are_blocked_but_submit_is_not() {
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
}
