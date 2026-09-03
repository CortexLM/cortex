//! Read-only client for the CortexLM/backend public Bounty feed.
//!
//! Cortex never serves `/v1/public/*`. It fetches:
//! `{BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/leaderboard`
//! `{BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/reports`
//!
//! A missing URL, an HTTP failure, and unparseable JSON are all errors. None
//! of them is a skip: this feed *is* the scorer, so a host that cannot read it
//! has nothing to pay miners on and must say so. No host is baked in.
//!
//! The two routes are separate GETs, so "read both once" would let a publish
//! landing between them be signed as one snapshot — mixing revisions. That is
//! not harmless: every tally comes from `/reports`, so a stale half can
//! under-count a miner's valid rows or drop it to `NotAttempted`, while
//! `/leaderboard` sets the champion walk order. The pair is therefore read
//! until two consecutive reads agree, and a feed that never holds still is an
//! error rather than a guess. Consecutive equals are not enough on their own:
//! a host that always serves leaderboard A beside reports B would agree with
//! itself. `valid_count` on each leaderboard row must match the `valid`
//! reports for that hotkey, or the pair is refused.

use bounty_challenge_task::backend_public_url;
use bounty_score::{parse_leaderboard, parse_reports, snapshot_halves_agree, PublicSnapshot};
use thiserror::Error;

/// Fetch / parse errors. Never embed secrets or hosts from env into Display
/// beyond the operator-configured base (trimmed).
#[derive(Debug, Error)]
pub enum BackendError {
    /// `BOUNTY_BACKEND_PUBLIC_URL` unset — this host cannot score.
    #[error("BOUNTY_BACKEND_PUBLIC_URL unset")]
    Unset,
    /// HTTP transport or status.
    #[error("backend public fetch failed")]
    Fetch,
    /// JSON did not match the public DTO.
    #[error("backend public json: {0}")]
    Json(String),
    /// The feed changed between the two route reads every time, so no single
    /// revision could be pinned.
    #[error("backend public feed changed under every read")]
    Inconsistent,
    /// Leaderboard `valid_count` does not match the `valid` reports. A feed
    /// that always serves one revision on `/leaderboard` and another on
    /// `/reports` is stable under re-read and must still be refused.
    #[error("backend public leaderboard and reports do not agree")]
    Mismatched,
}

/// How many times one call re-reads the pair of routes looking for two
/// consecutive reads that agree.
///
/// Two matching reads bracket the in-between read on both routes, so a publish
/// anywhere in that window shows up as a mismatch unless it also reverted.
const MAX_PAIR_READS: usize = 4;

/// Join `{base}/v1/bounty/public/{tail}` without inventing a host.
#[must_use]
pub fn public_path(base: &str, tail: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    format!("{b}/v1/bounty/public/{tail}")
}

/// Load a snapshot from a configured backend URL.
///
/// # Errors
/// [`BackendError::Unset`] when neither the argument nor the env carries a
/// base URL, [`BackendError::Fetch`] on transport or non-2xx,
/// [`BackendError::Json`] when a body does not match the public DTO,
/// [`BackendError::Mismatched`] when the two routes describe different
/// publications, and [`BackendError::Inconsistent`] when the feed never held
/// still long enough to read both routes at one revision.
pub async fn fetch_public_snapshot(base: Option<&str>) -> Result<PublicSnapshot, BackendError> {
    let url = match base {
        Some(u) if !u.trim().is_empty() => u.trim().to_owned(),
        _ => backend_public_url().ok_or(BackendError::Unset)?,
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|_| BackendError::Fetch)?;
    let mut prev = read_pair(&client, &url).await?;
    for _ in 1..MAX_PAIR_READS {
        let next = read_pair(&client, &url).await?;
        if next == prev {
            if snapshot_halves_agree(&next) {
                return Ok(next);
            }
            return Err(BackendError::Mismatched);
        }
        prev = next;
    }
    Err(BackendError::Inconsistent)
}

/// Read both public routes once.
///
/// Comparing the parsed snapshot rather than the raw bodies keeps the
/// consistency check at the granularity scoring actually uses: a field the
/// public DTO does not model (a `generated_at` stamp, say) cannot make a
/// stable feed look like a moving one.
async fn read_pair(client: &reqwest::Client, base: &str) -> Result<PublicSnapshot, BackendError> {
    let lb_text = get_text(client, &public_path(base, "leaderboard")).await?;
    let rp_text = get_text(client, &public_path(base, "reports")).await?;
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
            {"id":"1","hotkey":"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY","status":"valid","severity":"major",
             "problem_found":"seal returns 500","adjudicator":"bounty-adjudicator@cortex",
             "justification":"reproduced empty-bundle seal","adjudicated_at":"2026-08-30T00:00:00Z",
             "created_at":"2026-08-29T00:00:00Z"},
            {"id":"2","hotkey":"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY","status":"valid","severity":"major",
             "problem_found":"proxy 502","adjudicator":"bounty-adjudicator@cortex",
             "justification":"reproduced on master","adjudicated_at":"2026-08-30T00:00:00Z",
             "created_at":"2026-08-29T01:00:00Z"},
            {"id":"3","hotkey":"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY","status":"valid","severity":"major",
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

    /// An unset feed is a refusal, not a quiet skip. A skip here would let a
    /// host that reads nothing keep serving ingest and emitting leaves.
    #[tokio::test]
    async fn an_unset_url_is_an_error_not_a_skip() {
        if backend_public_url().is_some() {
            eprintln!("skip unset assertion: BOUNTY_BACKEND_PUBLIC_URL is set");
            return;
        }
        let err = fetch_public_snapshot(None).await.expect_err("unset");
        assert!(matches!(err, BackendError::Unset), "{err}");
        let blank = fetch_public_snapshot(Some("   ")).await.expect_err("blank");
        assert!(matches!(blank, BackendError::Unset), "{blank}");
    }

    /// A closed port is the shape of a backend outage. It must surface as a
    /// fetch error so the caller emits nothing.
    #[tokio::test]
    async fn an_unreachable_backend_is_a_fetch_error() {
        let err = fetch_public_snapshot(Some("http://127.0.0.1:1"))
            .await
            .expect_err("unreachable");
        assert!(matches!(err, BackendError::Fetch), "{err}");
    }
}
