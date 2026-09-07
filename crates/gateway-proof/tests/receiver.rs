#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use std::sync::atomic::Ordering;

use bundle::LocalTrustRoot;
use common::*;
use gateway_proof::{Error, Receiver};
use parity_scale_codec::Encode;
use proof_publication::{Client, ROUTE};
use serde_json::Value;

#[tokio::test]
async fn authenticated_batch_to_pinned_seal_restart_and_same_epoch_supersession() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let first = document(0, 10);
    assert_eq!(request(&f.app, ROUTE, Some(first.encode())).await.0, 200);
    let receipt = f.receiver.accept(&first.encode()).unwrap();
    assert_eq!(receipt, first.receipt().unwrap());
    assert_eq!(f.receiver.readback(0).unwrap(), first.encode());
    assert!(f.seal(360).is_err(), "missing bounty must fail D24");
    f.bounty().await;
    assert!(
        f.seal(361).is_err(),
        "same epoch cannot select a different block"
    );
    let seal = f.seal(360).unwrap();
    bundle::verify_bundle(
        &seal,
        &chain(),
        &LocalTrustRoot {
            challenges: challenges(),
            measurements_digest: [8; 32],
        },
    )
    .unwrap();
    assert_eq!(
        seal.body.final_vector,
        validator::independent_python_weights(&seal.body)
            .unwrap()
            .final_vector
    );
    assert_eq!(f.seal(360).unwrap().encode_bytes(), seal.encode_bytes());
    assert_eq!(f.stores.1.seal_record(EPOCH).unwrap().revision, 1);
    let latest: Value =
        serde_json::from_slice(&request(&f.app, "/v1/weights/latest", None).await.1).unwrap();
    assert_eq!(latest["sealed"], true);

    // Retry does not consult historical chain state after exact durable acceptance.
    f.source.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(f.receiver.accept(&first.encode()).unwrap(), receipt);
    assert!(f.receiver.accept(&document(1, 30).encode()).is_err());
    f.source.unavailable.store(false, Ordering::SeqCst);
    let restarted = connect(&f.url, f.source.clone(), config());
    assert_eq!(restarted.readback(0).unwrap(), first.encode());
    assert_eq!(
        restarted.stores().1.get_by_epoch(EPOCH).unwrap(),
        seal.encode_bytes()
    );
    assert!(Receiver::is_enabled(&f.db.app_pool().await.unwrap())
        .await
        .unwrap());

    let next = document(1, 30); // A different research round in the SAME chain epoch.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = f.app.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = Client::new(&format!("http://{address}"), config().proof_public_key).unwrap();
    assert_eq!(
        client.publish(&next).await.unwrap(),
        next.receipt().unwrap().digest
    );
    assert!(matches!(
        restarted.accept(&first.encode()),
        Err(Error::Conflict)
    ));
    assert!(client.publish(&first).await.is_err());
    assert!(matches!(restarted.readback(0), Err(Error::Conflict)));
    assert!(matches!(
        f.receiver.accept(&document(1, 31).encode()),
        Err(Error::Conflict)
    ));
    let mut resigned = next.clone();
    resigned.sign(&SECRET).unwrap();
    assert_ne!(resigned.signature, next.signature);
    assert!(matches!(
        f.receiver.accept(&resigned.encode()),
        Err(Error::Conflict)
    ));
    // No cache/LKG seal is served while the newer publication is unsealed.
    assert!(restarted.stores().1.latest_sealed().is_none());
    assert!(f.stores.1.get_by_root(&seal.body.merkle_root).is_none());
    let latest: Value =
        serde_json::from_slice(&request(&f.app, "/v1/weights/latest", None).await.1).unwrap();
    assert_eq!(latest["sealed"], false);
    assert!(
        f.seal(360).is_err(),
        "old block may not reseal the new batch"
    );
    let second_seal = f.seal(720).unwrap();
    assert_eq!(second_seal.body.block_b, 720);
    assert_ne!(seal.body.final_vector, second_seal.body.final_vector);
    assert_eq!(f.stores.1.seal_record(EPOCH).unwrap().revision, 2);
    assert_eq!(
        restarted.stores().1.get_by_epoch(EPOCH).unwrap(),
        second_seal.encode_bytes()
    );
    task.abort();
    f.close().await;
}

#[tokio::test]
async fn admin_seal_defaults_to_round_pin_and_ingress_has_bounded_exact_wire() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    assert_eq!(
        request(
            &f.app,
            ROUTE,
            Some(vec![0; proof_publication::MAX_WIRE_BYTES + 1])
        )
        .await
        .0,
        413
    );
    let mut trailing = document(0, 10).encode();
    trailing.push(0);
    assert_eq!(request(&f.app, ROUTE, Some(trailing)).await.0, 400);
    assert_eq!(
        request(&f.app, "/v2/weights/proof/unknown", None).await.0,
        404
    );
    f.receiver.accept(&document(0, 10).encode()).unwrap();
    f.bounty().await;
    // Local synthetic gateway key only; this is not a live gateway or reward write.
    std::env::set_var("BASE_GATEWAY_SK", hex::encode(GATEWAY_SECRET));
    let response = request(
        &f.app,
        "/v1/admin/seal",
        Some(serde_json::to_vec(&serde_json::json!({"epoch": EPOCH})).unwrap()),
    )
    .await;
    assert_eq!(response.0, 200);
    let bytes = f.stores.1.get_by_epoch(EPOCH).unwrap();
    assert_eq!(
        bundle::EpochBundleV1::decode_bytes(&bytes)
            .unwrap()
            .body
            .block_b,
        360
    );
    let latest: Value =
        serde_json::from_slice(&request(&f.app, "/v1/weights/latest", None).await.1).unwrap();
    assert_eq!(latest["sealed"], true);
    f.close().await;
}

#[tokio::test]
async fn receiver_rejects_bad_auth_metadata_roster_and_partial_batches_atomically() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    for case in 0..10 {
        let mut d = document(0, 10);
        match case {
            0 => {
                d.sign(&[99; 32]).unwrap();
            }
            1 => {
                d.block_hash = "f".repeat(64);
                d.sign(&SECRET).unwrap();
            }
            2 => {
                d.chain_epoch += 1;
                d.sign(&SECRET).unwrap();
            }
            3 => {
                d.netuid += 1;
                d.sign(&SECRET).unwrap();
            }
            4 => {
                d.block += 1;
                d.sign(&SECRET).unwrap();
            }
            5 => {
                let mut batch = leaves(b"proof", 10);
                batch.pop();
                d.leaves = batch.encode();
                d.sign(&SECRET).unwrap();
            }
            6 => {
                let mut batch = leaves(b"proof", 10);
                batch.push(batch[0].clone());
                d.leaves = batch.encode();
                d.sign(&SECRET).unwrap();
            }
            7 => {
                d.leaves = leaves(b"bounty", 10).encode();
                d.sign(&SECRET).unwrap();
            }
            8 => {
                let mut batch = leaves(b"proof", 10);
                batch[1].challenge_sig[0] ^= 1;
                d.leaves = batch.encode();
                d.sign(&SECRET).unwrap();
            }
            _ => {
                d.round = 50;
                d.block = 18_360;
                d.block_hash = "e".repeat(64);
                d.sign(&SECRET).unwrap();
            }
        }
        let response = request(&f.app, ROUTE, Some(d.encode())).await;
        assert_ne!(response.0, 200, "case {case}");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gateway_proof_round")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "case {case} must not partially persist");
    }
    assert_eq!(
        request(&f.app, ROUTE, Some(document(0, 10).encode()))
            .await
            .0,
        200
    );
    f.close().await;
}

#[tokio::test]
async fn legacy_replay_and_late_legacy_seals_cannot_bypass_v2_fencing() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    f.receiver.accept(&document(1, 30).encode()).unwrap();
    let leaf = &leaves(b"proof", 10)[1];
    let legacy: gateway::RawWeightRequest = serde_json::from_value(wire(leaf)).unwrap();
    let response = request(
        &f.app,
        "/v1/weights/raw",
        Some(serde_json::to_vec(&wire(leaf)).unwrap()),
    )
    .await;
    assert_eq!(response.0, 503);
    // Simulate a pre-v2 gateway still holding its own Postgres store handle.
    let old_stores = gateway_store_pg::stores(&f.url).unwrap();
    assert!(gateway::accept_raw_weight(&challenges(), old_stores.0.as_ref(), &legacy).is_err());
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM raw_weight_snapshot WHERE challenge_id = 'proof'")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
    f.bounty().await;
    let seal = f.seal(720).unwrap();
    // Infallible legacy interface may return bytes, but no late revision can land.
    old_stores.1.put_revision(EPOCH, seal.encode_bytes());
    assert_eq!(f.stores.1.seal_record(EPOCH).unwrap().revision, 1);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM epoch_bundle")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let mut changed = config();
    changed.anchor_block = 1;
    assert!(Receiver::connect(
        &f.url,
        f.source.clone(),
        std::sync::Arc::new(validator_sync::SyncChain::new(chain())),
        changed,
        challenges()
    )
    .is_err());
    f.close().await;
}
