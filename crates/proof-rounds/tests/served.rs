#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;

use std::{collections::BTreeMap, sync::Arc};

use axum::{body::Body, http::Request, Router};
use bundle::{LeafV1, LocalTrustRoot, ScoreOrAbsence};
use common::{
    pg::{Fixture, SEED},
    *,
};
use gateway::{
    admin_seal_router, bundle_router, weights_router, BundleStore, ChallengesBody, GatewayState,
    MemoryBundleStore, MemoryRawWeightStore, Registry, RegistryConfig,
};
use http_body_util::BodyExt;
use parity_scale_codec::DecodeAll;
use proof_autonomy::{ContributionAward, DecayPlan};
use proof_rounds::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use trustroot::{ChallengeEntry, ParticipantPolicy};
use uuid::Uuid;

async fn request(app: &Router, path: &str, document: Option<Value>) -> (u16, Value) {
    let request = Request::builder()
        .method(if document.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header("content-type", "application/json")
        .body(document.map_or_else(Body::empty, |value| {
            Body::from(serde_json::to_vec(&value).unwrap())
        }))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

fn wire(leaf: &LeafV1) -> Value {
    let score = match leaf.score_or_absence {
        ScoreOrAbsence::Score { value } => json!({"score": {"value":value}}),
        ScoreOrAbsence::NoScore { reason } => json!({"no_score": {"reason":reason as u8}}),
    };
    json!({ "challenge_id":String::from_utf8(leaf.challenge_id.clone()).unwrap(), "epoch":leaf.epoch,
        "miner_hotkey":hex::encode(leaf.miner_hotkey), "challenge_sig":hex::encode(leaf.challenge_sig),
        "score_or_absence":score })
}

#[tokio::test]
async fn retained_science_to_atlas_to_gateway_seal_and_independent_served_vector() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (research, _) = completed(&f).await;
    let store = RoundStore::new(f.database.app_pool().await.unwrap(), research, config());
    let source = source(&f).await;
    let frozen = store.freeze(&source, 0).await.unwrap();
    let proposal = paid_proposal(&frozen);
    let lease = store.acquire(0, Uuid::new_v4(), 60).await.unwrap();
    let decided = store.decide(&lease, &proposal, &SEED).await.unwrap();
    let proof_leaves = Vec::<LeafV1>::decode_all(&mut decided.leaves.as_slice()).unwrap();
    let (app, bundles, challenges) = gateway();
    assert_eq!(
        request(&app, "/v1/weights/latest", None).await.1["sealed"],
        false
    );
    let mut bad = wire(&proof_leaves[0]);
    bad["challenge_sig"] = json!("00".repeat(64));
    assert_eq!(request(&app, "/v1/weights/raw", Some(bad)).await.0, 401);
    for leaf in &proof_leaves {
        assert_eq!(
            request(&app, "/v1/weights/raw", Some(wire(leaf))).await.0,
            202
        );
    }
    let seal = json!({"epoch": source.epoch, "block_b":360, "netuid":1});
    assert_eq!(
        request(&app, "/v1/admin/seal", Some(seal.clone())).await.0,
        409
    );
    let scores: BTreeMap<_, _> = frozen
        .expected()
        .into_iter()
        .map(|key| {
            (
                key,
                ScoreOrAbsence::Score {
                    value: u64::from(key == [3; 32]),
                },
            )
        })
        .collect();
    let bounty = challenge_common::emit_signed_leaf_set(
        &SEED,
        b"bounty",
        source.epoch,
        &frozen.expected(),
        &scores,
    )
    .unwrap();
    for leaf in bounty.values() {
        assert_eq!(
            request(&app, "/v1/weights/raw", Some(wire(leaf))).await.0,
            202
        );
    }
    let sealed = request(&app, "/v1/admin/seal", Some(seal)).await;
    assert_eq!(sealed.0, 200, "{:?}", sealed.1);
    let latest = request(&app, "/v1/weights/latest", None).await;
    assert_eq!(latest.0, 200);
    assert_eq!(latest.1["sealed"], true);
    let sealed = bundles.get_by_epoch(source.epoch).unwrap();
    let sealed = bundle::EpochBundleV1::decode_all(&mut sealed.as_slice()).unwrap();
    let trust = LocalTrustRoot {
        challenges: (*challenges).clone(),
        measurements_digest: [8; 32],
    };
    bundle::verify_bundle(&sealed, &chain(), &trust).unwrap();
    assert_eq!(sealed.body.block_hash, source.boundary(360).unwrap().hash);
    let independent = validator::independent_python_weights(&sealed.body).unwrap();
    assert_eq!(sealed.body.final_vector, independent.final_vector);
    assert_eq!(independent.floats.uids, vec![0, 1, 2]);
    for (actual, expected) in independent.floats.weights.iter().zip([0.4_f64, 0.4, 0.2]) {
        assert!((actual - expected).abs() < 1e-12);
    }
    let served: Vec<f64> = serde_json::from_value(latest.1["weights"].clone()).unwrap();
    assert_eq!(
        served.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        independent
            .floats
            .weights
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    );
    f.close().await;
}

fn paid_proposal(frozen: &FrozenRound) -> proof_autonomy::AtlasDecision {
    let (digest, admitted) = frozen.contributions.first_key_value().unwrap();
    let mut proposal = decision(frozen);
    proposal.awards = vec![ContributionAward {
        contribution_digest: digest.clone(),
        miner_hotkey: hex::encode(miner()),
        units: 500_000,
        evidence_digests: admitted.evidence_digests.iter().cloned().collect(),
        rationale: "Synthetic observed discovery".into(),
        decay: DecayPlan {
            first_round: 0,
            initial_units: 500_000,
            retention_ppm: 500_000,
            expires_round: 10,
        },
        decay_revision: None,
    }];
    proposal
}

fn gateway() -> (Router, Arc<MemoryBundleStore>, Arc<ChallengesBody>) {
    let challenges = Arc::new(ChallengesBody {
        challenges: vec![
            ChallengeEntry {
                id: b"bounty".to_vec(),
                public_key: miner(),
                emission_share_bps: 2000,
                policy: ParticipantPolicy::AllMetagraphHotkeys,
            },
            ChallengeEntry {
                id: b"proof".to_vec(),
                public_key: miner(),
                emission_share_bps: 8000,
                policy: ParticipantPolicy::AllMetagraphHotkeys,
            },
        ],
    });
    let bundles = Arc::new(MemoryBundleStore::new());
    let state = GatewayState::with_parts_seal(
        Registry::shared(RegistryConfig::default()),
        Arc::new(validator_sync::SyncChain::new(chain())),
        challenges.clone(),
        Arc::new(MemoryRawWeightStore::new()),
        bundles.clone(),
        [8; 32],
        1,
    )
    .unwrap();
    std::env::set_var("BASE_GATEWAY_SK", hex::encode([9; 32]));
    let _ = telemetry::init_metrics();
    let app = weights_router(state.clone())
        .merge(bundle_router(state.clone()))
        .merge(admin_seal_router(state));
    (app, bundles, challenges)
}
