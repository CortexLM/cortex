use proof_autonomy::is_digest;
use proof_autonomy_pg::{ControllerLease, PgStore};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{RuntimeRun, WorkCandidate, WorkerError};

#[derive(Clone)]
pub struct WorkStore {
    pub(crate) pool: PgPool,
    pub(crate) orchestration: PgStore,
}

impl WorkStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            orchestration: PgStore::new(pool.clone()),
            pool,
        }
    }

    /// # Errors
    /// Missing schema, elevated service role or unexpected mutable bindings.
    pub async fn ready(&self) -> Result<(), WorkerError> {
        self.orchestration.ready().await?;
        let valid: bool = sqlx::query_scalar(
            "SELECT NOT has_table_privilege(current_user, 'proof_runtime_run', 'UPDATE,DELETE') \
             AND NOT has_table_privilege(current_user, 'proof_runtime_event', 'UPDATE,DELETE') \
             AND NOT has_table_privilege(current_user, 'proof_work_schedule', 'UPDATE,DELETE') \
             AND NOT EXISTS(SELECT 1 FROM information_schema.columns c \
                 WHERE c.table_schema = current_schema() \
                 AND ((c.table_name = 'proof_runtime_run' AND c.column_name NOT IN ('phase','controller_fence')) \
                     OR (c.table_name = 'proof_work_schedule' AND c.column_name NOT IN ('next_wake','revision'))) \
                 AND has_column_privilege(current_user, c.table_name, c.column_name, 'UPDATE'))",
        )
        .fetch_one(&self.pool)
        .await?;
        if !valid {
            return Err(WorkerError::Invalid);
        }
        Ok(())
    }

    /// Scheduling is advisory; `PgStore::acquire` arbitrates every candidate.
    ///
    /// # Errors
    /// Invalid bounded batch or database failure.
    pub async fn candidates(
        &self,
        limit: u32,
        cleanup: bool,
    ) -> Result<Vec<WorkCandidate>, WorkerError> {
        if !(1..=64).contains(&limit) {
            return Err(WorkerError::Invalid);
        }
        Ok(sqlx::query_as(
            "SELECT e.id, $2::boolean AS cleanup FROM proof_experiment e \
             LEFT JOIN proof_controller_lease l ON l.experiment_id = e.id \
             LEFT JOIN proof_work_schedule s ON s.experiment_id = e.id \
             LEFT JOIN proof_machine_quote q ON q.id = e.current_quote \
             WHERE (e.state NOT IN ('completed','cancelled','rejected','awaiting_consent') \
                 OR (e.state = 'awaiting_consent' AND (q.quote->>'expires_at')::bigint <= extract(epoch FROM clock_timestamp())) \
                 OR EXISTS(SELECT 1 FROM proof_resource r WHERE r.experiment_id = e.id AND r.status <> 'deleted')) \
             AND (e.state IN ('cancelling','deleting','reconciling','completed','cancelled','rejected')) = $2 \
             AND (l.expires_at IS NULL OR l.expires_at <= clock_timestamp()) \
             AND (s.next_wake IS NULL OR s.next_wake <= clock_timestamp() OR s.revision <> e.revision) \
             ORDER BY COALESCE(s.next_wake, '-infinity'), e.created_at, e.id LIMIT $1",
        ).bind(i64::from(limit)).bind(cleanup).fetch_all(&self.pool).await?)
    }

    /// # Errors
    /// Lost ownership or invalid bounded retry interval.
    pub async fn defer(&self, lease: &ControllerLease, seconds: u32) -> Result<(), WorkerError> {
        if !(1..=300).contains(&seconds) {
            return Err(WorkerError::Invalid);
        }
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        let revision = tx.experiment().revision;
        sqlx::query(
            "INSERT INTO proof_work_schedule(experiment_id, next_wake, revision) \
             VALUES ($1, clock_timestamp() + make_interval(secs => $2), $3) ON CONFLICT(experiment_id) \
             DO UPDATE SET next_wake = excluded.next_wake, revision = excluded.revision",
        ).bind(lease.experiment_id).bind(f64::from(seconds)).bind(revision).execute(tx.connection()).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Commit an invocation before spawning. A takeover keeps id, binding and
    /// original absolute deadline; an expired/missing checkpoint cannot restart.
    ///
    /// # Errors
    /// Wrong phase, changed binding, exhausted deadline or stale lease.
    pub async fn begin_run(
        &self,
        lease: &ControllerLease,
        binding: &str,
        seconds: u32,
    ) -> Result<(RuntimeRun, bool), WorkerError> {
        if !is_digest(binding) || !(1..=86_400).contains(&seconds) {
            return Err(WorkerError::Invalid);
        }
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        if !matches!(
            tx.experiment().state,
            proof_autonomy::ExperimentState::Running | proof_autonomy::ExperimentState::Collecting
        ) {
            return Err(WorkerError::Invalid);
        }
        let inserted = sqlx::query(
            "INSERT INTO proof_runtime_run(experiment_id, id, binding, deadline_ms, phase, controller_fence) \
             SELECT $1, $2, $3, LEAST(floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint + $4, \
                 min(authorized_until) * 1000), 'started', $5 FROM proof_resource WHERE experiment_id = $1 \
                 AND status = 'active' HAVING count(*) = 1 \
             ON CONFLICT(experiment_id) DO NOTHING",
        ).bind(lease.experiment_id).bind(Uuid::new_v4()).bind(binding)
        .bind(i64::from(seconds) * 1000).bind(lease.fence).execute(tx.connection()).await?.rows_affected() == 1;
        let run: RuntimeRun = sqlx::query_as(
            "SELECT experiment_id, id, binding, deadline_ms, phase, controller_fence FROM proof_runtime_run \
             WHERE experiment_id = $1 AND deadline_ms > extract(epoch FROM clock_timestamp()) * 1000 FOR UPDATE",
        ).bind(lease.experiment_id).fetch_optional(tx.connection()).await?.ok_or(WorkerError::Invalid)?;
        if run.binding != binding
            || run.phase != "started"
            || (!inserted && run.controller_fence >= lease.fence)
        {
            return Err(WorkerError::Invalid);
        }
        sqlx::query("UPDATE proof_runtime_run SET controller_fence = $2 WHERE experiment_id = $1")
            .bind(lease.experiment_id)
            .bind(lease.fence)
            .execute(tx.connection())
            .await?;
        sqlx::query("INSERT INTO proof_runtime_event(id, run_id, controller_fence, kind) VALUES($1,$2,$3,$4)")
            .bind(Uuid::new_v4()).bind(run.id).bind(lease.fence).bind(if inserted { "started" } else { "resumed" })
            .execute(tx.connection()).await?;
        tx.commit().await?;
        Ok((
            RuntimeRun {
                controller_fence: lease.fence,
                ..run
            },
            !inserted,
        ))
    }

    pub(crate) async fn remaining_run(
        &self,
        lease: &ControllerLease,
    ) -> Result<std::time::Duration, WorkerError> {
        let checked = std::time::Instant::now();
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        let ms: i64 = sqlx::query_scalar(
            "SELECT GREATEST(0, deadline_ms-ceil(extract(epoch FROM clock_timestamp())*1000)::bigint) \
             FROM proof_runtime_run WHERE experiment_id=$1 AND controller_fence=$2 AND phase='started'",
        )
        .bind(lease.experiment_id)
        .bind(lease.fence)
        .fetch_one(tx.connection())
        .await?;
        tx.commit().await?;
        Ok(
            std::time::Duration::from_millis(u64::try_from(ms).map_err(|_| WorkerError::Invalid)?)
                .saturating_sub(checked.elapsed()),
        )
    }

    /// Expired quotes cannot consume new consent. Return to discussion only
    /// before dispatch, cancelling the old pending intent in the same lock.
    ///
    /// # Errors
    /// Lost ownership, invalid stored quote or database failure.
    pub async fn refresh_expired_quote(&self, lease: &ControllerLease) -> Result<(), WorkerError> {
        use proof_autonomy::ExperimentState::{Approved, AwaitingConsent, Discussion};
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        if !matches!(tx.experiment().state, Approved | AwaitingConsent) {
            return Ok(());
        }
        let expired: bool = sqlx::query_scalar(
            "SELECT (quote->>'expires_at')::bigint <= extract(epoch FROM clock_timestamp()) \
             FROM proof_machine_quote WHERE id = $1 AND experiment_id = $2",
        )
        .bind(tx.experiment().current_quote)
        .bind(lease.experiment_id)
        .fetch_one(tx.connection())
        .await?;
        if expired {
            sqlx::query("UPDATE proof_service_intent SET status = 'cancelled' WHERE experiment_id = $1 AND kind = 'provision' AND status = 'pending'")
                .bind(lease.experiment_id).execute(tx.connection()).await?;
            if tx.experiment().state == Approved {
                tx.advance(AwaitingConsent, "quote_expired").await?;
            }
            tx.advance(Discussion, "quote_refresh").await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// # Errors
    /// Lost ownership or database failure. A run stays resumable.
    pub async fn interrupt_run(&self, lease: &ControllerLease) -> Result<(), WorkerError> {
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        sqlx::query(
            "INSERT INTO proof_runtime_event(id, run_id, controller_fence, kind) \
             SELECT $1, id, $3, 'interrupted' FROM proof_runtime_run WHERE experiment_id = $2 \
             AND controller_fence = $3 AND phase = 'started'",
        )
        .bind(Uuid::new_v4())
        .bind(lease.experiment_id)
        .bind(lease.fence)
        .execute(tx.connection())
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// # Errors
    /// Stale controller, missing run or invalid phase.
    pub async fn finish_run(
        &self,
        lease: &ControllerLease,
        success: bool,
    ) -> Result<(), WorkerError> {
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        let id: Uuid = sqlx::query_scalar(
            "UPDATE proof_runtime_run SET phase = $3 WHERE experiment_id = $1 AND controller_fence = $2 \
             AND phase = 'started' RETURNING id",
        ).bind(lease.experiment_id).bind(lease.fence).bind(if success { "finished" } else { "failed" })
        .fetch_one(tx.connection()).await?;
        sqlx::query("INSERT INTO proof_runtime_event(id, run_id, controller_fence, kind) VALUES($1,$2,$3,$4)")
            .bind(Uuid::new_v4()).bind(id).bind(lease.fence).bind(if success { "finished" } else { "failed" })
            .execute(tx.connection()).await?;
        tx.commit().await?;
        Ok(())
    }
}
