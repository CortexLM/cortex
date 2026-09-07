use std::time::Duration;

use async_trait::async_trait;
use proof_autonomy::commitment;

use crate::{PublicEvidence, ResearchError, ResearchStore};

/// Cortex-controlled publisher. The destination is operator configuration,
/// never part of agent input. Upsert by evidence digest; repeated delivery of
/// the exact document must be idempotent. Return its verified remote commitment.
#[async_trait]
pub trait EvidencePublisher: Send + Sync {
    async fn publish(&self, document: &PublicEvidence) -> Result<String, ResearchError>;
}

impl ResearchStore {
    /// Claim one outbox item with a monotonic fence, publish only its allowlist,
    /// and acknowledge only the still-owned attempt. A crash leaves a retry.
    ///
    /// # Errors
    /// Concurrent claim, corrupt retained bytes, provider outage/mismatch or DB failure.
    pub async fn publish(
        &self,
        digest: &str,
        publisher: &dyn EvidencePublisher,
    ) -> Result<(), ResearchError> {
        let summary = self.verify_retained(digest).await?;
        let expected = commitment(&summary)?;
        let delivered: Option<String> = sqlx::query_scalar(
            "SELECT confirmed_digest FROM proof_publication WHERE evidence_digest = $1 AND delivered",
        ).bind(digest).fetch_optional(&self.pool).await?.flatten();
        if let Some(delivered) = delivered {
            return if delivered == expected {
                Ok(())
            } else {
                Err(ResearchError::Publication)
            };
        }
        let fence: i64 = sqlx::query_scalar(
            "UPDATE proof_publication SET fence = fence + 1, \
             expires_at = clock_timestamp() + interval '60 seconds' \
             WHERE evidence_digest = $1 AND NOT delivered AND expires_at <= clock_timestamp() \
             RETURNING fence",
        )
        .bind(digest)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ResearchError::Publication)?;
        let result =
            tokio::time::timeout(Duration::from_secs(30), publisher.publish(&summary)).await;
        if !matches!(result, Ok(Ok(ref receipt)) if receipt == &expected) {
            // Release only our attempt, without acknowledging an ambiguous
            // external write. A later retry uses exactly the same document id.
            sqlx::query(
                "UPDATE proof_publication SET expires_at = clock_timestamp() \
                 WHERE evidence_digest = $1 AND fence = $2 AND NOT delivered",
            )
            .bind(digest)
            .bind(fence)
            .execute(&self.pool)
            .await?;
            return Err(ResearchError::Publication);
        }
        let changed = sqlx::query(
            "UPDATE proof_publication SET delivered = true, confirmed_digest = $3 \
             WHERE evidence_digest = $1 AND fence = $2 AND NOT delivered \
             AND expires_at > clock_timestamp()",
        )
        .bind(digest)
        .bind(fence)
        .bind(expected)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(ResearchError::Publication);
        }
        Ok(())
    }
}
