//! Transactional, headless Proof commands. No model or provider runs inside a
//! transaction. External work starts only from a committed service intent.
//!
//! Database fencing rejects obsolete controllers' writes; it does not fence an
//! already-issued request at Lium. Ambiguous dispatch requires reconciliation.

#![forbid(unsafe_code)]

mod cleanup;
mod commands;
mod lease;
mod quotes;
mod resources;
mod transaction;
mod types;
mod view;

pub use types::*;

use proof_autonomy::ContractError;
use sqlx::PgPool;

/// Trusted extension transaction. The experiment lock is held until commit;
/// the final fence check prevents an expired controller from publishing evidence.
pub struct ControllerTransaction {
    transaction: sqlx::Transaction<'static, sqlx::Postgres>,
    experiment: Experiment,
    lease: ControllerLease,
}

impl ControllerTransaction {
    #[must_use]
    pub const fn experiment(&self) -> &Experiment {
        &self.experiment
    }

    pub fn connection(&mut self) -> &mut sqlx::PgConnection {
        &mut self.transaction
    }

    /// Advance a lifecycle only after the trusted extension has checked its
    /// own prerequisites. No model or public route may choose these arguments.
    ///
    /// # Errors
    /// Illegal transition, unknown event kind, revision conflict or DB failure.
    pub async fn advance(
        &mut self,
        next: proof_autonomy::ExperimentState,
        kind: &str,
    ) -> Result<(), StoreError> {
        transaction::advance(&mut self.transaction, &mut self.experiment, next, kind).await
    }

    /// # Errors
    /// Expired or replaced controller, or database commit failure.
    pub async fn commit(mut self) -> Result<(), StoreError> {
        transaction::require_lease(&mut self.transaction, &self.lease).await?;
        self.transaction.commit().await?;
        Ok(())
    }
}

/// A connected database is required; there is no in-memory fallback.
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Begin a trusted controller extension using the same lock/fence order.
    /// Never hold this transaction while invoking a provider or a model.
    ///
    /// # Errors
    /// Missing experiment, stale controller or database failure.
    pub async fn controller_transaction(
        &self,
        lease: &ControllerLease,
    ) -> Result<ControllerTransaction, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let experiment =
            transaction::lock_experiment(&mut transaction, lease.experiment_id, None).await?;
        transaction::require_lease(&mut transaction, lease).await?;
        Ok(ControllerTransaction {
            transaction,
            experiment,
            lease: *lease,
        })
    }
}

/// Stable errors deliberately exclude SQL diagnostics, signatures and payloads.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("record missing or outside authorization")]
    Scope,
    #[error("stale experiment revision or conflicting command")]
    Conflict,
    #[error("authorization already consumed")]
    Replay,
    #[error("experiment intake quota exhausted")]
    Quota,
    #[error("controller lease unavailable or stale")]
    Fenced,
    #[error("stored contract is invalid")]
    Corrupt,
    #[error("database operation failed")]
    Database,
}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        if error
            .as_database_error()
            .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
        {
            Self::Conflict
        } else {
            Self::Database
        }
    }
}
