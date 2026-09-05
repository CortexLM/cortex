//! Proof orchestrator helpers: D24 leaf plan + crate re-exports.

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
use proof_score::{payout_lattices, MinerTopicRun, SealedBaseline};
use proof_task::{CHALLENGE_ID_BYTES, SCORE_MAX};

pub use proof_eval::{
    force_sim, resolve_eval_backend, scoring_readiness, supported_custom, BaselineMeasurement,
    EvalBackend, LiveScorer,
};
pub use proof_http::{hash_admin_token, proof_router, AppState};
pub use proof_store::{ArtifactManifest, MemoryStore};
pub use proof_task::{
    HoldoutRecord, InferenceOffer, OfferError, ProofPin, TopicDocument, BASE_MODEL_FAMILY,
    CHALLENGE_ID, CHALLENGE_ID_BYTES as PROOF_ID_BYTES, SCORE_MAX as PROOF_SCORE_MAX,
    SCORING_VERSION,
};

/// Build a D24-complete score map: each expected hotkey is a **sum** of
/// WTA/discovery topic masses, or an explicit `NoScore`.
///
/// Empty open set → `NoScore(ChallengeInternal)` (host problem, not a paid 0).
pub fn emission_scores(
    expected: &BTreeSet<Hotkey>,
    topics: &[TopicDocument],
    sealed: &BTreeMap<String, SealedBaseline>,
    champion_primary: &BTreeMap<String, f64>,
    per_miner: &BTreeMap<Hotkey, BTreeMap<String, MinerTopicRun>>,
) -> BTreeMap<Hotkey, ScoreOrAbsence> {
    if topics.is_empty() {
        return expected
            .iter()
            .map(|h| {
                (
                    *h,
                    ScoreOrAbsence::NoScore {
                        reason: NoScoreReasonCode::ChallengeInternal,
                    },
                )
            })
            .collect();
    }
    let mut hex_runs: BTreeMap<String, BTreeMap<String, MinerTopicRun>> = BTreeMap::new();
    for (h, runs) in per_miner {
        hex_runs.insert(hex::encode(h), runs.clone());
    }
    let paid = payout_lattices(topics, sealed, champion_primary, &hex_runs);
    expected
        .iter()
        .map(|h| {
            let value = paid
                .get(&hex::encode(h))
                .copied()
                .unwrap_or(0)
                .min(SCORE_MAX);
            let s = if value > 0 {
                ScoreOrAbsence::Score { value }
            } else {
                ScoreOrAbsence::NoScore {
                    reason: NoScoreReasonCode::NotAttempted,
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
    topics: &[TopicDocument],
    sealed: &BTreeMap<String, SealedBaseline>,
    champion_primary: &BTreeMap<String, f64>,
    per_miner: &BTreeMap<Hotkey, BTreeMap<String, MinerTopicRun>>,
) -> Result<BTreeMap<Hotkey, bundle::LeafV1>, LeafEmitError> {
    let scores = emission_scores(expected, topics, sealed, champion_primary, per_miner);
    emit_signed_leaf_set(secret, CHALLENGE_ID_BYTES, epoch, expected, &scores)
}

/// Per-miner topic attempts currently in the store.
pub fn store_runs(store: &MemoryStore) -> BTreeMap<String, BTreeMap<String, MinerTopicRun>> {
    let mut out = BTreeMap::new();
    if let Ok(keys) = store.scored_hotkeys() {
        for k in keys {
            if let Ok(m) = store.miner_runs(&k) {
                out.insert(k, m);
            }
        }
    }
    out
}

/// Per-miner topic lattices currently in the store (binary pass map).
pub fn store_scores(store: &MemoryStore) -> BTreeMap<String, BTreeMap<String, u64>> {
    let mut out = BTreeMap::new();
    if let Ok(keys) = store.scored_hotkeys() {
        for k in keys {
            if let Ok(m) = store.miner_scores(&k) {
                out.insert(k, m);
            }
        }
    }
    out
}

/// Parse a 32-byte hex hotkey.
pub fn parse_hotkey(hex_s: &str) -> Option<Hotkey> {
    let t = hex_s.trim().trim_start_matches("0x");
    let bytes = hex::decode(t).ok()?;
    <[u8; 32]>::try_from(bytes).ok()
}

/// Load frozen holdout records from an operator JSON file body.
///
/// The body is a JSON array, or `{ "topics": { "<id>": [...] } }` / `{ "<id>": [...] }`.
pub fn parse_holdout_file(body: &str, topic_id: &str) -> Result<Vec<HoldoutRecord>, String> {
    if let Ok(list) = serde_json::from_str::<Vec<HoldoutRecord>>(body) {
        return Ok(list);
    }
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("parse holdout: {e}"))?;
    if let Some(map) = value.get("topics").and_then(|v| v.as_object()) {
        if let Some(arr) = map.get(topic_id) {
            return serde_json::from_value(arr.clone())
                .map_err(|e| format!("parse holdout for {topic_id}: {e}"));
        }
    }
    if let Some(arr) = value.get(topic_id) {
        return serde_json::from_value(arr.clone())
            .map_err(|e| format!("parse holdout for {topic_id}: {e}"));
    }
    Err(format!("no holdout records for topic {topic_id}"))
}

#[cfg(test)]
mod tests {
    use challenge_common::public_key_from_secret;
    use crypto::KEY_LEN;
    use proof_score::MinerTopicRun;
    use proof_task::{
        default_adamw, holdout_commitment, synthetic_holdout, PayoutMode, TopicDocument,
        TopicStatus, FLOPS_BUDGET_MAX, METRIC_TOKENS_PER_SEC,
    };

    use super::*;

    fn sk() -> [u8; KEY_LEN] {
        let mut s = [9u8; KEY_LEN];
        s[0] = 7;
        s
    }

    fn sealed() -> proof_task::Baseline {
        let mut b = default_adamw(FLOPS_BUDGET_MAX);
        b.script_sha256 = "11".repeat(32);
        b.metrics_commitment = "22".repeat(32);
        b
    }

    fn discovery_topic(id: &str) -> TopicDocument {
        TopicDocument {
            id: id.into(),
            statement: "Beat sealed AdamW.".into(),
            payout_mode: PayoutMode::Discovery,
            baseline: sealed(),
            holdout_commitment: holdout_commitment(&synthetic_holdout(24, 1)),
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    fn wta_topic(id: &str) -> TopicDocument {
        let mut t = discovery_topic(id);
        t.payout_mode = PayoutMode::Wta;
        t.metric.family = proof_task::MetricFamily::Throughput;
        t.metric.primary = METRIC_TOKENS_PER_SEC.into();
        t.metric.direction = proof_task::MetricDirection::Max;
        t.metric.epsilon_rel = 0.05;
        t.metric.quality_floor_nll = 0.02;
        t.metric.wall_budget_s = 14_400;
        t
    }

    fn pass(primary: f64, digest: &str) -> MinerTopicRun {
        MinerTopicRun {
            pass: true,
            primary: Some(primary),
            artifact_digest: digest.into(),
            near_duplicate: false,
        }
    }

    #[test]
    fn d24_covers_every_hotkey_and_sums_open_topics() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let e: BTreeSet<Hotkey> = [a, b].into_iter().collect();
        let topics = vec![wta_topic("dt-no-ib-v0"), discovery_topic("adamw-beater-v0")];
        let mut per = BTreeMap::new();
        per.insert(a, {
            let mut m = BTreeMap::new();
            m.insert("dt-no-ib-v0".into(), pass(200.0, "d1"));
            m.insert("adamw-beater-v0".into(), pass(2.5, "d2"));
            m
        });
        let mut sealed = BTreeMap::new();
        sealed.insert("adamw-beater-v0".into(), proof_score::flat_nll(3.0));
        let leaves =
            emit_epoch(&sk(), 9, &e, &topics, &sealed, &BTreeMap::new(), &per).expect("emit");
        assert_eq!(leaves.len(), 2);
        assert!(matches!(
            leaves[&a].score_or_absence,
            ScoreOrAbsence::Score { value: SCORE_MAX }
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
    fn skipped_open_topic_contributes_zero_to_the_sum() {
        let a = [1u8; 32];
        let e: BTreeSet<Hotkey> = [a].into_iter().collect();
        let topics = vec![wta_topic("dt-no-ib-v0"), wta_topic("other-v0")];
        let mut per = BTreeMap::new();
        per.insert(a, {
            let mut m = BTreeMap::new();
            m.insert("dt-no-ib-v0".into(), pass(200.0, "d1"));
            m
        });
        let leaves = emit_epoch(
            &sk(),
            1,
            &e,
            &topics,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &per,
        )
        .expect("emit");
        match &leaves[&a].score_or_absence {
            ScoreOrAbsence::Score { value } => assert_eq!(*value, SCORE_MAX / 2),
            ScoreOrAbsence::NoScore { .. } => panic!("expected a score"),
        }
    }

    #[test]
    fn no_open_topics_is_challenge_internal_not_a_zero() {
        let a = [1u8; 32];
        let e: BTreeSet<Hotkey> = [a].into_iter().collect();
        let leaves = emit_epoch(
            &sk(),
            1,
            &e,
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .expect("emit");
        assert!(matches!(
            leaves[&a].score_or_absence,
            ScoreOrAbsence::NoScore {
                reason: NoScoreReasonCode::ChallengeInternal
            }
        ));
    }

    #[test]
    fn leaf_domain_is_not_another_live_challenge() {
        assert_eq!(PROOF_ID_BYTES, b"proof");
        for other in [
            &b"relearn"[..],
            b"relearn-agent",
            b"bounty",
            b"relearn-image",
        ] {
            assert_ne!(PROOF_ID_BYTES, other);
        }
    }
}
