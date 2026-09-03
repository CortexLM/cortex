//! Bounty orchestrator helpers: D24 leaf plan + crate re-exports.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};

mod backend;

use bounty_challenge_task::{hotkey_hex, CHALLENGE_ID_BYTES, SCORE_MAX};
use bounty_score::{
    champion_hold_lattice, lattice_from_precision_and_impact, score_plan_from_snapshot,
    MinerHoldout, PublicScorePlan, PublicSnapshot,
};
use bounty_store::MemoryStore;
use bundle::{NoScoreReasonCode, ScoreOrAbsence};
use challenge_common::{emit_signed_leaf_set, Hotkey, LeafEmitError};

pub use backend::{
    fetch_public_snapshot, public_path, snapshot_from_json, try_fetch_public_snapshot, BackendError,
};
pub use bounty_challenge_task::{
    backend_public_url, chat_command_display, force_sim, resolve_scoring_backend, ScoringBackend,
    CHALLENGE_ID, CHALLENGE_ID_BYTES as BOUNTY_ID_BYTES, CHAT_COMMAND_PLACEHOLDER,
    SCORE_MAX as BOUNTY_SCORE_MAX, SCORING_VERSION, TERMS_TEXT,
};
pub use bounty_http::{bounty_router, hash_admin_token, AppState};
pub use bounty_store::MemoryStore as BountyStore;

/// Build a D24-complete score map.
///
/// Champion (if any) gets a positive lattice from holdout precision.
/// Miners with a net malicious penalty get `InvalidResponse` (burn toward uid 0).
/// Everyone else is explicit `NoScore` (unmatched emission burns to uid 0).
pub fn emission_scores(
    expected: &BTreeSet<Hotkey>,
    champion_hotkey: Option<Hotkey>,
    champion_lattice: u64,
    holdouts: &BTreeMap<Hotkey, MinerHoldout>,
) -> BTreeMap<Hotkey, ScoreOrAbsence> {
    expected
        .iter()
        .map(|h| {
            let s = match champion_hotkey {
                Some(c) if c == *h && champion_lattice > 0 => ScoreOrAbsence::Score {
                    value: champion_lattice.min(SCORE_MAX),
                },
                _ => {
                    if holdouts.get(h).is_some_and(|r| r.net_credit() < 0) {
                        ScoreOrAbsence::NoScore {
                            reason: NoScoreReasonCode::InvalidResponse,
                        }
                    } else {
                        ScoreOrAbsence::NoScore {
                            reason: NoScoreReasonCode::NotAttempted,
                        }
                    }
                }
            };
            (*h, s)
        })
        .collect()
}

/// Sign the exact-E leaf set for this epoch.
pub fn emit_epoch(
    secret: &[u8; 32],
    epoch: u64,
    expected: &BTreeSet<Hotkey>,
    champion_hotkey: Option<Hotkey>,
    champion_lattice: u64,
    holdouts: &BTreeMap<Hotkey, MinerHoldout>,
) -> Result<BTreeMap<Hotkey, bundle::LeafV1>, LeafEmitError> {
    let scores = emission_scores(expected, champion_hotkey, champion_lattice, holdouts);
    emit_signed_leaf_set(secret, CHALLENGE_ID_BYTES, epoch, expected, &scores)
}

/// Lattice for the current store champion.
pub fn live_champion_lattice(store: &MemoryStore, champion_hotkey: Option<&str>) -> u64 {
    let Some(hk) = champion_hotkey else {
        return 0;
    };
    let Ok(h) = store.holdout(hk) else {
        return 0;
    };
    if let (Some(p), Some(impact)) = (h.precision_bps(), h.impact_bps()) {
        if h.decided() > 0 && h.net_credit() >= 0 {
            return lattice_from_precision_and_impact(p, impact).max(champion_hold_lattice() / 4);
        }
    }
    champion_hold_lattice()
}

/// Parse a 64-hex hotkey.
pub fn parse_hotkey_hex(hex_s: &str) -> Option<Hotkey> {
    let t = hex_s.trim().trim_start_matches("0x");
    let bytes = hex::decode(t).ok()?;
    <[u8; 32]>::try_from(bytes).ok()
}

/// Map a backend public snapshot onto D24 scores for `expected`.
#[must_use]
pub fn emission_from_public_snapshot(
    expected: &BTreeSet<Hotkey>,
    snap: &PublicSnapshot,
) -> (PublicScorePlan, BTreeMap<Hotkey, ScoreOrAbsence>) {
    let plan = score_plan_from_snapshot(snap);
    let champ = plan.champion_hex.as_deref().and_then(parse_hotkey_hex);
    let mut holdouts = BTreeMap::new();
    for h in expected {
        let hex_s = hotkey_hex(h);
        if let Some(row) = plan.holdouts.get(&hex_s) {
            holdouts.insert(*h, row.clone());
        }
    }
    let scores = emission_scores(expected, champ, plan.champion_lattice, &holdouts);
    (plan, scores)
}

#[cfg(test)]
mod tests {
    use super::*;
    use challenge_common::public_key_from_secret;

    fn sk() -> [u8; 32] {
        let mut s = [9u8; 32];
        s[0] = 2;
        s
    }

    #[test]
    fn d24_covers_every_hotkey() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let mut e = BTreeSet::new();
        e.insert(a);
        e.insert(b);
        let mut holdouts = BTreeMap::new();
        holdouts.insert(
            b,
            MinerHoldout {
                malicious: 3,
                ..MinerHoldout::default()
            },
        );
        let leaves = emit_epoch(&sk(), 9, &e, Some(a), 12_000, &holdouts).expect("emit");
        assert_eq!(leaves.len(), 2);
        assert!(matches!(
            leaves[&a].score_or_absence,
            ScoreOrAbsence::Score { value: 12_000 }
        ));
        assert!(matches!(
            leaves[&b].score_or_absence,
            ScoreOrAbsence::NoScore {
                reason: NoScoreReasonCode::InvalidResponse
            }
        ));
        let pk = public_key_from_secret(&sk()).expect("pk");
        for leaf in leaves.values() {
            challenge_common::verify_leaf_sig(leaf, &pk).expect("sig");
        }
    }

    #[test]
    fn unmatched_is_not_attempted() {
        let a = [1u8; 32];
        let mut e = BTreeSet::new();
        e.insert(a);
        let leaves = emit_epoch(&sk(), 1, &e, None, 0, &BTreeMap::new()).expect("emit");
        assert!(matches!(
            leaves[&a].score_or_absence,
            ScoreOrAbsence::NoScore {
                reason: NoScoreReasonCode::NotAttempted
            }
        ));
    }

    #[test]
    fn emission_maps_backend_public_snapshot() {
        let alice = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let snap = snapshot_from_json(
            &format!(r#"{{"items":[{{"hotkey":"{alice}","valid_count":3}}]}}"#),
            &format!(
                r#"{{"items":[
                {{"id":"1","hotkey":"{alice}","status":"valid","severity":"major",
                  "problem_found":"seal 500","adjudicator":"bounty-adjudicator@cortex",
                  "justification":"reproduced","adjudicated_at":"2026-08-30T00:00:00Z",
                  "created_at":"2026-08-29T00:00:00Z"}},
                {{"id":"2","hotkey":"{alice}","status":"valid","severity":"major",
                  "problem_found":"proxy 502","adjudicator":"bounty-adjudicator@cortex",
                  "justification":"reproduced","adjudicated_at":"2026-08-30T00:00:00Z",
                  "created_at":"2026-08-29T01:00:00Z"}},
                {{"id":"3","hotkey":"{alice}","status":"valid","severity":"major",
                  "problem_found":"health flap","adjudicator":"bounty-adjudicator@cortex",
                  "justification":"reproduced","adjudicated_at":"2026-08-30T00:00:00Z",
                  "created_at":"2026-08-29T02:00:00Z"}}
            ]}}"#
            ),
        )
        .expect("mock");
        let hk = parse_hotkey_hex(&hotkey_hex(
            &bounty_challenge_task::parse_hotkey(alice).expect("alice"),
        ))
        .expect("hex");
        let mut expected = BTreeSet::new();
        expected.insert(hk);
        let (plan, scores) = emission_from_public_snapshot(&expected, &snap);
        assert!(plan.champion_hex.is_some());
        assert!(matches!(
            scores[&hk],
            ScoreOrAbsence::Score { value } if value > 0
        ));
    }
}
