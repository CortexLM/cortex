//! Strict evidence receiver: exact signed bytes keyed by evidence digest.
//! Identical replay acknowledges; any different bytes for a digest conflict.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use parity_scale_codec::Encode;
use proof_publication::{
    EvidencePublication, EvidenceReceipt, EVIDENCE_ROUTE, MAX_EVIDENCE_WIRE_BYTES,
};
use sqlx::Row;

use crate::{lock, Error, Receiver};

impl Receiver {
    pub(crate) fn evidence_router(self: &Arc<Self>) -> Router {
        Router::new()
            .route(EVIDENCE_ROUTE, post(post_evidence))
            .route(
                &format!("{EVIDENCE_ROUTE}/{{evidence_digest}}"),
                get(get_evidence),
            )
            .layer(DefaultBodyLimit::max(MAX_EVIDENCE_WIRE_BYTES))
            .with_state(self.clone())
    }

    /// The first accepted bytes for an evidence digest are final. A re-POST of
    /// the same content (same receipt digest, any valid Proof signature) is
    /// acknowledged; different content for the same digest is a conflict.
    ///
    /// # Errors
    /// Bad wire/signature, conflicting content for a stored digest or DB uncertainty.
    pub fn accept_evidence(&self, wire: &[u8]) -> Result<EvidenceReceipt, Error> {
        let document = EvidencePublication::from_wire(wire)?;
        document.verify(&self.config.proof_public_key)?;
        let receipt = document.receipt()?;
        let public = self.config.proof_public_key;
        let candidate = document;
        self.exec.run(move |pool| async move {
            let mut tx = pool.begin().await?;
            lock(&mut tx).await?;
            let bytes = candidate.encode();
            let expected = candidate.receipt()?;
            if let Some(existing) = fetch(&mut tx, &candidate.evidence_digest).await? {
                let stored = EvidencePublication::from_wire(&existing)?;
                stored.verify(&public)?;
                if stored.receipt()? != expected {
                    return Err(Error::Conflict);
                }
                tx.commit().await?;
                return Ok(());
            }
            sqlx::query(
                "INSERT INTO gateway_proof_evidence (evidence_digest, digest, signature, wire) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&candidate.evidence_digest)
            .bind(&expected.digest)
            .bind(&candidate.signature)
            .bind(&bytes)
            .execute(&mut *tx)
            .await?;
            if fetch(&mut tx, &candidate.evidence_digest).await? != Some(bytes) {
                return Err(Error::Unavailable);
            }
            tx.commit().await?;
            Ok(())
        })?;
        Ok(receipt)
    }

    /// # Errors
    /// Unknown digest, corrupt stored document or unavailable database.
    pub fn readback_evidence(&self, evidence_digest: &str) -> Result<Vec<u8>, Error> {
        if !proof_autonomy::is_digest(evidence_digest) {
            return Err(Error::Invalid);
        }
        let key = evidence_digest.to_owned();
        let wire = self.exec.run(move |pool| async move {
            let mut conn = pool.acquire().await?;
            fetch(&mut conn, &key).await?.ok_or(Error::Conflict)
        })?;
        let document = EvidencePublication::from_wire(&wire)?;
        document.verify(&self.config.proof_public_key)?;
        if document.evidence_digest != evidence_digest {
            return Err(Error::Unavailable);
        }
        Ok(wire)
    }
}

async fn fetch(conn: &mut sqlx::PgConnection, digest: &str) -> Result<Option<Vec<u8>>, Error> {
    let row = sqlx::query("SELECT wire FROM gateway_proof_evidence WHERE evidence_digest = $1")
        .bind(digest)
        .fetch_optional(conn)
        .await?;
    row.map(|row| row.try_get::<Vec<u8>, _>("wire").map_err(Error::from))
        .transpose()
}

async fn post_evidence(State(receiver): State<Arc<Receiver>>, bytes: Bytes) -> Response {
    match tokio::task::spawn_blocking(move || receiver.accept_evidence(&bytes)).await {
        Ok(Ok(receipt)) => ([(header::CACHE_CONTROL, "no-store")], Json(receipt)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(_) => Error::Unavailable.into_response(),
    }
}

async fn get_evidence(
    State(receiver): State<Arc<Receiver>>,
    Path(evidence_digest): Path<String>,
) -> Response {
    match tokio::task::spawn_blocking(move || receiver.readback_evidence(&evidence_digest)).await {
        Ok(Ok(bytes)) => (
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(_) => Error::Unavailable.into_response(),
    }
}
