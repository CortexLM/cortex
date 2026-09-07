use std::time::Duration;

use proof_autonomy::{commitment, is_digest, ROUND_BLOCKS};
use proof_research::ResearchStore;
use proof_rounds::{RoundConfig, RoundError, RoundLease, RoundStore};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{AtlasError, AtlasRun};

#[derive(Clone)]
pub struct AtlasStore {
    pub(crate) pool: PgPool,
    pub(crate) rounds: RoundStore,
    pub(crate) config: RoundConfig,
}

impl AtlasStore {
    #[must_use]
    pub fn new(pool: PgPool, research: ResearchStore, config: RoundConfig) -> Self {
        Self {
            rounds: RoundStore::new(pool.clone(), research, config.clone()),
            pool,
            config,
        }
    }

    #[must_use]
    pub fn rounds(&self) -> &RoundStore {
        &self.rounds
    }

    /// # Errors
    /// Missing migration or overprivileged controller connection.
    pub async fn ready(&self) -> Result<(), AtlasError> {
        self.rounds.ready().await?;
        let valid: bool = sqlx::query_scalar(
            "SELECT NOT has_table_privilege(current_user, 'proof_atlas_runtime', 'UPDATE,DELETE') \
             AND NOT has_table_privilege(current_user, 'proof_atlas_runtime_event', 'UPDATE,DELETE') \
             AND NOT EXISTS(SELECT 1 FROM information_schema.columns c \
                 WHERE c.table_schema = current_schema() \
                 AND c.table_name IN ('proof_atlas_runtime', 'proof_atlas_runtime_event') \
                 AND NOT (c.table_name = 'proof_atlas_runtime' AND c.column_name IN ('phase','controller_fence')) \
                 AND has_column_privilege(current_user, c.table_name, c.column_name, 'UPDATE'))",
        ).fetch_one(&self.pool).await?;
        if !valid {
            return Err(AtlasError::Invalid);
        }
        Ok(())
    }

    pub(crate) fn boundary(&self, round: u64) -> Result<u64, AtlasError> {
        round
            .checked_add(1)
            .and_then(|r| r.checked_mul(ROUND_BLOCKS))
            .and_then(|b| b.checked_add(self.config.anchor_block))
            .ok_or(AtlasError::Invalid)
    }

    // Advisory selection only: freeze/acquire/publish retain their own fences.
    pub(crate) async fn head(&self) -> Result<(u64, bool, bool), AtlasError> {
        let row: Option<(i64, bool, bool)> = sqlx::query_as(
            "SELECT r.round, d.round IS NOT NULL, COALESCE(p.delivered, false) \
             FROM proof_atlas_round r LEFT JOIN proof_atlas_decision d USING(round) \
             LEFT JOIN proof_atlas_publication p USING(round) ORDER BY r.round DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some((round, decided, delivered)) = row else {
            return Ok((0, false, false));
        };
        let round = u64::try_from(round).map_err(|_| AtlasError::Invalid)?;
        if delivered {
            Ok((
                round.checked_add(1).ok_or(AtlasError::Invalid)?,
                false,
                false,
            ))
        } else {
            Ok((round, true, decided))
        }
    }

    /// Commit the invocation before launch. A controller fence can consume it
    /// only once; takeover changes only that fence, never id/binding/deadline.
    ///
    /// # Errors
    /// Lost ownership, existing decision, changed binding or exhausted run.
    pub async fn begin_run(
        &self,
        lease: &RoundLease,
        binding: &str,
        seconds: u32,
    ) -> Result<(AtlasRun, bool), AtlasError> {
        let frozen = self.rounds.authorize(lease).await?;
        if !is_digest(binding)
            || binding != frozen.config.runtime_digest
            || !(1..=86_400).contains(&seconds)
        {
            return Err(AtlasError::Invalid);
        }
        let digest = commitment(&frozen)?;
        let mut tx = self.owned(lease).await?;
        let inserted = sqlx::query(
            "INSERT INTO proof_atlas_runtime(round,id,binding,frozen_digest,deadline_ms,controller_fence,phase) \
             SELECT $1,$2,$3,$4,floor(extract(epoch FROM clock_timestamp())*1000)::bigint+$5,$6,'started' \
             WHERE NOT EXISTS(SELECT 1 FROM proof_atlas_decision WHERE round=$1) \
             ON CONFLICT(round) DO NOTHING",
        ).bind(number(lease.round)?).bind(Uuid::new_v4()).bind(binding).bind(&digest)
            .bind(i64::from(seconds)*1000).bind(lease.fence).execute(&mut *tx).await?.rows_affected() == 1;
        let mut run: AtlasRun = sqlx::query_as(
            "SELECT id,binding,frozen_digest,deadline_ms,controller_fence,phase FROM proof_atlas_runtime \
             WHERE round=$1 AND deadline_ms > extract(epoch FROM clock_timestamp())*1000 \
             AND NOT EXISTS(SELECT 1 FROM proof_atlas_decision WHERE round=$1) FOR UPDATE",
        ).bind(number(lease.round)?).fetch_optional(&mut *tx).await?.ok_or(AtlasError::Exhausted)?;
        if run.binding != binding || run.frozen_digest != digest {
            return Err(AtlasError::Invalid);
        }
        if run.phase != "started" || (!inserted && run.controller_fence >= lease.fence) {
            return Err(AtlasError::Exhausted);
        }
        sqlx::query("UPDATE proof_atlas_runtime SET controller_fence=$2 WHERE round=$1")
            .bind(number(lease.round)?)
            .bind(lease.fence)
            .execute(&mut *tx)
            .await?;
        event(
            &mut tx,
            run.id,
            lease.fence,
            if inserted { "started" } else { "resumed" },
        )
        .await?;
        guard(&mut tx, lease).await?;
        tx.commit().await?;
        run.controller_fence = lease.fence;
        Ok((run, !inserted))
    }

    pub(crate) async fn remaining(&self, lease: &RoundLease) -> Result<Duration, AtlasError> {
        let mut tx = self.owned(lease).await?;
        let ms: i64 = sqlx::query_scalar(
            "SELECT deadline_ms-ceil(extract(epoch FROM clock_timestamp())*1000)::bigint \
             FROM proof_atlas_runtime WHERE round=$1 AND controller_fence=$2 AND phase='started'",
        )
        .bind(number(lease.round)?)
        .bind(lease.fence)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        let ms = u64::try_from(ms).map_err(|_| AtlasError::Exhausted)?;
        if ms == 0 {
            return Err(AtlasError::Exhausted);
        }
        Ok(Duration::from_millis(ms))
    }

    // Completion is derived from the private operation's durable decision,
    // never from exit status, stdout, proposed round ids or model claims.
    pub(crate) async fn finish(&self, lease: &RoundLease) -> Result<bool, AtlasError> {
        self.rounds.authorize(lease).await?;
        let mut tx = self.owned(lease).await?;
        let decided: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM proof_atlas_decision WHERE round=$1)")
                .bind(number(lease.round)?)
                .fetch_one(&mut *tx)
                .await?;
        let phase = if decided { "finished" } else { "failed" };
        let id: Uuid = sqlx::query_scalar(
            "UPDATE proof_atlas_runtime SET phase=$3 WHERE round=$1 AND controller_fence=$2 \
             AND phase='started' RETURNING id",
        )
        .bind(number(lease.round)?)
        .bind(lease.fence)
        .bind(phase)
        .fetch_one(&mut *tx)
        .await?;
        event(&mut tx, id, lease.fence, phase).await?;
        guard(&mut tx, lease).await?;
        tx.commit().await?;
        Ok(decided)
    }

    pub(crate) async fn release(&self, lease: &RoundLease) -> Result<(), AtlasError> {
        sqlx::query(
            "UPDATE proof_atlas_lease SET expires_at=clock_timestamp() WHERE round=$1 AND owner=$2 AND fence=$3",
        ).bind(number(lease.round)?).bind(lease.owner).bind(lease.fence).execute(&self.pool).await?;
        Ok(())
    }

    async fn owned(
        &self,
        lease: &RoundLease,
    ) -> Result<Transaction<'static, Postgres>, AtlasError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':atlas-rounds',0))",
        )
        .execute(&mut *tx)
        .await?;
        guard(&mut tx, lease).await?;
        Ok(tx)
    }
}

fn number(round: u64) -> Result<i64, AtlasError> {
    i64::try_from(round).map_err(|_| AtlasError::Invalid)
}
async fn guard(tx: &mut Transaction<'_, Postgres>, lease: &RoundLease) -> Result<(), AtlasError> {
    let valid: Option<i64> = sqlx::query_scalar(
        "SELECT fence FROM proof_atlas_lease WHERE round=$1 AND owner=$2 AND fence=$3 \
         AND expires_at>clock_timestamp() FOR UPDATE",
    )
    .bind(number(lease.round)?)
    .bind(lease.owner)
    .bind(lease.fence)
    .fetch_optional(&mut **tx)
    .await?;
    valid.ok_or(RoundError::Fenced)?;
    Ok(())
}
async fn event(
    tx: &mut Transaction<'_, Postgres>,
    run: Uuid,
    fence: i64,
    kind: &str,
) -> Result<(), AtlasError> {
    sqlx::query("INSERT INTO proof_atlas_runtime_event(id,run_id,controller_fence,kind) VALUES($1,$2,$3,$4)")
        .bind(Uuid::new_v4()).bind(run).bind(fence).bind(kind).execute(&mut **tx).await?;
    Ok(())
}
