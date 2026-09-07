#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use proof_autonomy::*;
use uuid::Uuid;

fn hotkey() -> String {
    hex::encode(challenge_common::public_key_from_secret(&[7; 32]).unwrap())
}

fn quote() -> MachineQuote {
    MachineQuote {
        schema_version: 1,
        id: Uuid::new_v4(),
        experiment_id: Uuid::new_v4(),
        miner_hotkey: hotkey(),
        account_id: Uuid::new_v4(),
        recipe_digest: "a".repeat(64),
        offer_id: "executor-1".into(),
        gpu_type: "H100".into(),
        gpu_count: 2,
        gpu_memory_mib: 80_000,
        ram_mib: 128_000,
        disk_gib: 100,
        image: "ghcr.io/cortexlm/proof-eval".into(),
        image_digest: format!("sha256:{}", "b".repeat(64)),
        hourly_total_microusd: 4_000_000,
        maximum_total_microusd: 8_000_000,
        lifetime_seconds: 7_200,
        issued_at: 100,
        expires_at: 200,
        provider_fingerprint: "c".repeat(64),
    }
}

#[test]
fn consent_binds_machine_owner_and_total_price() {
    let mut quote = quote();
    let digest = commitment(&quote).unwrap();
    let consent = SignedConsent {
        quote_digest: digest.clone(),
        signature: hex::encode(
            crypto::sign_raw(&[7; 32], CONSENT_DOMAIN, digest.as_bytes()).unwrap(),
        ),
    };
    assert!(consent.verify(&quote, 150).is_ok());
    assert_eq!(consent.verify(&quote, 200), Err(ContractError::Expired));
    quote.offer_id = "executor-2".into();
    assert_eq!(consent.verify(&quote, 150), Err(ContractError::Signature));
    quote.maximum_total_microusd = 1;
    assert!(quote.validate(150).is_err());
}

#[test]
fn signed_actions_cannot_cross_methods_paths_or_bodies() {
    let mut action = SignedAction {
        miner_hotkey: hotkey(),
        nonce: Uuid::new_v4(),
        expires_at: 200,
        method: "POST".into(),
        path: "/v2/experiments/one/cancel".into(),
        body_digest: "a".repeat(64),
        signature: String::new(),
    };
    let payload = (
        &action.miner_hotkey,
        action.nonce,
        action.expires_at,
        &action.method,
        &action.path,
        &action.body_digest,
    );
    action.signature = hex::encode(
        crypto::sign_raw(
            &[7; 32],
            ACTION_DOMAIN,
            commitment(&payload).unwrap().as_bytes(),
        )
        .unwrap(),
    );
    assert!(action
        .verify("POST", &action.path, &action.body_digest, 150)
        .is_ok());
    assert!(action
        .verify("DELETE", &action.path, &action.body_digest, 150)
        .is_err());
    assert!(action
        .verify("POST", "/other", &action.body_digest, 150)
        .is_err());
}

#[test]
fn revoked_capability_never_executes_or_crosses_experiments() {
    let mut grant = ResourceGrant {
        experiment_id: Uuid::new_v4(),
        account_id: Uuid::new_v4(),
        resource_id: "pod-1".into(),
        expires_at: 200,
        revoked: false,
        operations: vec![CapabilityOperation::Execute],
    };
    let authorize = |grant: &ResourceGrant, id| {
        grant.authorize(
            id,
            grant.account_id,
            "pod-1",
            CapabilityOperation::Execute,
            100,
        )
    };
    assert!(authorize(&grant, grant.experiment_id).is_ok());
    assert_eq!(authorize(&grant, Uuid::new_v4()), Err(ContractError::Scope));
    grant.revoked = true;
    assert_eq!(
        authorize(&grant, grant.experiment_id),
        Err(ContractError::Scope)
    );
}

#[test]
fn cancellation_cannot_complete_before_deletion() {
    use ExperimentState::{Cancelled, Cancelling, Deleting, Reconciling, Running};
    assert!(Running.transition(Cancelling).is_ok());
    assert!(Cancelling.transition(Cancelled).is_err());
    assert!(Cancelling.transition(Deleting).is_ok());
    assert!(Deleting.transition(Reconciling).is_ok());
    assert!(Reconciling.transition(Running).is_err());
    assert!(Deleting.transition(Cancelled).is_ok());
    assert!(Cancelled.transition(Running).is_err());
}

fn snapshot(round: u64) -> RoundSnapshot {
    RoundSnapshot {
        round,
        anchor_block: 100,
        finalized_block: 100 + (round + 1) * ROUND_BLOCKS,
        finalized_hash: "1".repeat(64),
        chain_epoch: round + 1,
        corpus_digest: "2".repeat(64),
        policy_digest: "3".repeat(64),
        runtime_digest: "4".repeat(64),
    }
}

fn award() -> ContributionAward {
    ContributionAward {
        contribution_digest: "a".repeat(64),
        miner_hotkey: hex::encode([5; 32]),
        units: 500_000,
        evidence_digests: vec!["b".repeat(64)],
        rationale: "Reproduced improvement under the approved compute budget.".into(),
        decay_revision: None,
        decay: DecayPlan {
            first_round: 0,
            initial_units: 500_000,
            retention_ppm: 500_000,
            expires_round: 10,
        },
    }
}

#[test]
fn atlas_decay_is_per_contribution_and_cannot_be_reset() {
    let mut award = award();
    let mut admitted = BTreeMap::from([(
        award.contribution_digest.clone(),
        AdmittedContribution {
            miner_hotkey: [5; 32],
            evidence_digests: BTreeSet::from(["b".repeat(64)]),
            rewardable: true,
            previous: None,
        },
    )]);
    let mut decision = AtlasDecision {
        schema_version: 1,
        scoring_version: ATLAS_SCORING_VERSION,
        snapshot: snapshot(0),
        awards: vec![award.clone()],
        rationale: "Allocate to the verified discovery; retain residual burn.".into(),
    };
    let first = decision.validate(&snapshot(0), &admitted).unwrap();
    admitted
        .get_mut(&award.contribution_digest)
        .unwrap()
        .previous = first.awards.get(&award.contribution_digest).cloned();
    decision.snapshot = snapshot(1);
    assert!(decision.validate(&snapshot(1), &admitted).is_err());
    award.units = 250_000;
    decision.awards = vec![award.clone()];
    let second = decision.validate(&snapshot(1), &admitted).unwrap();
    assert_eq!(second.burn_units, 750_000);
    award.decay.first_round = 1;
    decision.awards = vec![award];
    assert!(decision.validate(&snapshot(1), &admitted).is_err());
}

#[test]
fn exact_e_residual_preserves_absolute_allocation() {
    let allocation = Allocation {
        miner_units: BTreeMap::from([([5; 32], 250_000)]),
        burn_units: 750_000,
        awards: BTreeMap::new(),
    };
    let expected = BTreeSet::from([[0; 32], [5; 32], [6; 32]]);
    let uids = BTreeMap::from([([0; 32], 0), ([5; 32], 1), ([6; 32], 2)]);
    let scores = allocation.emission_scores(&expected, &uids).unwrap();
    assert_eq!(scores.len(), expected.len());
    assert!(matches!(
        scores[&[0; 32]],
        bundle::ScoreOrAbsence::Score { value: 750_000 }
    ));
    assert!(allocation
        .emission_scores(&BTreeSet::from([[5; 32]]), &uids)
        .is_err());
}

#[test]
fn stale_corpus_and_unverified_evidence_refuse_credit() {
    let decision = AtlasDecision {
        schema_version: 1,
        scoring_version: ATLAS_SCORING_VERSION,
        snapshot: snapshot(0),
        awards: vec![award()],
        rationale: "Verified discovery.".into(),
    };
    assert_eq!(
        decision
            .validate(&snapshot(1), &BTreeMap::new())
            .unwrap_err(),
        ContractError::Stale
    );
    assert_eq!(
        decision
            .validate(&snapshot(0), &BTreeMap::new())
            .unwrap_err(),
        ContractError::Evidence
    );
}

#[test]
fn decay_is_bounded_for_extreme_rounds() {
    let mut decay = award().decay;
    decay.expires_round = u64::MAX;
    assert_eq!(decay.cap_at(u64::MAX - 1), 0);
    decay.retention_ppm = PPM;
    assert!(decay.validate().is_err());
}

#[test]
fn revised_decay_binds_history_without_resetting_age_or_increasing_credit() {
    let mut proposed = award();
    let previous = PreviousAward {
        round: 0,
        units: proposed.units,
        decay: proposed.decay.clone(),
    };
    let admitted = BTreeMap::from([(
        proposed.contribution_digest.clone(),
        AdmittedContribution {
            miner_hotkey: [5; 32],
            evidence_digests: BTreeSet::from(["b".repeat(64)]),
            rewardable: true,
            previous: Some(previous.clone()),
        },
    )]);
    proposed.units = 200_000;
    proposed.decay.retention_ppm = 600_000;
    let mut decision = AtlasDecision {
        schema_version: 1,
        scoring_version: ATLAS_SCORING_VERSION,
        snapshot: snapshot(1),
        awards: vec![proposed],
        rationale: "Audited change in continuing utility.".into(),
    };
    assert!(decision.validate(&snapshot(1), &admitted).is_err());
    decision.awards[0].decay_revision = Some(DecayRevision {
        previous_award_digest: commitment(&previous).unwrap(),
        rationale: "Retain value longer, without raising current credit or resetting age.".into(),
    });
    assert!(decision.validate(&snapshot(1), &admitted).is_ok());
    let valid = decision.clone();
    decision.awards[0].decay.first_round = 1;
    assert!(decision.validate(&snapshot(1), &admitted).is_err());
    decision = valid.clone();
    decision.awards[0].decay.initial_units += 1;
    assert!(decision.validate(&snapshot(1), &admitted).is_err());
    decision = valid.clone();
    decision.awards[0].units = previous.units + 1;
    assert!(decision.validate(&snapshot(1), &admitted).is_err());
    decision = valid;
    decision.awards[0]
        .decay_revision
        .as_mut()
        .unwrap()
        .previous_award_digest = "f".repeat(64);
    assert!(decision.validate(&snapshot(1), &admitted).is_err());
}

#[test]
fn absolute_decay_survives_the_actual_served_python_normalization() {
    let burn = [3; 32];
    let miner = [5; 32];
    let bounty = [6; 32];
    let expected = BTreeSet::from([burn, miner, bounty]);
    let uids = BTreeMap::from([(burn, 0), (miner, 1), (bounty, 2)]);
    for (units, expected_miner, expected_burn) in [(500_000, 0.4, 0.4), (250_000, 0.2, 0.6)] {
        let allocation = Allocation {
            miner_units: BTreeMap::from([(miner, units)]),
            burn_units: 1_000_000 - units,
            awards: BTreeMap::new(),
        };
        let scores = allocation.emission_scores(&expected, &uids).unwrap();
        let signed =
            challenge_common::emit_signed_leaf_set(&[7; 32], b"proof", 1, &expected, &scores)
                .unwrap();
        let mut leaves: Vec<aggregate::VerifiedLeaf> = signed
            .values()
            .map(|leaf| {
                challenge_common::verify_leaf_sig(
                    leaf,
                    &challenge_common::public_key_from_secret(&[7; 32]).unwrap(),
                )
                .unwrap();
                let score_or_absence = match leaf.score_or_absence {
                    bundle::ScoreOrAbsence::Score { value } => {
                        aggregate::ScoreOrAbsence::Score { value }
                    }
                    bundle::ScoreOrAbsence::NoScore { reason } => {
                        aggregate::ScoreOrAbsence::NoScore {
                            reason: reason as u8,
                        }
                    }
                };
                aggregate::VerifiedLeaf {
                    challenge_id: b"proof".to_vec(),
                    miner_hotkey: leaf.miner_hotkey,
                    score_or_absence,
                }
            })
            .collect();
        leaves.push(aggregate::VerifiedLeaf {
            challenge_id: b"bounty".to_vec(),
            miner_hotkey: bounty,
            score_or_absence: aggregate::ScoreOrAbsence::Score { value: 1 },
        });
        let vector = aggregate::aggregate_python_vector(
            &leaves,
            &[(b"proof".to_vec(), 8000), (b"bounty".to_vec(), 2000)],
            &uids
                .iter()
                .map(|(key, uid)| (*key, *uid))
                .collect::<Vec<_>>(),
            aggregate::ALGORITHM_VERSION,
        )
        .unwrap();
        for (uid, expected) in [(0, expected_burn), (1, expected_miner), (2, 0.2)] {
            let index = vector
                .floats
                .uids
                .iter()
                .position(|found| *found == uid)
                .unwrap();
            assert!((vector.floats.weights[index] - expected).abs() < 1e-12);
        }
    }
}
