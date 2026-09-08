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
    force_sim, resolve_eval_backend, scoring_readiness, sim_stub_win, supported_custom,
    BaselineMeasurement, EvalBackend, LiveScorer,
};
pub use proof_http::{hash_admin_token, proof_router, AppState};
pub use proof_store::{ArtifactManifest, MemoryStore, StoreError};
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

/// Why a journaled emission could not sign a leaf set.
#[derive(Debug, thiserror::Error)]
pub enum EmitStoreError {
    /// Durable snapshot or sync reader failed; never treat this as "nobody scored".
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// D24 leaf signing failed.
    #[error("leaf: {0}")]
    Leaf(#[from] LeafEmitError),
}

/// Per-miner topic attempts currently in the store.
///
/// Journaled stores refuse these sync readers. Call [`store_runs_durable`]
/// or read a [`MemoryStore::snapshot_durable`] first. Errors are not empty maps.
pub fn store_runs(
    store: &MemoryStore,
) -> Result<BTreeMap<String, BTreeMap<String, MinerTopicRun>>, StoreError> {
    let mut out = BTreeMap::new();
    for k in store.scored_hotkeys()? {
        out.insert(k.clone(), store.miner_runs(&k)?);
    }
    Ok(out)
}

/// Per-miner topic lattices currently in the store (binary pass map).
///
/// Journaled stores refuse these sync readers. Errors are not empty maps.
pub fn store_scores(
    store: &MemoryStore,
) -> Result<BTreeMap<String, BTreeMap<String, u64>>, StoreError> {
    let mut out = BTreeMap::new();
    for k in store.scored_hotkeys()? {
        out.insert(k.clone(), store.miner_scores(&k)?);
    }
    Ok(out)
}

/// Sealed baselines currently in the store.
pub fn store_baselines(
    store: &MemoryStore,
) -> Result<BTreeMap<String, SealedBaseline>, StoreError> {
    let mut out = BTreeMap::new();
    for topic in store.topics()? {
        if let Some(metrics) = store.baseline(&topic.id)? {
            out.insert(topic.id, metrics);
        }
    }
    Ok(out)
}

/// Operator-crowned champion primaries currently in the store.
pub fn store_champions(store: &MemoryStore) -> Result<BTreeMap<String, f64>, StoreError> {
    let mut out = BTreeMap::new();
    for topic in store.topics()? {
        if let Some(primary) = store.champion_primary(&topic)? {
            out.insert(topic.id, primary);
        }
    }
    Ok(out)
}

/// Decode stored hex hotkeys. An invalid key is a store fault, not a skip.
pub fn runs_by_hotkey(
    hex_runs: &BTreeMap<String, BTreeMap<String, MinerTopicRun>>,
) -> Result<BTreeMap<Hotkey, BTreeMap<String, MinerTopicRun>>, StoreError> {
    let mut out = BTreeMap::new();
    for (key, runs) in hex_runs {
        let hotkey = parse_hotkey(key).ok_or_else(|| {
            StoreError::Illegal(format!("stored miner hotkey is not 32 bytes: {key}"))
        })?;
        out.insert(hotkey, runs.clone());
    }
    Ok(out)
}

/// Refresh the journal snapshot, then read payout runs. Propagates backend errors.
pub async fn store_runs_durable(
    store: &MemoryStore,
) -> Result<BTreeMap<String, BTreeMap<String, MinerTopicRun>>, StoreError> {
    store_runs(&store.snapshot_durable().await?)
}

/// Refresh the journal snapshot once, then sign the exact-E leaf set.
///
/// A journal failure is an error, never an empty score map. Sync readers on
/// the live journaled store still refuse; this path always snapshots first.
pub async fn emit_epoch_from_store(
    store: &MemoryStore,
    secret: &[u8; 32],
    epoch: u64,
    expected: &BTreeSet<Hotkey>,
) -> Result<BTreeMap<Hotkey, bundle::LeafV1>, EmitStoreError> {
    let snap = store.snapshot_durable().await?;
    let mut topics = Vec::new();
    for id in snap.open_ids(epoch)? {
        topics.push(snap.topic(&id)?);
    }
    let sealed = store_baselines(&snap)?;
    let champions = store_champions(&snap)?;
    let per_miner = runs_by_hotkey(&store_runs(&snap)?)?;
    Ok(emit_epoch(
        secret, epoch, expected, &topics, &sealed, &champions, &per_miner,
    )?)
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

    #[test]
    fn store_readers_propagate_errors_instead_of_empty_maps() {
        let store = MemoryStore::new();
        store
            .put_topic(discovery_topic("adamw-beater-v0"))
            .expect("topic");
        store
            .record_topic_run(&hex::encode([1u8; 32]), "adamw-beater-v0", pass(2.5, "d2"))
            .expect("run");
        assert_eq!(store_runs(&store).expect("runs").len(), 1);
        assert_eq!(store_scores(&store).expect("scores").len(), 1);
        assert!(
            runs_by_hotkey(&BTreeMap::from([("not-a-hotkey".into(), BTreeMap::new())])).is_err()
        );
    }

    #[tokio::test]
    async fn emit_epoch_from_store_snapshots_memory_and_covers_e() {
        let store = MemoryStore::new();
        let topic = discovery_topic("adamw-beater-v0");
        store.put_topic(topic.clone()).expect("topic");
        store
            .set_baseline("adamw-beater-v0", proof_score::flat_nll(3.0))
            .expect("sealed");
        let a = [1u8; 32];
        let b = [2u8; 32];
        store
            .record_topic_run(&hex::encode(a), "adamw-beater-v0", pass(2.5, "d2"))
            .expect("run");
        let expected: BTreeSet<Hotkey> = [a, b].into_iter().collect();
        let leaves = emit_epoch_from_store(&store, &sk(), 1, &expected)
            .await
            .expect("emit");
        assert_eq!(leaves.len(), 2);
        assert!(matches!(
            leaves[&a].score_or_absence,
            ScoreOrAbsence::Score { .. }
        ));
        assert!(matches!(
            leaves[&b].score_or_absence,
            ScoreOrAbsence::NoScore { .. }
        ));
    }

    #[tokio::test]
    #[ignore = "requires disposable DATABASE_URL"]
    #[allow(clippy::too_many_lines)]
    async fn emit_epoch_from_store_reads_the_journal_snapshot() {
        let database = db::test_pool().await.expect("disposable postgres");
        let writer_pool = database.app_pool().await.expect("writer");
        let reader_pool = database.app_pool().await.expect("reader");
        let writer = MemoryStore::new()
            .with_journal(proof_store::durable::DurableJournal::new(writer_pool))
            .await
            .expect("journal");
        let reader = MemoryStore::new()
            .with_journal(proof_store::durable::DurableJournal::new(
                reader_pool.clone(),
            ))
            .await
            .expect("journal");
        assert!(
            store_runs(&writer).is_err(),
            "journaled sync readers must refuse, not return empty"
        );
        let topic = discovery_topic("adamw-beater-v0");
        writer.put_topic(topic.clone()).expect("topic");
        reader.put_topic(topic).expect("topic");
        writer
            .set_baseline("adamw-beater-v0", proof_score::flat_nll(3.0))
            .expect("sealed");
        reader
            .set_baseline("adamw-beater-v0", proof_score::flat_nll(3.0))
            .expect("sealed");
        let a = [1u8; 32];
        let row = proof_store::Submission {
            id: String::new(),
            topic_id: "adamw-beater-v0".into(),
            miner_hotkey: hex::encode(a),
            artifact_digest: "ab".repeat(32),
            artifact_uri: None,
            claim: "durable emit".into(),
            declared_flops: 1,
            architecture: String::new(),
            inference_offer_id: String::new(),
            config_commitment: String::new(),
            manifest: proof_store::ArtifactManifest::default(),
            nonce: "n".into(),
            submission_digest: "cd".repeat(32),
            state: proof_store::SubmissionState::AwaitingAdmin,
            receipt_json: None,
            verdict: None,
            detail: None,
        };
        writer
            .finish_durable(row, pass(2.5, &"ab".repeat(32)))
            .await
            .expect("finish");
        let expected: BTreeSet<Hotkey> = [a].into_iter().collect();
        let leaves = emit_epoch_from_store(&reader, &sk(), 1, &expected)
            .await
            .expect("reader must snapshot the writer's commit");
        assert!(matches!(
            leaves[&a].score_or_absence,
            ScoreOrAbsence::Score { .. }
        ));
        reader_pool.close().await;
        assert!(emit_epoch_from_store(&reader, &sk(), 1, &expected)
            .await
            .is_err());
        database.drop_schema().await.expect("drop");
    }
}
