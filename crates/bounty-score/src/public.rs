//! Consumer types for the CortexLM/backend public Bounty feed.
//!
//! Cortex **reads** these payloads. It does not serve a public API.
//! Only `hotkey` + `problem_found` + `justification` + status counts
//! enter scoring. Account ids, sessions, and Chat logs are not in this DTO.

use std::collections::BTreeMap;

use bounty_challenge_task::{hotkey_hex, parse_hotkey};
use serde::{Deserialize, Serialize};

use crate::{
    judge_challenger, lattice_from_precision, Adjudication, ChampionVerdict, MinerHoldout,
};

/// Published report as served by the backend public API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicReport {
    /// Backend report id.
    pub id: String,
    /// Miner hotkey (SS58). On-chain public.
    pub hotkey: String,
    /// Adjudication status. Pending items are ignored for scoring.
    pub status: PublicStatus,
    /// Short public statement of the bug (`Problème trouvé`).
    pub problem_found: String,
    /// Agent/service id that decided (not a human email).
    pub adjudicator: String,
    /// Why the agent accepted or rejected. Required for scoring.
    pub justification: String,
    /// RFC3339 adjudicate time.
    pub adjudicated_at: String,
    /// RFC3339 create time.
    pub created_at: String,
    /// Original report when `status` is duplicate.
    #[serde(default)]
    pub related_report_id: Option<String>,
}

/// Backend leaderboard row (valid-count ranking).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderboardRow {
    /// Miner hotkey (SS58).
    pub hotkey: String,
    /// Count of valid published reports.
    pub valid_count: u64,
    /// Optional lattice / weight hint from backend.
    #[serde(default)]
    pub weight: Option<u64>,
}

/// Snapshot fetched from backend (`/v1/bounty/public/*`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicSnapshot {
    /// Leaderboard rows.
    #[serde(default)]
    pub leaderboard: Vec<LeaderboardRow>,
    /// Published reports.
    #[serde(default)]
    pub reports: Vec<PublicReport>,
}

/// Wire status on the backend public feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicStatus {
    /// Unique reproducing bug.
    Valid,
    /// Duplicate of an open report.
    Duplicate,
    /// Already fixed, not in prod.
    AlreadyFixedNotProd,
    /// Fabricated / malicious.
    InvalidMalicious,
    /// Not yet published — ignored for scoring.
    Pending,
}

impl PublicStatus {
    fn adjudication(self) -> Option<Adjudication> {
        match self {
            Self::Valid => Some(Adjudication::Valid),
            Self::Duplicate => Some(Adjudication::Duplicate),
            Self::AlreadyFixedNotProd => Some(Adjudication::AlreadyFixedNotProd),
            Self::InvalidMalicious => Some(Adjudication::InvalidMalicious),
            Self::Pending => None,
        }
    }
}

/// True when the report may enter scoring (published + justified).
#[must_use]
pub fn scorable(report: &PublicReport) -> bool {
    report.status.adjudication().is_some()
        && !report.problem_found.trim().is_empty()
        && !report.justification.trim().is_empty()
}

/// Parse a leaderboard list (`{ "items": [...] }` or a bare array).
pub fn parse_leaderboard(raw: &str) -> Result<Vec<LeaderboardRow>, String> {
    parse_items(raw)
}

/// Parse a reports list (`{ "items": [...] }` or a bare array).
pub fn parse_reports(raw: &str) -> Result<Vec<PublicReport>, String> {
    parse_items(raw)
}

#[derive(Deserialize)]
struct ItemsWrap<T> {
    items: Vec<T>,
}

fn parse_items<T: for<'de> Deserialize<'de>>(raw: &str) -> Result<Vec<T>, String> {
    if let Ok(v) = serde_json::from_str::<Vec<T>>(raw) {
        return Ok(v);
    }
    serde_json::from_str::<ItemsWrap<T>>(raw)
        .map(|w| w.items)
        .map_err(|e| format!("backend public json: {e}"))
}

/// Per-hotkey holdout from published, justified reports only.
#[must_use]
pub fn holdouts_from_reports(reports: &[PublicReport]) -> BTreeMap<String, MinerHoldout> {
    let mut out: BTreeMap<String, MinerHoldout> = BTreeMap::new();
    for r in reports {
        if !scorable(r) {
            continue;
        }
        let Some(v) = r.status.adjudication() else {
            continue;
        };
        let Ok(bytes) = parse_hotkey(&r.hotkey) else {
            continue;
        };
        let key = hotkey_hex(&bytes);
        out.entry(key).or_default().record(v);
    }
    out
}

/// Rank leaderboard by `valid_count` descending (stable on hotkey).
#[must_use]
pub fn rank_leaderboard(rows: &[LeaderboardRow]) -> Vec<LeaderboardRow> {
    let mut v = rows.to_vec();
    v.sort_by(|a, b| {
        b.valid_count
            .cmp(&a.valid_count)
            .then_with(|| a.hotkey.cmp(&b.hotkey))
    });
    v
}

/// Score plan from a backend snapshot: precision holdouts + champion lattice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicScorePlan {
    /// Hex hotkey → holdout tallies.
    pub holdouts: BTreeMap<String, MinerHoldout>,
    /// Champion hex hotkey, if any.
    pub champion_hex: Option<String>,
    /// Lattice for the champion.
    pub champion_lattice: u64,
    /// Displacement verdict that crowned the champion.
    pub verdict: Option<ChampionVerdict>,
}

/// Build a score plan from backend public JSON objects.
///
/// Reports missing `problem_found` or `justification` are dropped.
/// Champion is the highest-precision miner that displaces the previous
/// (or the first eligible). Leaderboard `valid_count` breaks ties.
#[must_use]
pub fn score_plan_from_snapshot(snap: &PublicSnapshot) -> PublicScorePlan {
    let holdouts = holdouts_from_reports(&snap.reports);
    let ranked = rank_leaderboard(&snap.leaderboard);
    let mut order: Vec<String> = Vec::new();
    for row in &ranked {
        if let Ok(b) = parse_hotkey(&row.hotkey) {
            let h = hotkey_hex(&b);
            if holdouts.contains_key(&h) && !order.contains(&h) {
                order.push(h);
            }
        }
    }
    for k in holdouts.keys() {
        if !order.contains(k) {
            order.push(k.clone());
        }
    }

    let mut champion_hex = None;
    let mut champion_lattice = 0;
    let mut verdict = None;
    let mut champ_hold = MinerHoldout::default();
    for h in &order {
        let Some(chall) = holdouts.get(h) else {
            continue;
        };
        let v = judge_challenger(&champ_hold, chall);
        if v.eligible {
            champion_hex = Some(h.clone());
            champion_lattice = v.lattice;
            if champion_lattice == 0 {
                if let Some(p) = chall.precision_bps() {
                    champion_lattice = lattice_from_precision(p);
                }
            }
            champ_hold = chall.clone();
            verdict = Some(v);
        }
    }

    PublicScorePlan {
        holdouts,
        champion_hex,
        champion_lattice,
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HK_A: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    const HK_B: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";

    fn report(id: &str, hk: &str, status: PublicStatus, problem: &str, why: &str) -> PublicReport {
        PublicReport {
            id: id.into(),
            hotkey: hk.into(),
            status,
            problem_found: problem.into(),
            adjudicator: "bounty-adjudicator@cortex".into(),
            justification: why.into(),
            adjudicated_at: "2026-08-30T00:00:00Z".into(),
            created_at: "2026-08-29T00:00:00Z".into(),
            related_report_id: None,
        }
    }

    #[test]
    fn missing_justification_or_problem_is_not_scored() {
        let reports = vec![
            report("1", HK_A, PublicStatus::Valid, "seal 500", "reproduced"),
            report("2", HK_A, PublicStatus::Valid, "", "reproduced"),
            report("3", HK_A, PublicStatus::Valid, "other", ""),
            report("4", HK_A, PublicStatus::Pending, "wip", "n/a"),
        ];
        let h = holdouts_from_reports(&reports);
        assert_eq!(h.len(), 1);
        let only = h.values().next().expect("one");
        assert_eq!(only.valid, 1);
    }

    #[test]
    fn scoring_uses_hotkey_counts() {
        let reports = vec![
            report(
                "1",
                HK_A,
                PublicStatus::Valid,
                "bug a",
                "reproduced on master",
            ),
            report(
                "2",
                HK_A,
                PublicStatus::Valid,
                "bug b",
                "reproduced on master",
            ),
            report(
                "3",
                HK_A,
                PublicStatus::Valid,
                "bug c",
                "reproduced on master",
            ),
            report(
                "4",
                HK_B,
                PublicStatus::InvalidMalicious,
                "invented",
                "does not exist",
            ),
            report(
                "5",
                HK_B,
                PublicStatus::InvalidMalicious,
                "invented 2",
                "does not exist",
            ),
            report(
                "6",
                HK_B,
                PublicStatus::InvalidMalicious,
                "invented 3",
                "does not exist",
            ),
        ];
        let plan = score_plan_from_snapshot(&PublicSnapshot {
            leaderboard: vec![
                LeaderboardRow {
                    hotkey: HK_A.into(),
                    valid_count: 3,
                    weight: None,
                },
                LeaderboardRow {
                    hotkey: HK_B.into(),
                    valid_count: 0,
                    weight: None,
                },
            ],
            reports,
        });
        assert!(plan.champion_hex.is_some());
        let champ = plan.champion_hex.as_ref().expect("champ");
        let a_hex = hotkey_hex(&parse_hotkey(HK_A).expect("a"));
        assert_eq!(champ, &a_hex);
        assert!(plan.champion_lattice > 0);
        assert_eq!(plan.holdouts.get(&a_hex).map(|h| h.valid), Some(3));
    }

    #[test]
    fn leaderboard_sorts_by_valid_count() {
        let ranked = rank_leaderboard(&[
            LeaderboardRow {
                hotkey: "b".into(),
                valid_count: 1,
                weight: None,
            },
            LeaderboardRow {
                hotkey: "a".into(),
                valid_count: 9,
                weight: Some(1),
            },
            LeaderboardRow {
                hotkey: "c".into(),
                valid_count: 9,
                weight: None,
            },
        ]);
        assert_eq!(ranked[0].hotkey, "a");
        assert_eq!(ranked[1].hotkey, "c");
        assert_eq!(ranked[2].valid_count, 1);
    }

    #[test]
    fn parse_wrapped_items() {
        let raw = r#"{"items":[{"hotkey":"a","valid_count":2}]}"#;
        let rows = parse_leaderboard(raw).expect("parse");
        assert_eq!(rows[0].valid_count, 2);
    }
}
