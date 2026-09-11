//! Bounded disconnect-survive hop for Proof submit only.
//!
//! Extracted from `gateway` so that crate stays under the LOC cap. Public
//! challenge GETs and non-Proof POSTs stay on the request future (cancel on
//! hang-up). Proof `POST /v1/submissions` is spawned with a hard admission
//! cap and a 7200 s deadline matching miner `ctx`.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};

/// Detached Proof submit hop: matches miner `ctx` POST wait (7200 s).
pub const PROOF_PROXY_DEADLINE_SECS: u64 = 7200;
const MAX_DETACHED_PROOF: usize = 8;
static DETACHED_PROOF: AtomicUsize = AtomicUsize::new(0);

/// Collapse `.` / empty / `..` segments the same way `url`/`reqwest` will.
#[must_use]
pub fn normalize_proxy_path(rest: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                let _ = out.pop();
            }
            other => out.push(other),
        }
    }
    out.join("/")
}

/// Whether this public hop may outlive the caller (Proof live submit only).
#[must_use]
pub fn is_proof_submit(challenge_id: &str, method: &Method, rest: &str) -> bool {
    challenge_id == "proof"
        && *method == Method::POST
        && normalize_proxy_path(rest) == "v1/submissions"
}

struct DetachPermit;
impl Drop for DetachPermit {
    fn drop(&mut self) {
        DETACHED_PROOF.fetch_sub(1, Ordering::SeqCst);
    }
}

fn admit_proof() -> Option<DetachPermit> {
    if DETACHED_PROOF.fetch_add(1, Ordering::SeqCst) < MAX_DETACHED_PROOF {
        Some(DetachPermit)
    } else {
        DETACHED_PROOF.fetch_sub(1, Ordering::SeqCst);
        None
    }
}

/// Spawn `work` so a miner hang-up does not abort Proof evaluate.
pub async fn detach_proof(work: impl Future<Output = Response> + Send + 'static) -> Response {
    let Some(permit) = admit_proof() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "proof proxy at capacity").into_response();
    };
    match tokio::spawn(async move {
        let _permit = permit;
        match tokio::time::timeout(Duration::from_secs(PROOF_PROXY_DEADLINE_SECS), work).await {
            Ok(r) => r,
            Err(_) => (StatusCode::GATEWAY_TIMEOUT, "upstream deadline").into_response(),
        }
    })
    .await
    {
        Ok(r) => r,
        Err(_) => (StatusCode::BAD_GATEWAY, "proxy task failed").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_proof_submit_survives_disconnect() {
        assert_eq!(PROOF_PROXY_DEADLINE_SECS, 7200);
        assert!(is_proof_submit("proof", &Method::POST, "v1/submissions"));
        assert!(is_proof_submit("proof", &Method::POST, "/v1/submissions"));
        assert!(is_proof_submit("proof", &Method::POST, "v1/./submissions"));
        assert!(!is_proof_submit("proof", &Method::GET, "v1/submissions"));
        assert!(!is_proof_submit("proof", &Method::POST, "v1/status"));
        assert!(!is_proof_submit("bounty", &Method::POST, "v1/submissions"));
        assert!(!is_proof_submit("proof", &Method::POST, "v1/reports"));
        assert!(!is_proof_submit(
            "proof",
            &Method::POST,
            "v1/submissions/pf"
        ));
    }

    #[tokio::test]
    async fn ninth_detached_proof_is_503() {
        let hold = std::sync::Arc::new(tokio::sync::Notify::new());
        let mut joins = Vec::new();
        for _ in 0..MAX_DETACHED_PROOF {
            let hold = hold.clone();
            joins.push(tokio::spawn(async move {
                detach_proof(async move {
                    hold.notified().await;
                    StatusCode::OK.into_response()
                })
                .await
            }));
        }
        let started = tokio::time::Instant::now();
        loop {
            if DETACHED_PROOF.load(Ordering::SeqCst) >= MAX_DETACHED_PROOF {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(2));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let ninth = detach_proof(async { StatusCode::OK.into_response() }).await;
        assert_eq!(ninth.status(), StatusCode::SERVICE_UNAVAILABLE);
        hold.notify_waiters();
        for j in joins {
            let _ = j.await;
        }
    }
}
