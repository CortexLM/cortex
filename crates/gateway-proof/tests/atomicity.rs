#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use chain_live::FinalizedSnapshot;
use common::*;
use gateway_proof::{Error, FinalizedSource};
use parity_scale_codec::Encode;

struct Gate {
    block: u64,
    entered: tokio::sync::Notify,
    release: (Mutex<bool>, Condvar),
}

impl Gate {
    fn new(block: u64) -> Arc<Self> {
        Arc::new(Self {
            block,
            entered: tokio::sync::Notify::new(),
            release: (Mutex::new(false), Condvar::new()),
        })
    }
    fn open(&self) {
        *self.release.0.lock().unwrap() = true;
        self.release.1.notify_all();
    }
}

impl FinalizedSource for Gate {
    fn snapshot(&self, block: u64) -> Result<FinalizedSnapshot, Error> {
        if block == self.block {
            self.entered.notify_one();
            let (guard, _) = self
                .release
                .1
                .wait_timeout_while(
                    self.release.0.lock().unwrap(),
                    Duration::from_secs(5),
                    |released| !*released,
                )
                .unwrap();
            assert!(*guard, "test gate timed out");
        }
        Source::default().snapshot(block)
    }
}

#[tokio::test]
async fn timed_out_old_delivery_cannot_replace_a_newer_same_epoch_round() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let gate = Gate::new(360);
    let receiver = connect(&f.url, gate.clone(), config());
    let mut old = tokio::task::spawn_blocking(move || receiver.accept(&document(0, 10).encode()));
    gate.entered.notified().await;
    assert!(tokio::time::timeout(Duration::from_millis(30), &mut old)
        .await
        .is_err());
    // The client has timed out, but its server-side work may still complete.
    let next = document(1, 30).encode();
    f.receiver.accept(&next).unwrap();
    gate.open();
    assert!(matches!(old.await.unwrap(), Err(Error::Conflict)));
    assert_eq!(f.receiver.readback(1).unwrap(), next);
    let rounds: Vec<i64> = sqlx::query_scalar("SELECT round FROM gateway_proof_round")
        .fetch_all(f.db.pool())
        .await
        .unwrap();
    assert_eq!(rounds, vec![1]);
    f.close().await;
}

#[tokio::test]
async fn seal_lock_covers_chain_reads_signing_and_commit() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    f.receiver.accept(&document(0, 10).encode()).unwrap();
    f.bounty().await;
    let gate = Gate::new(360);
    let receiver = connect(&f.url, gate.clone(), config());
    let stores = receiver.stores();
    let old_seal = tokio::task::spawn_blocking(move || {
        gateway::seal_epoch(
            &chain(),
            &challenges(),
            stores.0.as_ref(),
            stores.1.as_ref(),
            &gateway::SealParams {
                epoch: EPOCH,
                netuid: 1,
                block_b: 360,
                gateway_secret: GATEWAY_SECRET,
                measurements_digest: [8; 32],
            },
        )
    });
    gate.entered.notified().await;
    let receiver = f.receiver.clone();
    let next = document(1, 30);
    let expected = next.encode();
    let mut delivery = tokio::task::spawn_blocking(move || receiver.accept(&next.encode()));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut delivery)
            .await
            .is_err(),
        "publication must wait for the pinned seal transaction"
    );
    gate.open();
    old_seal.await.unwrap().unwrap();
    delivery.await.unwrap().unwrap();
    assert_eq!(f.receiver.readback(1).unwrap(), expected);
    assert!(
        f.stores.1.latest_sealed().is_none(),
        "a completed but superseded seal must not be returned from cache"
    );
    assert!(f.seal(360).is_err());
    f.seal(720).unwrap();
    f.close().await;
}

#[tokio::test]
async fn app_role_cannot_rewrite_publication_order_or_seal_provenance() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let app = f.db.app_pool().await.unwrap();
    for (table, column) in [
        ("gateway_proof_config", "anchor_block"),
        ("gateway_proof_round", "round"),
        ("gateway_proof_seal", "round"),
    ] {
        assert!(
            sqlx::query(&format!(
                "UPDATE {table} SET {column} = {column} WHERE false"
            ))
            .execute(&app)
            .await
            .is_err(),
            "{table} must be append-only for base_app"
        );
        assert!(
            sqlx::query(&format!("DELETE FROM {table} WHERE false"))
                .execute(&app)
                .await
                .is_err(),
            "{table} may not be reset by base_app"
        );
    }
    f.close().await;
}

#[tokio::test]
async fn commit_failure_never_returns_a_receipt_or_phantom_seal() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let first = document(0, 10);
    f.receiver.accept(&first.encode()).unwrap();
    // Real deferred COMMIT failure, not a mocked transport error.
    sqlx::raw_sql(
        "CREATE FUNCTION reject_test_commit() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'synthetic commit failure'; END; $$; \
         CREATE CONSTRAINT TRIGGER reject_round_commit AFTER INSERT ON gateway_proof_round \
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_test_commit(); \
         CREATE CONSTRAINT TRIGGER reject_seal_commit AFTER INSERT ON epoch_bundle \
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_test_commit();",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        f.receiver.accept(&document(1, 30).encode()),
        Err(Error::Unavailable)
    ));
    assert_eq!(f.receiver.readback(0).unwrap(), first.encode());
    f.bounty().await;
    assert!(f.seal(360).is_err());
    assert!(f.stores.1.latest_sealed().is_none());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gateway_proof_seal")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "seal provenance must roll back with the bundle");
    sqlx::raw_sql(
        "DROP TRIGGER reject_round_commit ON gateway_proof_round; \
         DROP TRIGGER reject_seal_commit ON epoch_bundle;",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    f.receiver.accept(&document(1, 30).encode()).unwrap();
    f.seal(720).unwrap();
    f.close().await;
}
