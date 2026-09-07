#![allow(clippy::expect_used, clippy::unwrap_used, dead_code)]

#[path = "../../../proof-autonomy-pg/tests/common/mod.rs"]
pub mod pg;

use async_trait::async_trait;
use chain::{ChainClient, FakeChain, FakeChainConfig};
use chain_live::FinalizedSnapshot;
use pg::{consent, signed, Fixture, SEED};
use proof_autonomy::{
    commitment, AtlasDecision, DeletionResult, MachineQuote, ProvisionResult, ATLAS_SCORING_VERSION,
};
use proof_autonomy_pg::CreateExperiment;
use proof_eval::BaselineMeasurement;
use proof_research::*;
use proof_rounds::*;
use proof_score::{AgentVerdict, HarnessMetrics, ProofKind};
use proof_task::{HoldoutSplit, ProofPin, TopicDocument, TopicStatus};
use uuid::Uuid;

pub fn miner() -> [u8; 32] {
    challenge_common::public_key_from_secret(&SEED).unwrap()
}

pub fn config() -> RoundConfig {
    RoundConfig {
        netuid: 1,
        anchor_block: 0,
        policy_digest: "a".repeat(64),
        runtime_digest: "b".repeat(64),
        proof_public_key: hex::encode(miner()),
    }
}

pub fn chain() -> FakeChain {
    FakeChain::new(FakeChainConfig {
        hotkeys: vec![vec![1; 32], miner().to_vec(), vec![3; 32]],
        current_block: 10_000,
        owner_hotkey: vec![1; 32],
        ..FakeChainConfig::default()
    })
}

pub struct Source {
    pub cutoff: u64,
    pub epoch: u64,
}
impl FinalizedRoundSource for Source {
    fn boundary(&self, block: u64) -> Result<FinalizedSnapshot, RoundError> {
        let chain = chain();
        let hash = chain.block_hash(block).unwrap();
        Ok(FinalizedSnapshot {
            block,
            hash,
            chain_epoch: self.epoch,
            timestamp_ms: self.cutoff,
            metagraph: chain.metagraph_at(&hash).unwrap(),
        })
    }
}

pub async fn source(f: &Fixture) -> Source {
    let cutoff: i64 =
        sqlx::query_scalar("SELECT ceil(extract(epoch FROM clock_timestamp()) * 1000)::bigint")
            .fetch_one(f.database.pool())
            .await
            .unwrap();
    Source {
        cutoff: u64::try_from(cutoff).unwrap(),
        epoch: 7,
    }
}

pub fn decision(frozen: &FrozenRound) -> AtlasDecision {
    AtlasDecision {
        schema_version: 1,
        scoring_version: ATLAS_SCORING_VERSION,
        snapshot: frozen.snapshot.clone(),
        awards: vec![],
        rationale: "Local test proposal".into(),
    }
}

fn metrics(nll: f64) -> HarnessMetrics {
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
        topic_pubkey: pg::miner(&SEED),
        eval_image_digest: format!("sha256:{}", "b".repeat(64)),
        ..ProofPin::default()
    };
    pin.inference.model = "local-faux".into();
    let baseline_script = b"print('local baseline')".to_vec();
    let candidate_script = b"print('local candidate')".to_vec();
    let baseline = BaselineMeasurement {
        eval_image_digest: pin.eval_image_digest.clone(),
        topic_id: "round-fixture".into(),
        holdout_commitment: "c".repeat(64),
        holdout_nll: 2.0,
        split_nll: metrics(2.0).split_nll,
        ..BaselineMeasurement::default()
    };
    let mut topic = TopicDocument {
        id: baseline.topic_id.clone(),
        statement: "Synthetic round fixture".into(),
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
            let mut observation = |script: &str, nll: f64| {
                let log = format!("synthetic local observation {seed} {nll}").into_bytes();
                let digest = artifact_digest(&log);
                artifacts.insert(digest.clone(), log);
                Measurement {
                    seed: *seed,
                    script_digest: script.into(),
                    log_digest: digest,
                    exit_code: 0,
                    wall_ms: 10,
                    flops_used: 100,
                    metrics: metrics(nll),
                }
            };
            PairedMeasurement {
                baseline: observation(&recipe.topic.baseline.script_sha256, 2.0),
                candidate: observation(&recipe.candidate_script_digest, 1.9),
            }
        })
        .collect();
    let evidence = ScientificEvidence {
        schema_version: 1,
        experiment_id: Uuid::new_v4(),
        chain_epoch: 7,
        recipe_digest: commitment(&recipe).unwrap(),
        resource_id: "round-pod".into(),
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
            rationale: "synthetic verdict".into(),
            topic_id: recipe.topic.id.clone(),
            family: recipe.topic.metric.family,
        },
    };
    (pin, recipe, evidence, artifacts)
}

pub struct Publisher;
#[async_trait]
impl EvidencePublisher for Publisher {
    async fn publish(&self, doc: &PublicEvidence) -> Result<String, ResearchError> {
        Ok(commitment(doc)?)
    }
}
#[async_trait]
impl RoundPublisher for Publisher {
    async fn publish(&self, doc: &RoundPublication) -> Result<String, RoundError> {
        Ok(commitment(doc)?)
    }
}

pub async fn completed(f: &Fixture) -> (ResearchStore, PublicEvidence) {
    let (pin, recipe, evidence, artifacts) = scientific();
    let store = ResearchStore::new(f.database.app_pool().await.unwrap(), pin.clone());
    let digest = store.register_recipe(&recipe).await.unwrap();
    let request = CreateExperiment {
        id: evidence.experiment_id,
        account_id: f.account.id,
        recipe_digest: digest,
    };
    let experiment = f
        .store
        .create_experiment(
            &request,
            &signed(&request, "/v2/experiments", f.now().await, &SEED),
        )
        .await
        .unwrap();
    let lease = f
        .store
        .acquire(experiment.id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    let now = f.now().await;
    let quote = MachineQuote {
        schema_version: 1,
        id: Uuid::new_v4(),
        experiment_id: experiment.id,
        miner_hotkey: experiment.miner_hotkey.clone(),
        account_id: experiment.account_id,
        recipe_digest: experiment.recipe_digest,
        offer_id: "local-faux".into(),
        gpu_type: "local-faux".into(),
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
    let experiment = f.store.publish_quote(&lease, 0, &quote).await.unwrap();
    let experiment = f
        .store
        .consent(experiment.id, experiment.revision, &consent(&quote))
        .await
        .unwrap();
    let intent = f
        .store
        .intents(experiment.id, &experiment.miner_hotkey)
        .await
        .unwrap()[0]
        .id;
    f.store
        .begin_provision(&lease, experiment.revision, intent)
        .await
        .unwrap();
    f.store
        .record_provision(
            &lease,
            intent,
            &ProvisionResult::Confirmed {
                resource_id: evidence.resource_id.clone(),
            },
        )
        .await
        .unwrap();
    f.store
        .adopt_resource(&lease, experiment.revision + 1)
        .await
        .unwrap();
    let summary = store.record(&lease, &evidence, &artifacts).await.unwrap();
    store
        .publish(&summary.evidence_digest, &Publisher)
        .await
        .unwrap();
    let target = f
        .store
        .begin_cleanup(&lease, &evidence.resource_id)
        .await
        .unwrap();
    f.store
        .record_deletion(
            &lease,
            &evidence.resource_id,
            target.resource.deletion_id.unwrap(),
            &DeletionResult::Confirmed,
        )
        .await
        .unwrap();
    store
        .complete(&lease, &summary.evidence_digest)
        .await
        .unwrap();
    (store, summary)
}
