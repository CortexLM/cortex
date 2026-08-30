//! Bounty orchestrator helpers: D24 leaf plan + crate re-exports.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};

use bounty_challenge_task::{CHALLENGE_ID_BYTES, SCORE_MAX};
use bounty_score::{champion_hold_lattice, lattice_from_precision, MinerHoldout};
use bounty_store::MemoryStore;
use bundle::{NoScoreReasonCode, ScoreOrAbsence};
use challenge_common::{emit_signed_leaf_set, Hotkey, LeafEmitError};

pub use bounty_challenge_task::{
    chat_command_display, CHALLENGE_ID, CHALLENGE_ID_BYTES as BOUNTY_ID_BYTES,
    CHAT_COMMAND_PLACEHOLDER, SCORE_MAX as BOUNTY_SCORE_MAX, SCORING_VERSION, TERMS_TEXT,
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
    if let Some(p) = h.precision_bps() {
        if h.decided() > 0 && h.net_credit() >= 0 {
            return lattice_from_precision(p).max(champion_hold_lattice() / 4);
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
                valid: 0,
                already_fixed: 0,
                malicious: 3,
                duplicate: 0,
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
}
