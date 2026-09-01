//! Relearn Agent orchestrator helpers: D24 leaf plan + crate re-exports.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};

use bundle::{NoScoreReasonCode, ScoreOrAbsence};
use challenge_common::{emit_signed_leaf_set, Hotkey, LeafEmitError};
use relearn_agent_score::champion_hold_lattice;
use relearn_agent_store::SubmissionState;
use relearn_agent_task::{CHALLENGE_ID_BYTES, SCORE_MAX};

pub use relearn_agent_eval::{
    boot_base_champion, force_sim, resolve_eval_backend, scoring_readiness, BaselineMeasurement,
    EvalBackend, LiveScorer, BASE_CHAMPION_RUN,
};
pub use relearn_agent_http::{hash_admin_token, relearn_agent_router, AppState};
pub use relearn_agent_store::{ArtifactManifest, MemoryStore};
pub use relearn_agent_task::{
    episode_commitment, AgentEpisode, RelearnAgentPin, ToolKind, BASE_MODEL_ID, CHALLENGE_ID,
    CHALLENGE_ID_BYTES as RELEARN_AGENT_ID_BYTES, SCORE_MAX as RELEARN_AGENT_SCORE_MAX,
    SCORING_VERSION,
};

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

/// Lattice for the current store champion, or the hold value when only the
/// measured base checkpoint is live.
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

/// Load frozen episodes from an operator JSON file body.
///
/// The file is a JSON array of episode objects. It is verified against the
/// pin's commitment by [`MemoryStore::load_episodes`], so a wrong or edited
/// file scores nothing instead of falling back to the published split.
pub fn parse_episode_file(body: &str) -> Result<Vec<AgentEpisode>, String> {
    serde_json::from_str(body).map_err(|e| format!("parse episodes: {e}"))
}

#[cfg(test)]
mod tests {
    use challenge_common::public_key_from_secret;
    use crypto::KEY_LEN;

    use super::*;

    fn sk() -> [u8; KEY_LEN] {
        let mut s = [9u8; KEY_LEN];
        s[0] = 5;
        s
    }

    #[test]
    fn d24_covers_every_hotkey() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let e: BTreeSet<Hotkey> = [a, b].into_iter().collect();
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
    fn leaf_domain_is_not_another_live_challenge() {
        assert_eq!(RELEARN_AGENT_ID_BYTES, b"relearn-agent");
        for other in [&b"relearn"[..], b"relearn-image", b"bounty"] {
            assert_ne!(RELEARN_AGENT_ID_BYTES, other);
        }
    }

    #[test]
    fn never_emits_score_on_empty_expected() {
        let leaves = emit_epoch(&sk(), 1, &BTreeSet::new(), None, 0).expect("empty E");
        assert!(leaves.is_empty());
    }

    #[test]
    fn no_champion_means_zero_lattice() {
        assert_eq!(live_champion_lattice(&MemoryStore::new()), 0);
    }

    #[test]
    fn episode_file_parses_records() {
        let body = r#"[{"id":900,"goal":"find the total","environment_id":"dev",
            "tools":["inspect"],"observation_hash":"aa","answer_hash":"bb","min_tool_calls":2}]"#;
        let recs = parse_episode_file(body).expect("parse");
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].tools, vec![ToolKind::Inspect]);
        assert!(parse_episode_file("not json").is_err());
    }

    #[test]
    fn hotkey_parse_round_trip() {
        assert!(parse_hotkey(&"ab".repeat(32)).is_some());
        assert!(parse_hotkey("0xzz").is_none());
    }
}
