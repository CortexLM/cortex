#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use axum::{
    body::Bytes,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use bundle::ScoreOrAbsence;
use parity_scale_codec::Encode;
use proof_publication::{Client, Receipt, RoundPublication, ROUTE};

const SECRET: [u8; 32] = [7; 32];

fn document() -> RoundPublication {
    let scores = BTreeMap::from([([1; 32], ScoreOrAbsence::Score { value: 3 })]);
    let leaves: Vec<_> = challenge_common::emit_signed_leaf_set(
        &SECRET,
        b"proof",
        7,
        &scores.keys().copied().collect(),
        &scores,
    )
    .unwrap()
    .into_values()
    .collect();
    let mut document = RoundPublication {
        round: 0,
        chain_epoch: 7,
        netuid: 1,
        block: 360,
        block_hash: "a".repeat(64),
        frozen_digest: "b".repeat(64),
        decision_digest: "c".repeat(64),
        leaves: leaves.encode(),
        signature: String::new(),
    };
    document.sign(&SECRET).unwrap();
    document
}

#[test]
fn envelope_binds_every_field_and_leaf_signature() {
    let public = challenge_common::public_key_from_secret(&SECRET).unwrap();
    let document = document();
    document.verify(&public).unwrap();
    assert_eq!(
        RoundPublication::from_wire(&document.encode()).unwrap(),
        document
    );
    assert!(document.verify(&[99; 32]).is_err());
    for index in 0..8 {
        let mut altered = document.clone();
        match index {
            0 => altered.round += 1,
            1 => altered.chain_epoch += 1,
            2 => altered.netuid += 1,
            3 => altered.block += 1,
            4 => altered.block_hash = "d".repeat(64),
            5 => altered.frozen_digest = "d".repeat(64),
            6 => altered.decision_digest = "d".repeat(64),
            _ => altered.leaves[20] ^= 1,
        }
        assert!(altered.verify(&public).is_err(), "field {index}");
    }
    let mut bad_leaf = document.clone();
    let mut leaves = document.verify(&public).unwrap();
    leaves[0].challenge_sig[0] ^= 1;
    bad_leaf.leaves = leaves.encode();
    bad_leaf.sign(&SECRET).unwrap();
    assert!(bad_leaf.verify(&public).is_err());
    let mut trailing = document.encode();
    trailing.push(0);
    assert!(RoundPublication::from_wire(&trailing).is_err());
    assert!(document
        .validate_roster(&public, 1, 0, &[[1; 32], [2; 32]])
        .is_err());
    assert!(document.validate_roster(&public, 1, 1, &[[1; 32]]).is_err());
}

#[test]
fn destinations_are_pinned_not_agent_controlled_redirects() {
    for url in [
        "http://example.com",
        "https://example.com/path",
        "https://u:p@example.com",
        "https://example.com/?token=x",
        "https://example.com/#fragment",
    ] {
        assert!(Client::new(url, [1; 32]).is_err(), "{url}");
    }
    assert!(Client::new("https://example.com", [1; 32]).is_ok());
    assert!(Client::new("http://127.0.0.1:1234", [1; 32]).is_ok());
}

async fn server(app: Router) -> (Client, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        Client::new(
            &format!("http://{address}"),
            challenge_common::public_key_from_secret(&SECRET).unwrap(),
        )
        .unwrap(),
        server,
    )
}

#[tokio::test]
async fn exact_retries_require_receipt_and_current_byte_readback() {
    let document = document();
    let receipt = document.receipt().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let ack = receipt.clone();
    let capture = seen.clone();
    let wire = document.encode();
    let app = Router::new()
        .route(
            ROUTE,
            post(move |bytes: Bytes| {
                capture.lock().unwrap().push(bytes.to_vec());
                let ack = ack.clone();
                async move { Json(ack) }
            }),
        )
        .route(
            &format!("{ROUTE}/{{round}}"),
            get(move || {
                let wire = wire.clone();
                async move { wire }
            }),
        );
    let (client, task) = server(app).await;
    for _ in 0..2 {
        assert_eq!(client.publish(&document).await.unwrap(), receipt.digest);
    }
    assert_eq!(
        *seen.lock().unwrap(),
        vec![document.encode(), document.encode()]
    );
    task.abort();

    for mode in 0..4 {
        let ack = if mode == 2 {
            Receipt {
                round: 0,
                digest: "f".repeat(64),
            }
        } else {
            receipt.clone()
        };
        let wire = if mode == 1 {
            document.encode()
        } else {
            vec![0]
        };
        let app = Router::new()
            .route(
                ROUTE,
                post(move || {
                    let ack = ack.clone();
                    async move {
                        (
                            if mode == 1 {
                                StatusCode::CONFLICT
                            } else {
                                StatusCode::OK
                            },
                            Json(ack),
                        )
                    }
                }),
            )
            .route(
                &format!("{ROUTE}/{{round}}"),
                get(move || {
                    let wire = wire.clone();
                    async move {
                        (
                            if mode == 3 {
                                StatusCode::CONFLICT
                            } else {
                                StatusCode::OK
                            },
                            wire,
                        )
                    }
                }),
            );
        let (client, task) = server(app).await;
        assert!(client.publish(&document).await.is_err(), "mode {mode}");
        task.abort();
    }
}
