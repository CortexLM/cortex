#![allow(clippy::expect_used, clippy::unwrap_used, dead_code)]

use crate::common::{self, consent, signed, Fixture, SEED};
use proof_autonomy::{commitment, DeletionResult, MachineQuote, ProvisionResult};
use proof_autonomy_pg::{ControllerLease, CreateExperiment};
use proof_eval::BaselineMeasurement;
use proof_research::*;
use proof_score::{AgentVerdict, HarnessMetrics, ProofKind};
use proof_task::{HoldoutSplit, ProofPin, TopicDocument, TopicStatus};
use uuid::Uuid;

pub fn metrics(nll: f64) -> HarnessMetrics {
    HarnessMetrics {
        holdout_nll: nll,
        split_nll: HoldoutSplit::SCORED
            .iter()
            .map(|s| (s.as_str().into(), nll))
            .collect(),
        ..HarnessMetrics::default()
    }
}

pub fn scientific() -> (
    ProofPin,
    ScientificRecipe,
    ScientificEvidence,
    RetainedArtifacts,
) {
    let mut pin = ProofPin {
        topic_pubkey: common::miner(&SEED),
        eval_image_digest: format!("sha256:{}", "b".repeat(64)),
        ..ProofPin::default()
    };
    pin.inference.model = "local-faux".into();
    let baseline_script = b"print('local baseline fixture')".to_vec();
    let candidate_script = b"print('local candidate fixture')".to_vec();
    let baseline = BaselineMeasurement {
        eval_image_digest: pin.eval_image_digest.clone(),
        topic_id: "local-science".into(),
        holdout_commitment: "c".repeat(64),
        holdout_nll: 2.0,
        split_nll: metrics(2.0).split_nll,
        ..BaselineMeasurement::default()
    };
    let mut topic = TopicDocument {
        id: baseline.topic_id.clone(),
        statement: "Local synthetic scientific contract fixture".into(),
        status: TopicStatus::Open,
        holdout_commitment: baseline.holdout_commitment.clone(),
        ..TopicDocument::default()
    };
    topic.baseline.script_sha256 = artifact_digest(&baseline_script);
    topic.baseline.metrics_commitment = baseline.commitment();
    topic.signature = topic.sign_with(&SEED).unwrap();
    let recipe = ScientificRecipe {
        schema_version: 1,
        topic,
        baseline,
        candidate_script_digest: artifact_digest(&candidate_script),
        seeds: vec![1, 2, 3],
        maximum_wall_ms: 1000,
    };
    let mut artifacts = RetainedArtifacts::from([
        (artifact_digest(&baseline_script), baseline_script),
        (artifact_digest(&candidate_script), candidate_script),
    ]);
    let measurements = recipe
        .seeds
        .iter()
        .map(|seed| {
            let mut observed = |script: &str, nll: f64, kind: &str| {
                let log = format!("private synthetic log {kind} seed {seed}").into_bytes();
                let log_digest = artifact_digest(&log);
                artifacts.insert(log_digest.clone(), log);
                Measurement {
                    seed: *seed,
                    script_digest: script.into(),
                    log_digest,
                    exit_code: 0,
                    wall_ms: 50,
                    flops_used: 100,
                    metrics: metrics(nll),
                }
            };
            PairedMeasurement {
                baseline: observed(&recipe.topic.baseline.script_sha256, 2.0, "baseline"),
                candidate: observed(&recipe.candidate_script_digest, 1.9, "candidate"),
            }
        })
        .collect();
    let evidence = ScientificEvidence {
        schema_version: 1,
        experiment_id: Uuid::new_v4(),
        chain_epoch: 0,
        recipe_digest: commitment(&recipe).unwrap(),
        resource_id: "science-pod".into(),
        measurements,
        contamination_hits: vec![],
        verdict: AgentVerdict {
            verdict: ProofKind::Clean,
            reproduced: true,
            claim_holds_public: true,
            contamination: false,
            canary_hit: false,
            flops_used: 100,
            flops_budget: recipe.topic.flops_budget,
            cheat_codes: vec![],
            rationale: "private synthetic rationale".into(),
            topic_id: recipe.topic.id.clone(),
            family: recipe.topic.metric.family,
        },
    };
    (pin, recipe, evidence, artifacts)
}

pub async fn running(f: &Fixture, pin: &ProofPin, digest: String, id: Uuid) -> ControllerLease {
    let request = CreateExperiment {
        id,
        account_id: f.account.id,
        recipe_digest: digest,
    };
    let e = f
        .store
        .create_experiment(
            &request,
            &signed(&request, "/v2/experiments", f.now().await, &SEED),
        )
        .await
        .unwrap();
    let lease = f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap();
    let now = f.now().await;
    let quote = MachineQuote {
        schema_version: 1,
        id: Uuid::new_v4(),
        experiment_id: e.id,
        miner_hotkey: e.miner_hotkey.clone(),
        account_id: e.account_id,
        recipe_digest: e.recipe_digest.clone(),
        offer_id: "local".into(),
        gpu_type: "local".into(),
        gpu_count: 1,
        gpu_memory_mib: 100,
        ram_mib: 100,
        disk_gib: 10,
        image: pin.eval_image.clone(),
        image_digest: pin.eval_image_digest.clone(),
        hourly_total_microusd: 100,
        maximum_total_microusd: 100,
        lifetime_seconds: 3600,
        issued_at: now,
        expires_at: now + 120,
        provider_fingerprint: "d".repeat(64),
    };
    let e = f.store.publish_quote(&lease, 0, &quote).await.unwrap();
    let e = f
        .store
        .consent(e.id, e.revision, &consent(&quote))
        .await
        .unwrap();
    let intent = f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
    f.store
        .begin_provision(&lease, e.revision, intent)
        .await
        .unwrap();
    f.store
        .record_provision(
            &lease,
            intent,
            &ProvisionResult::Confirmed {
                resource_id: "science-pod".into(),
            },
        )
        .await
        .unwrap();
    f.store
        .adopt_resource(&lease, e.revision + 1)
        .await
        .unwrap();
    lease
}

pub async fn clean(f: &Fixture, lease: &ControllerLease) {
    let target = f.store.begin_cleanup(lease, "science-pod").await.unwrap();
    f.store
        .record_deletion(
            lease,
            "science-pod",
            target.resource.deletion_id.unwrap(),
            &DeletionResult::Confirmed,
        )
        .await
        .unwrap();
}
