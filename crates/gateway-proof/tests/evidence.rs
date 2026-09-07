#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use common::*;
use gateway_proof::Error;
use parity_scale_codec::Encode;
use proof_publication::{EvidencePublication, EvidenceReceipt, EVIDENCE_ROUTE};
use proof_research::PublicEvidence;

fn evidence(passed: bool) -> EvidencePublication {
    let mut document = EvidencePublication::unsigned(&PublicEvidence {
        schema_version: 1,
        evidence_digest: "a".repeat(64),
        recipe_digest: "b".repeat(64),
        repetitions: 3,
        primary_mean: 1.5,
        primary_standard_error: 0.25,
        passed,
    });
    document.sign(&SECRET).unwrap();
    document
}

#[tokio::test]
async fn evidence_is_exact_append_only_and_idempotent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let first = evidence(true);
    let path = format!("{EVIDENCE_ROUTE}/{}", first.evidence_digest);
    assert_eq!(request(&f.app, &path, None).await.0, 409);
    let (status, body) = request(&f.app, EVIDENCE_ROUTE, Some(first.encode())).await;
    assert_eq!(status, 200);
    let receipt: EvidenceReceipt = serde_json::from_slice(&body).unwrap();
    assert_eq!(receipt, first.receipt().unwrap());
    assert_eq!(request(&f.app, &path, None).await, (200, first.encode()));
    // Identical bytes and a freshly signed same-content retry both acknowledge,
    // while readback keeps serving the first stored bytes.
    assert_eq!(
        f.receiver.accept_evidence(&first.encode()).unwrap(),
        receipt
    );
    let mut resigned = first.clone();
    resigned.sign(&SECRET).unwrap();
    assert_ne!(resigned.signature, first.signature);
    assert_eq!(
        f.receiver.accept_evidence(&resigned.encode()).unwrap(),
        receipt
    );
    assert_eq!(request(&f.app, &path, None).await, (200, first.encode()));
    assert!(matches!(
        f.receiver.accept_evidence(&evidence(false).encode()),
        Err(Error::Conflict)
    ));
    assert_eq!(
        request(&f.app, EVIDENCE_ROUTE, Some(evidence(false).encode()))
            .await
            .0,
        409
    );
    assert_eq!(
        f.receiver
            .readback_evidence(&first.evidence_digest)
            .unwrap(),
        first.encode()
    );
    let mut forged = first.clone();
    forged.sign(&[99; 32]).unwrap();
    forged.evidence_digest = "c".repeat(64);
    assert_eq!(
        request(&f.app, EVIDENCE_ROUTE, Some(forged.encode()))
            .await
            .0,
        401
    );
    let mut trailing = first.encode();
    trailing.push(0);
    assert_eq!(request(&f.app, EVIDENCE_ROUTE, Some(trailing)).await.0, 400);
    assert_eq!(
        request(&f.app, EVIDENCE_ROUTE, Some(vec![0; 4097])).await.0,
        413
    );
    assert_eq!(
        request(&f.app, &format!("{EVIDENCE_ROUTE}/not-a-digest"), None)
            .await
            .0,
        400
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gateway_proof_evidence")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let restarted = connect(&f.url, f.source.clone(), config());
    assert_eq!(
        restarted.readback_evidence(&first.evidence_digest).unwrap(),
        first.encode()
    );
    f.close().await;
}
