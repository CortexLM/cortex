#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]
//! Restart durability: submissions and scored runs must survive process death.
//! Without this the service silently discards every scored run on restart,
//! which is disqualifying for anything paying emissions.

use proof_score::MinerTopicRun;
use proof_store::{durable::DurableJournal, MemoryStore, Submission, SubmissionState};

fn submission(id: &str, hotkey: &str, topic: &str) -> Submission {
    Submission {
        id: id.to_owned(),
        topic_id: topic.to_owned(),
        miner_hotkey: hotkey.to_owned(),
        artifact_digest: "a".repeat(64),
        artifact_uri: None,
        claim: "durable claim".into(),
        declared_flops: 1_000,
        architecture: String::new(),
        inference_offer_id: "offer".into(),
        config_commitment: "c".repeat(64),
        manifest: proof_store::ArtifactManifest::default(),
        nonce: "n".into(),
        submission_digest: "d".repeat(64),
        state: SubmissionState::AwaitingAdmin,
        receipt_json: None,
        verdict: None,
        detail: None,
    }
}

fn run() -> MinerTopicRun {
    MinerTopicRun {
        pass: true,
        primary: Some(1.25),
        artifact_digest: "a".repeat(64),
        near_duplicate: false,
    }
}

#[tokio::test]
#[ignore = "requires disposable DATABASE_URL"]
async fn multi_instance_atomicity_identity_and_cancellation() {
    let database = db::test_pool().await.unwrap();
    let pool = database.app_pool().await.unwrap();
    let a = MemoryStore::new()
        .with_journal(DurableJournal::new(pool.clone()))
        .await
        .unwrap();
    let b = MemoryStore::new()
        .with_journal(DurableJournal::new(database.app_pool().await.unwrap()))
        .await
        .unwrap();
    let (one, two) = tokio::join!(
        a.finish_durable(submission("", "miner-a", "topic"), run()),
        b.finish_durable(submission("", "miner-b", "topic"), run())
    );
    let one = one.unwrap();
    assert_ne!(one.id, two.unwrap().id);
    assert_eq!(a.list_durable().await.unwrap().len(), 2);
    assert_eq!(
        b.get_durable(&one.id).await.unwrap().miner_hotkey,
        "miner-a"
    );
    assert_eq!(
        b.snapshot_durable()
            .await
            .unwrap()
            .scored_hotkeys()
            .unwrap()
            .len(),
        2
    );
    assert!(a.insert(one.clone()).is_err());
    assert!(a.record_topic_run("miner", "topic", run()).is_err());
    assert!(a.record_topic_score("miner", "topic", 1).is_err());
    assert!(a
        .record_topic_run_durable("miner", "topic", run())
        .await
        .is_err());
    assert!(a.get(&one.id).is_err());
    assert!(a.list().is_err());
    assert!(a.miner_runs("miner-a").is_err());
    assert!(a.scored_hotkeys().is_err());
    assert!(a.insert_durable(one.clone()).await.is_err());
    for key in [
        "miner_hotkey",
        "topic_id",
        "artifact_digest",
        "nonce",
        "claim",
        "submission_digest",
        "declared_flops",
        "manifest",
    ] {
        let mut changed = serde_json::to_value(&one).unwrap();
        changed[key] = match key {
            "declared_flops" => serde_json::json!(2000),
            "manifest" => serde_json::json!({"train_dataset_ids":["changed"]}),
            "artifact_digest" => serde_json::json!("b".repeat(64)),
            _ => serde_json::json!("changed"),
        };
        assert!(
            a.update_durable(serde_json::from_value(changed).unwrap(), run())
                .await
                .is_err(),
            "{key}"
        );
    }
    let mut updated = one.clone();
    updated.detail = Some("updated".into());
    a.update_durable(updated.clone(), run()).await.unwrap();
    assert_eq!(b.get_durable(&one.id).await.unwrap().detail, updated.detail);

    // Force failure after the submission SQL, proving transaction rollback.
    sqlx::query(
        "ALTER TABLE proof_topic_run ADD CONSTRAINT reject_test CHECK (primary_value <> 7)",
    )
    .execute(database.pool())
    .await
    .unwrap();
    let bad = MinerTopicRun {
        primary: Some(7.0),
        ..run()
    };
    assert!(a
        .finish_durable(submission("", "rollback", "topic"), bad.clone())
        .await
        .is_err());
    updated.detail = Some("must roll back".into());
    assert!(a.update_durable(updated, bad).await.is_err());
    assert_eq!(b.list_durable().await.unwrap().len(), 2);
    assert_eq!(
        b.get_durable(&one.id).await.unwrap().detail.as_deref(),
        Some("updated")
    );
    assert_eq!(
        b.snapshot_durable()
            .await
            .unwrap()
            .miner_runs("miner-a")
            .unwrap()["topic"]
            .primary,
        Some(1.25)
    );
    for primary in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(a
            .update_durable(
                one.clone(),
                MinerTopicRun {
                    primary: Some(primary),
                    ..run()
                }
            )
            .await
            .is_err());
        assert!(sqlx::query("UPDATE proof_topic_run SET primary_value = $1")
            .bind(primary)
            .execute(&pool)
            .await
            .is_err());
    }

    // The client task can disappear after COMMIT without hiding the row in a cache.
    let (sent, received) = tokio::sync::oneshot::channel();
    let writer = a.clone();
    let task = tokio::spawn(async move {
        let row = writer
            .finish_durable(submission("", "cancelled", "topic"), run())
            .await
            .unwrap();
        sent.send(row.id).unwrap();
        std::future::pending::<()>().await;
    });
    let id = received.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(b.get_durable(&id).await.unwrap().miner_hotkey, "cancelled");
    let reload = MemoryStore::new()
        .with_journal(DurableJournal::new(database.app_pool().await.unwrap()))
        .await
        .unwrap();
    assert_eq!(reload.list_durable().await.unwrap().len(), 3);
    pool.close().await;
    assert!(a.get_durable(&id).await.is_err());
    assert!(a
        .finish_durable(submission("", "closed", "topic"), run())
        .await
        .is_err());
    database.drop_schema().await.unwrap();
}
