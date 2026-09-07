//! Write-through durability for submissions and scored runs.
//!
//! `MemoryStore` alone loses every scored run on restart, which is
//! disqualifying for a service that pays emissions. Topics, holdouts and
//! sealed baselines are reloaded from operator files at startup, so only two
//! things genuinely need a journal: the submissions the service accepted and
//! the per-topic runs payout reads.
//!
//! Postgres owns identities and atomic submission/payout commits. Reads use an
//! explicit MVCC snapshot, never a long-lived process cache.

use proof_score::MinerTopicRun;
use sqlx::{PgPool, Row};

use crate::{StoreError, Submission};

/// Postgres-backed journal for the state that must survive a restart.
#[derive(Clone)]
pub struct DurableJournal {
    pool: PgPool,
}

impl DurableJournal {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a submission, or update only its mutable terminal fields, and
    /// record its payout run in the same transaction.
    pub(crate) async fn commit(
        &self,
        mut row: Submission,
        run: &MinerTopicRun,
        update: bool,
    ) -> Result<Submission, StoreError> {
        {
            if run.primary.is_some_and(|v| !v.is_finite())
                || run.artifact_digest != row.artifact_digest
                || row.verdict.as_ref().is_some_and(|v| v.pass != run.pass)
            {
                return Err(StoreError::Illegal("invalid topic run".into()));
            }
        }
        let mut tx = self.pool.begin().await.map_err(|_| StoreError::Backend)?;
        let generated = row.id.is_empty();
        if update && generated {
            return Err(StoreError::Illegal("update requires id".into()));
        }
        loop {
            if generated {
                let n: i64 = sqlx::query_scalar("SELECT nextval('proof_submission_id_seq')")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|_| StoreError::Backend)?;
                row.id = format!("pf_{n:016x}");
            }
            let document = serde_json::to_value(&row).map_err(|_| StoreError::Backend)?;
            let decoded: Submission = serde_json::from_value(document.clone())
                .map_err(|_| StoreError::Illegal("non-finite submission metric".into()))?;
            if decoded.verdict != row.verdict {
                return Err(StoreError::Illegal("non-finite submission metric".into()));
            }
            let result = if update {
                sqlx::query(
                    "UPDATE proof_submission SET document = $5, updated_at = now() \
                     WHERE id = $1 AND topic_id = $2 AND miner_hotkey = $3 \
                     AND artifact_digest = $4 AND \
                     document - ARRAY['state','receipt_json','verdict','detail'] = \
                     $5::jsonb - ARRAY['state','receipt_json','verdict','detail']",
                )
            } else {
                sqlx::query(
                    "INSERT INTO proof_submission (id, topic_id, miner_hotkey, artifact_digest, document) \
                     VALUES ($1,$2,$3,$4,$5) ON CONFLICT (id) DO NOTHING",
                )
            }.bind(&row.id).bind(&row.topic_id).bind(&row.miner_hotkey)
                .bind(&row.artifact_digest).bind(document)
                .execute(&mut *tx).await.map_err(|_| StoreError::Backend)?;
            if result.rows_affected() == 1 {
                break;
            }
            if !generated {
                return Err(StoreError::Illegal(
                    "duplicate id or immutable identity mismatch".into(),
                ));
            }
        }
        {
            sqlx::query(
                "INSERT INTO proof_topic_run \
                 (miner_hotkey, topic_id, pass, primary_value, artifact_digest, near_duplicate) \
                 VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (miner_hotkey, topic_id) DO UPDATE SET \
                 pass = EXCLUDED.pass, primary_value = EXCLUDED.primary_value, \
                 artifact_digest = EXCLUDED.artifact_digest, \
                 near_duplicate = EXCLUDED.near_duplicate, updated_at = now()",
            )
            .bind(&row.miner_hotkey)
            .bind(&row.topic_id)
            .bind(run.pass)
            .bind(run.primary)
            .bind(&run.artifact_digest)
            .bind(run.near_duplicate)
            .execute(&mut *tx)
            .await
            .map_err(|_| StoreError::Backend)?;
        }
        tx.commit().await.map_err(|_| StoreError::Backend)?;
        Ok(row)
    }

    /// Read submissions and payouts from one MVCC snapshot.
    pub async fn snapshot(
        &self,
    ) -> Result<(Vec<Submission>, Vec<(String, String, MinerTopicRun)>), StoreError> {
        let mut tx = self.pool.begin().await.map_err(|_| StoreError::Backend)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|_| StoreError::Backend)?;
        let submissions = Self::read_submissions(&mut tx).await?;
        let runs = Self::read_runs(&mut tx).await?;
        tx.commit().await.map_err(|_| StoreError::Backend)?;
        Ok((submissions, runs))
    }

    /// Every retained submission, oldest first, for reload at startup.
    ///
    /// # Errors
    /// Database unavailable or a stored document no longer decodes.
    async fn read_submissions(
        connection: &mut sqlx::PgConnection,
    ) -> Result<Vec<Submission>, StoreError> {
        let rows = sqlx::query("SELECT document FROM proof_submission ORDER BY created_at, id")
            .fetch_all(connection)
            .await
            .map_err(|_| StoreError::Backend)?;
        rows.into_iter()
            .map(|row| {
                let value: serde_json::Value =
                    row.try_get("document").map_err(|_| StoreError::Backend)?;
                serde_json::from_value(value).map_err(|_| StoreError::Backend)
            })
            .collect()
    }

    /// Every retained attempt as `(hotkey, topic_id, run)`.
    ///
    /// # Errors
    /// Database unavailable.
    async fn read_runs(
        connection: &mut sqlx::PgConnection,
    ) -> Result<Vec<(String, String, MinerTopicRun)>, StoreError> {
        let rows = sqlx::query(
            "SELECT miner_hotkey, topic_id, pass, primary_value, artifact_digest, near_duplicate \
             FROM proof_topic_run",
        )
        .fetch_all(connection)
        .await
        .map_err(|_| StoreError::Backend)?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("miner_hotkey")
                        .map_err(|_| StoreError::Backend)?,
                    row.try_get("topic_id").map_err(|_| StoreError::Backend)?,
                    MinerTopicRun {
                        pass: row.try_get("pass").map_err(|_| StoreError::Backend)?,
                        primary: row
                            .try_get("primary_value")
                            .map_err(|_| StoreError::Backend)?,
                        artifact_digest: row
                            .try_get("artifact_digest")
                            .map_err(|_| StoreError::Backend)?,
                        near_duplicate: row
                            .try_get("near_duplicate")
                            .map_err(|_| StoreError::Backend)?,
                    },
                ))
            })
            .collect()
    }
}
