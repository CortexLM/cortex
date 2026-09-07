#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use axum::{
    body::Bytes,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use parity_scale_codec::Encode;
use proof_autonomy::commitment;
use proof_publication::{
    EvidencePublication, EvidenceReceipt, GatewayEvidencePublisher, EVIDENCE_ROUTE,
};
use proof_research::PublicEvidence;

const SECRET: [u8; 32] = [7; 32];

fn public() -> [u8; 32] {
    challenge_common::public_key_from_secret(&SECRET).unwrap()
}

fn evidence() -> PublicEvidence {
    PublicEvidence {
        schema_version: 1,
        evidence_digest: "a".repeat(64),
        recipe_digest: "b".repeat(64),
        repetitions: 3,
        primary_mean: 1.25,
        primary_standard_error: 0.5,
        passed: true,
    }
}

fn signed() -> EvidencePublication {
    let mut document = EvidencePublication::unsigned(&evidence());
    document.sign(&SECRET).unwrap();
    document
}

#[test]
fn envelope_binds_every_public_field_and_matches_store_commitment() {
    let document = signed();
    document.verify(&public()).unwrap();
    assert_eq!(document.document(), evidence());
    assert_eq!(
        document.receipt().unwrap().digest,
        commitment(&evidence()).unwrap()
    );
    assert_eq!(
        EvidencePublication::from_wire(&document.encode()).unwrap(),
        document
    );
    assert!(document.verify(&[99; 32]).is_err());
    for index in 0..8 {
        let mut altered = document.clone();
        match index {
            0 => altered.schema_version = 2,
            1 => altered.evidence_digest = "c".repeat(64),
            2 => altered.recipe_digest = "c".repeat(64),
            3 => altered.repetitions += 1,
            4 => altered.primary_mean_bits ^= 1,
            5 => altered.primary_standard_error_bits ^= 1,
            6 => altered.passed = false,
            _ => altered.signature = altered.signature.to_ascii_uppercase(),
        }
        assert!(altered.verify(&public()).is_err(), "field {index}");
    }
    let mut nan = EvidencePublication::unsigned(&PublicEvidence {
        primary_mean: f64::NAN,
        ..evidence()
    });
    nan.sign(&SECRET).unwrap();
    assert!(nan.verify(&public()).is_err());
    let mut trailing = document.encode();
    trailing.push(0);
    assert!(EvidencePublication::from_wire(&trailing).is_err());
    assert!(GatewayEvidencePublisher::new("http://127.0.0.1:1", public(), [8; 32]).is_err());
    assert!(GatewayEvidencePublisher::new("http://example.com", public(), SECRET).is_err());
}

async fn server(app: Router) -> (GatewayEvidencePublisher, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        GatewayEvidencePublisher::new(&format!("http://{address}"), public(), SECRET).unwrap(),
        server,
    )
}

#[tokio::test]
async fn delivery_requires_exact_receipt_and_byte_readback() {
    let document = signed();
    let receipt = document.receipt().unwrap();
    // Each delivery is freshly signed (randomized sr25519); the echo server
    // returns exactly what it first stored, as the durable receiver does.
    let seen: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = seen.clone();
    let stored = seen.clone();
    let ack = receipt.clone();
    let app = Router::new()
        .route(
            EVIDENCE_ROUTE,
            post(move |bytes: Bytes| {
                capture.lock().unwrap().push(bytes.to_vec());
                let ack = ack.clone();
                async move { Json(ack) }
            }),
        )
        .route(
            &format!("{EVIDENCE_ROUTE}/{{digest}}"),
            get(move || {
                let wire = stored.lock().unwrap().first().cloned().unwrap();
                async move { wire }
            }),
        );
    let (client, task) = server(app).await;
    for _ in 0..2 {
        assert_eq!(client.deliver(&evidence()).await.unwrap(), receipt.digest);
    }
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_ne!(seen[0], seen[1], "each delivery is freshly signed");
    for wire in &seen {
        let sent = EvidencePublication::from_wire(wire).unwrap();
        sent.verify(&public()).unwrap();
        assert_eq!(sent.document(), evidence());
    }
    task.abort();

    for mode in 0..5 {
        let ack = if mode == 2 {
            EvidenceReceipt {
                evidence_digest: "a".repeat(64),
                digest: "f".repeat(64),
            }
        } else {
            receipt.clone()
        };
        let wire = match mode {
            1 => document.encode(),
            4 => {
                let mut other = EvidencePublication::unsigned(&PublicEvidence {
                    passed: false,
                    ..evidence()
                });
                other.sign(&SECRET).unwrap();
                other.encode()
            }
            _ => vec![0],
        };
        let app = Router::new()
            .route(
                EVIDENCE_ROUTE,
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
                &format!("{EVIDENCE_ROUTE}/{{digest}}"),
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
        assert!(client.deliver(&evidence()).await.is_err(), "mode {mode}");
        task.abort();
    }
}
