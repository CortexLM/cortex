#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "../../proof-autonomy-pg/tests/common/mod.rs"]
mod common;
mod science;

use async_trait::async_trait;
use common::{signed, Fixture, SEED};
use proof_autonomy::{commitment, ExperimentState};
use proof_autonomy_pg::{CancelExperiment, ControllerLease};
use proof_research::*;
use science::{clean, metrics, running, scientific};
use uuid::Uuid;

#[test]
fn repeated_observations_preserve_topic_gates_and_public_allowlist() {
    let (pin, recipe, evidence, artifacts) = scientific();
    recipe.validate(&pin).unwrap();
    let summary = evidence.evaluate(&recipe, &artifacts).unwrap();
    assert!(summary.passed);
    assert_eq!(summary.repetitions, 3);
    assert!(summary.primary_standard_error < 1e-12);
    let public = serde_json::to_string(&summary).unwrap();
    for forbidden in [
        "private",
        "rationale",
        "resource_id",
        "science-pod",
        "account",
        "signature",
    ] {
        assert!(!public.contains(forbidden));
    }
    let mut failed = evidence.clone();
    failed.measurements[0].candidate.metrics = metrics(2.1);
    assert!(!failed.evaluate(&recipe, &artifacts).unwrap().passed);
    failed = evidence.clone();
    failed
        .contamination_hits
        .push("controller-found-contamination".into());
    assert!(!failed.evaluate(&recipe, &artifacts).unwrap().passed);
    failed = evidence.clone();
    failed.verdict.claim_holds_public = false;
    assert!(!failed.evaluate(&recipe, &artifacts).unwrap().passed);
}

#[test]
fn incomplete_or_tampered_scientific_records_never_become_credit() {
    let (pin, mut recipe, evidence, artifacts) = scientific();
    let mut bad = evidence.clone();
    bad.measurements.pop();
    assert!(bad.evaluate(&recipe, &artifacts).is_err());
    bad = evidence.clone();
    bad.measurements[0].candidate.metrics.holdout_nll = f64::NAN;
    assert!(bad.evaluate(&recipe, &artifacts).is_err());
    bad = evidence.clone();
    bad.measurements[0].baseline.metrics = metrics(3.0);
    assert!(bad.evaluate(&recipe, &artifacts).is_err());
    bad = evidence.clone();
    bad.measurements[0].candidate.exit_code = 1;
    assert!(bad.evaluate(&recipe, &artifacts).is_err());
    let mut altered = artifacts.clone();
    altered.values_mut().next().unwrap().push(b'!');
    assert!(evidence.evaluate(&recipe, &altered).is_err());
    altered = artifacts.clone();
    altered.remove(&recipe.candidate_script_digest);
    assert!(evidence.evaluate(&recipe, &altered).is_err());
    recipe.topic.epsilon_nll = 0.0;
    assert!(recipe.validate(&pin).is_err());
}

struct LocalPublisher {
    available: bool,
}
#[async_trait]
impl EvidencePublisher for LocalPublisher {
    async fn publish(&self, document: &PublicEvidence) -> Result<String, ResearchError> {
        if !self.available {
            return Err(ResearchError::Publication);
        }
        Ok(commitment(document)?)
    }
}

#[tokio::test]
async fn durable_evidence_requires_retained_bytes_and_confirmed_publication() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (pin, recipe, evidence, artifacts) = scientific();
    let store = ResearchStore::new(f.database.app_pool().await.unwrap(), pin.clone());
    let digest = store.register_recipe(&recipe).await.unwrap();
    let lease = running(&f, &pin, digest, evidence.experiment_id).await;
    let summary = store.record(&lease, &evidence, &artifacts).await.unwrap();
    assert!(store.rewardable(&summary.evidence_digest).await.is_err());
    assert!(store
        .publish(
            &summary.evidence_digest,
            &LocalPublisher { available: false }
        )
        .await
        .is_err());
    assert!(store.rewardable(&summary.evidence_digest).await.is_err());
    store
        .publish(
            &summary.evidence_digest,
            &LocalPublisher { available: true },
        )
        .await
        .unwrap();
    assert!(store.rewardable(&summary.evidence_digest).await.is_err());
    assert!(store
        .complete(&lease, &summary.evidence_digest)
        .await
        .is_err());
    clean(&f, &lease).await;
    let completed = store
        .complete(&lease, &summary.evidence_digest)
        .await
        .unwrap();
    assert_eq!(completed.state, ExperimentState::Completed);
    assert!(
        store
            .rewardable(&summary.evidence_digest)
            .await
            .unwrap()
            .passed
    );
    let restarted = ResearchStore::new(f.database.app_pool().await.unwrap(), pin);
    assert!(restarted.rewardable(&summary.evidence_digest).await.is_ok());
    assert!(restarted
        .evidence(evidence.experiment_id, &"f".repeat(64))
        .await
        .is_err());
    sqlx::query("UPDATE proof_evidence_artifact SET bytes = '\\x00' WHERE evidence_digest = $1")
        .bind(&summary.evidence_digest)
        .execute(f.database.pool())
        .await
        .unwrap();
    assert!(restarted
        .rewardable(&summary.evidence_digest)
        .await
        .is_err());
    f.close().await;
}

async fn recorded(f: &Fixture, passed: bool) -> (ResearchStore, ControllerLease, PublicEvidence) {
    let (pin, recipe, mut evidence, artifacts) = scientific();
    evidence.verdict.claim_holds_public = passed;
    let store = ResearchStore::new(f.database.app_pool().await.unwrap(), pin.clone());
    let digest = store.register_recipe(&recipe).await.unwrap();
    let lease = running(f, &pin, digest, evidence.experiment_id).await;
    let summary = store.record(&lease, &evidence, &artifacts).await.unwrap();
    assert_eq!(
        store.record(&lease, &evidence, &artifacts).await.unwrap(),
        summary
    );
    (store, lease, summary)
}

#[tokio::test]
async fn scientific_rejection_is_retained_but_never_rewardable() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (store, lease, summary) = recorded(&f, false).await;
    assert!(!summary.passed);
    clean(&f, &lease).await;
    assert!(store
        .complete(&lease, &summary.evidence_digest)
        .await
        .is_err());
    store
        .publish(
            &summary.evidence_digest,
            &LocalPublisher { available: true },
        )
        .await
        .unwrap();
    let rejected = store
        .complete(&lease, &summary.evidence_digest)
        .await
        .unwrap();
    assert_eq!(rejected.state, ExperimentState::Rejected);
    assert!(store.rewardable(&summary.evidence_digest).await.is_err());
    assert_eq!(
        store
            .evidence(lease.experiment_id, &f.account.miner_hotkey)
            .await
            .unwrap(),
        summary
    );
    f.close().await;
}

#[tokio::test]
async fn cancellation_wins_over_scientific_completion() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (store, lease, summary) = recorded(&f, true).await;
    store
        .publish(
            &summary.evidence_digest,
            &LocalPublisher { available: true },
        )
        .await
        .unwrap();
    let experiment = f
        .store
        .experiment(lease.experiment_id, &f.account.miner_hotkey)
        .await
        .unwrap();
    let request = CancelExperiment {
        experiment_id: experiment.id,
        revision: experiment.revision,
    };
    f.store
        .cancel(
            &request,
            &signed(
                &request,
                &format!("/v2/experiments/{}/cancel", experiment.id),
                f.now().await,
                &SEED,
            ),
        )
        .await
        .unwrap();
    assert!(store
        .complete(&lease, &summary.evidence_digest)
        .await
        .is_err());
    let lease = f
        .store
        .acquire(lease.experiment_id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    clean(&f, &lease).await;
    assert!(store
        .complete(&lease, &summary.evidence_digest)
        .await
        .is_err());
    assert_eq!(
        f.store.finish_cleanup(&lease).await.unwrap().state,
        ExperimentState::Cancelled
    );
    assert!(store.rewardable(&summary.evidence_digest).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn corrupted_relational_metadata_and_public_receipts_fail_closed() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (store, _, summary) = recorded(&f, true).await;
    sqlx::query("UPDATE proof_publication SET delivered = true, confirmed_digest = $2 WHERE evidence_digest = $1")
        .bind(&summary.evidence_digest).bind("f".repeat(64)).execute(f.database.pool()).await.unwrap();
    assert!(store
        .publish(
            &summary.evidence_digest,
            &LocalPublisher { available: true }
        )
        .await
        .is_err());
    let other = f.create().await;
    sqlx::query("UPDATE proof_scientific_evidence SET experiment_id = $2 WHERE digest = $1")
        .bind(&summary.evidence_digest)
        .bind(other.id)
        .execute(f.database.pool())
        .await
        .unwrap();
    assert!(store
        .evidence(other.id, &f.account.miner_hotkey)
        .await
        .is_err());
    let app = f.database.app_pool().await.unwrap();
    assert!(
        sqlx::query("UPDATE proof_publication SET evidence_digest = evidence_digest")
            .execute(&app)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE proof_evidence_artifact SET bytes = bytes")
            .execute(&app)
            .await
            .is_err()
    );
    f.close().await;
}

struct MismatchedPublisher;
#[async_trait]
impl EvidencePublisher for MismatchedPublisher {
    async fn publish(&self, _: &PublicEvidence) -> Result<String, ResearchError> {
        Ok("f".repeat(64))
    }
}

#[tokio::test]
async fn mismatched_receipt_can_retry_but_delivery_is_idempotent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (store, _, summary) = recorded(&f, true).await;
    assert!(store
        .publish(&summary.evidence_digest, &MismatchedPublisher)
        .await
        .is_err());
    store
        .publish(
            &summary.evidence_digest,
            &LocalPublisher { available: true },
        )
        .await
        .unwrap();
    // Already delivered: the unavailable transport must not be invoked again.
    store
        .publish(
            &summary.evidence_digest,
            &LocalPublisher { available: false },
        )
        .await
        .unwrap();
    let fence: i64 = sqlx::query_scalar("SELECT fence FROM proof_publication")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(fence, 2);
    f.close().await;
}

struct HeldPublisher {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl EvidencePublisher for HeldPublisher {
    async fn publish(&self, document: &PublicEvidence) -> Result<String, ResearchError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(commitment(document)?)
    }
}

#[tokio::test]
async fn publication_takeover_fences_late_acknowledgement() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (store, _, summary) = recorded(&f, true).await;
    let publisher = std::sync::Arc::new(HeldPublisher {
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let old = {
        let store = store.clone();
        let digest = summary.evidence_digest.clone();
        let publisher = publisher.clone();
        tokio::spawn(async move { store.publish(&digest, publisher.as_ref()).await })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        publisher.started.notified(),
    )
    .await
    .unwrap();
    assert!(store
        .publish(
            &summary.evidence_digest,
            &LocalPublisher { available: true }
        )
        .await
        .is_err());
    sqlx::query(
        "UPDATE proof_publication SET expires_at = clock_timestamp() - interval '1 second'",
    )
    .execute(f.database.pool())
    .await
    .unwrap();
    store
        .publish(
            &summary.evidence_digest,
            &LocalPublisher { available: true },
        )
        .await
        .unwrap();
    publisher.release.notify_one();
    assert!(old.await.unwrap().is_err());
    let fence: i64 = sqlx::query_scalar("SELECT fence FROM proof_publication")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(fence, 2);
    f.close().await;
}

#[tokio::test]
async fn resource_expiry_while_artifact_insert_waits_rolls_back_all_evidence() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (pin, recipe, evidence, artifacts) = scientific();
    let store = ResearchStore::new(f.database.app_pool().await.unwrap(), pin.clone());
    let digest = store.register_recipe(&recipe).await.unwrap();
    let lease = running(&f, &pin, digest, evidence.experiment_id).await;
    sqlx::query("UPDATE proof_resource SET authorized_until = ceil(extract(epoch FROM clock_timestamp()))::bigint + 2")
        .execute(f.database.pool()).await.unwrap();
    let mut blocker = f.database.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE proof_evidence_artifact IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let writing = tokio::spawn(async move { store.record(&lease, &evidence, &artifacts).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let contended: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation = 'proof_evidence_artifact'::regclass AND NOT granted)",
            ).fetch_one(f.database.pool()).await.unwrap();
            if contended { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    sqlx::query("SELECT pg_sleep(3)")
        .execute(f.database.pool())
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    assert!(writing.await.unwrap().is_err());
    for table in [
        "proof_scientific_evidence",
        "proof_evidence_artifact",
        "proof_publication",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(f.database.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    f.close().await;
}

#[tokio::test]
async fn stale_controller_cannot_record_evidence_or_enqueue_publication() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (pin, recipe, evidence, artifacts) = scientific();
    let store = ResearchStore::new(f.database.app_pool().await.unwrap(), pin.clone());
    let digest = store.register_recipe(&recipe).await.unwrap();
    let lease = running(&f, &pin, digest, evidence.experiment_id).await;
    f.expire(&lease).await;
    assert!(store.record(&lease, &evidence, &artifacts).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_publication")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    f.close().await;
}
