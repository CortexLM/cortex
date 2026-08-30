//! Read-only client for the CortexLM/backend public Bounty feed.
//!
//! Cortex never serves `/v1/public/*`. It fetches:
//! `{BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/leaderboard`
//! `{BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/reports`
//!
//! Missing URL → skip / sim. No host is baked in.

use bounty_challenge_task::backend_public_url;
use bounty_score::{parse_leaderboard, parse_reports, PublicSnapshot};
use thiserror::Error;

/// Fetch / parse errors. Never embed secrets or hosts from env into Display
/// beyond the operator-configured base (trimmed).
#[derive(Debug, Error)]
pub enum BackendError {
    /// `BOUNTY_BACKEND_PUBLIC_URL` unset — caller should skip / sim.
    #[error("BOUNTY_BACKEND_PUBLIC_URL unset")]
    Unset,
    /// HTTP transport or status.
    #[error("backend public fetch failed")]
    Fetch,
    /// JSON did not match the public DTO.
    #[error("backend public json: {0}")]
    Json(String),
}

/// Join `{base}/v1/bounty/public/{tail}` without inventing a host.
#[must_use]
pub fn public_path(base: &str, tail: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    format!("{b}/v1/bounty/public/{tail}")
}

/// Load a snapshot from a configured backend URL.
///
/// # Errors
/// [`BackendError::Unset`] when the env is empty (CI-safe skip).
pub async fn fetch_public_snapshot(base: Option<&str>) -> Result<PublicSnapshot, BackendError> {
    let url = match base {
        Some(u) if !u.trim().is_empty() => u.trim().to_owned(),
        _ => backend_public_url().ok_or(BackendError::Unset)?,
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|_| BackendError::Fetch)?;
    let lb_text = get_text(&client, &public_path(&url, "leaderboard")).await?;
    let rp_text = get_text(&client, &public_path(&url, "reports")).await?;
    let leaderboard = parse_leaderboard(&lb_text).map_err(BackendError::Json)?;
    let reports = parse_reports(&rp_text).map_err(BackendError::Json)?;
    Ok(PublicSnapshot {
        leaderboard,
        reports,
    })
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String, BackendError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|_| BackendError::Fetch)?;
    if !resp.status().is_success() {
        return Err(BackendError::Fetch);
    }
    resp.text().await.map_err(|_| BackendError::Fetch)
}

/// Parse a snapshot from two JSON bodies (unit tests / mocks).
pub fn snapshot_from_json(
    leaderboard: &str,
    reports: &str,
) -> Result<PublicSnapshot, BackendError> {
    Ok(PublicSnapshot {
        leaderboard: parse_leaderboard(leaderboard).map_err(BackendError::Json)?,
        reports: parse_reports(reports).map_err(BackendError::Json)?,
    })
}

/// Fetch when `BOUNTY_BACKEND_PUBLIC_URL` is set; `Ok(None)` when unset (CI skip).
pub async fn try_fetch_public_snapshot() -> Result<Option<PublicSnapshot>, BackendError> {
    match fetch_public_snapshot(None).await {
        Ok(s) => Ok(Some(s)),
        Err(BackendError::Unset) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bounty_score::score_plan_from_snapshot;

    #[test]
    fn public_path_uses_operator_base_only() {
        assert_eq!(
            public_path("http://127.0.0.1:9", "leaderboard"),
            "http://127.0.0.1:9/v1/bounty/public/leaderboard"
        );
        assert_eq!(
            public_path("http://127.0.0.1:9/", "reports"),
            "http://127.0.0.1:9/v1/bounty/public/reports"
        );
    }

    #[test]
    fn mock_backend_json_scores_hotkeys() {
        let lb = r#"{"items":[
            {"hotkey":"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY","valid_count":3},
            {"hotkey":"5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty","valid_count":0}
        ]}"#;
        let rp = r#"{"items":[
            {"id":"1","hotkey":"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY","status":"valid",
             "problem_found":"seal returns 500","adjudicator":"bounty-adjudicator@cortex",
             "justification":"reproduced empty-bundle seal","adjudicated_at":"2026-08-30T00:00:00Z",
             "created_at":"2026-08-29T00:00:00Z"},
            {"id":"2","hotkey":"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY","status":"valid",
             "problem_found":"proxy 502","adjudicator":"bounty-adjudicator@cortex",
             "justification":"reproduced on master","adjudicated_at":"2026-08-30T00:00:00Z",
             "created_at":"2026-08-29T01:00:00Z"},
            {"id":"3","hotkey":"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY","status":"valid",
             "problem_found":"health flap","adjudicator":"bounty-adjudicator@cortex",
             "justification":"reproduced","adjudicated_at":"2026-08-30T00:00:00Z",
             "created_at":"2026-08-29T02:00:00Z"}
        ]}"#;
        let snap = snapshot_from_json(lb, rp).expect("mock");
        let plan = score_plan_from_snapshot(&snap);
        assert!(plan.champion_hex.is_some());
        assert!(plan.champion_lattice > 0);
        assert_eq!(plan.holdouts.len(), 1);
    }

    #[tokio::test]
    async fn fetch_skips_when_url_unset() {
        if backend_public_url().is_some() {
            eprintln!("skip unset assertion: BOUNTY_BACKEND_PUBLIC_URL is set");
            return;
        }
        let err = fetch_public_snapshot(None).await.expect_err("unset");
        assert!(matches!(err, BackendError::Unset));
        let skip = try_fetch_public_snapshot().await.expect("skip");
        assert!(skip.is_none());
    }
}
