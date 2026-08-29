//! Relearn orchestrator helpers: D24 leaf plan + crate re-exports.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};

use bundle::{NoScoreReasonCode, ScoreOrAbsence};
use challenge_common::{emit_signed_leaf_set, Hotkey, LeafEmitError};
use relearn_challenge_task::{CHALLENGE_ID_BYTES, SCORE_MAX};
use relearn_score::champion_hold_lattice;
use relearn_store::SubmissionState;

pub use relearn_challenge_task::{
    BASE_MODEL_ID, CHALLENGE_ID, CHALLENGE_ID_BYTES as RELEARN_ID_BYTES,
    SCORE_MAX as RELEARN_SCORE_MAX, SCORING_VERSION, TEACHER_MODEL_ID,
};
pub use relearn_eval::{resolve_teacher_backend, RelearnPin};
pub use relearn_http::{hash_admin_token, relearn_router, AppState};
pub use relearn_store::MemoryStore;

/// Build a D24-complete score map: champion (if any) gets a positive lattice;
/// everyone else is explicit `NoScore` (never silent).
pub fn emission_scores(
    expected: &BTreeSet<Hotkey>,
    champion_hotkey: Option<Hotkey>,
    champion_lattice: u64,
) -> BTreeMap<Hotkey, ScoreOrAbsence> {
    expected
        .iter()
        .map(|h| {
            let s = match champion_hotkey {
                Some(c) if c == *h && champion_lattice > 0 => ScoreOrAbsence::Score {
                    value: champion_lattice.min(SCORE_MAX),
                },
                _ => ScoreOrAbsence::NoScore {
                    reason: NoScoreReasonCode::NotAttempted,
                },
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
) -> Result<BTreeMap<Hotkey, bundle::LeafV1>, LeafEmitError> {
    let scores = emission_scores(expected, champion_hotkey, champion_lattice);
    emit_signed_leaf_set(secret, CHALLENGE_ID_BYTES, epoch, expected, &scores)
}

/// Lattice for the current store champion, or the hold value when only
/// the base model is live (burn is wrong: the factory still has a champ).
pub fn live_champion_lattice(store: &MemoryStore) -> u64 {
    if let Ok(Some(id)) = store.champion_id() {
        if let Ok(row) = store.get(&id) {
            if row.state == SubmissionState::Champion {
                if let Some(v) = row.verdict {
                    if v.eligible && v.lattice > 0 {
                        return v.lattice;
                    }
                }
                return champion_hold_lattice();
            }
        }
    }
    0
}

/// Parse a 64-hex hotkey.
pub fn parse_hotkey(hex_s: &str) -> Option<Hotkey> {
    let t = hex_s.trim().trim_start_matches("0x");
    let bytes = hex::decode(t).ok()?;
    <[u8; 32]>::try_from(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use challenge_common::public_key_from_secret;
    use crypto::KEY_LEN;

    fn sk() -> [u8; KEY_LEN] {
        let mut s = [7u8; KEY_LEN];
        s[0] = 1;
        s
    }

    #[test]
    fn d24_covers_every_hotkey() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let mut e = BTreeSet::new();
        e.insert(a);
        e.insert(b);
        let leaves = emit_epoch(&sk(), 9, &e, Some(a), 12_000).expect("emit");
        assert_eq!(leaves.len(), 2);
        assert!(matches!(
            leaves[&a].score_or_absence,
            ScoreOrAbsence::Score { value: 12_000 }
        ));
        assert!(matches!(
            leaves[&b].score_or_absence,
            ScoreOrAbsence::NoScore { .. }
        ));
        let pk = public_key_from_secret(&sk()).expect("pk");
        for leaf in leaves.values() {
            challenge_common::verify_leaf_sig(leaf, &pk).expect("sig");
        }
    }

    #[test]
    fn never_emits_score_on_empty_expected() {
        let e = BTreeSet::new();
        let leaves = emit_epoch(&sk(), 1, &e, None, 0).expect("empty E");
        assert!(leaves.is_empty());
    }
}
