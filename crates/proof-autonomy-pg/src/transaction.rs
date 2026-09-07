use proof_autonomy::{commitment, ExperimentState, SignedAction};
use serde::{de::DeserializeOwned, Serialize};
use sqlx::{types::Json, Postgres, Transaction};
use uuid::Uuid;

use crate::{ControllerLease, Experiment, StoreError};

pub(crate) type Tx = Transaction<'static, Postgres>;

#[derive(sqlx::FromRow)]
pub(crate) struct ExperimentRow {
    id: Uuid,
    miner_hotkey: String,
    account_id: Uuid,
    recipe_digest: String,
    state: String,
    revision: i64,
    current_quote: Option<Uuid>,
}

impl ExperimentRow {
    pub(crate) fn decode(self) -> Result<Experiment, StoreError> {
        Ok(Experiment {
            id: self.id,
            miner_hotkey: self.miner_hotkey,
            account_id: self.account_id,
            recipe_digest: self.recipe_digest,
            state: decode(&self.state)?,
            revision: self.revision,
            current_quote: self.current_quote,
        })
    }
}

pub(crate) fn decode<T: DeserializeOwned>(value: &str) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| StoreError::Corrupt)
}

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_value(value)
        .map_err(|_| StoreError::Corrupt)?
        .as_str()
        .map(str::to_owned)
        .ok_or(StoreError::Corrupt)
}

pub(crate) async fn now(tx: &mut Tx) -> Result<u64, StoreError> {
    let seconds: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint")
            .fetch_one(&mut **tx)
            .await?;
    u64::try_from(seconds).map_err(|_| StoreError::Corrupt)
}

pub(crate) async fn lock_experiment(
    tx: &mut Tx,
    id: Uuid,
    revision: Option<i64>,
) -> Result<Experiment, StoreError> {
    let record: ExperimentRow =
        sqlx::query_as("SELECT * FROM proof_experiment WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(StoreError::Scope)?;
    let experiment = record.decode()?;
    if revision.is_some_and(|revision| revision != experiment.revision) {
        return Err(StoreError::Conflict);
    }
    Ok(experiment)
}

pub(crate) async fn active_account(
    tx: &mut Tx,
    account: Uuid,
    miner: &str,
) -> Result<(), StoreError> {
    let active: Option<bool> = sqlx::query_scalar(
        "SELECT NOT revoked FROM proof_miner_account \
         WHERE id = $1 AND miner_hotkey = $2 FOR SHARE",
    )
    .bind(account)
    .bind(miner)
    .fetch_optional(&mut **tx)
    .await?;
    if active != Some(true) {
        return Err(StoreError::Scope);
    }
    Ok(())
}

pub(crate) async fn consume_action(
    tx: &mut Tx,
    action: &SignedAction,
    path: &str,
    body: &impl Serialize,
) -> Result<(), StoreError> {
    let digest = commitment(body)?;
    verify_action(tx, action, path, body).await?;
    let changed = sqlx::query(
        "INSERT INTO proof_action_nonce (miner_hotkey, nonce, body_digest, action) \
         VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(&action.miner_hotkey)
    .bind(action.nonce)
    .bind(&digest)
    .bind(Json(action))
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(StoreError::Replay);
    }
    Ok(())
}

pub(crate) async fn verify_action(
    tx: &mut Tx,
    action: &SignedAction,
    path: &str,
    body: &impl Serialize,
) -> Result<(), StoreError> {
    action.verify("POST", path, &commitment(body)?, now(tx).await?)?;
    Ok(())
}

// All command transactions lock the experiment before the lease. Check expiry
// after obtaining the lock so waiting for another transaction cannot renew it.
pub(crate) async fn require_lease(tx: &mut Tx, lease: &ControllerLease) -> Result<(), StoreError> {
    sqlx::query(
        "SELECT experiment_id FROM proof_controller_lease WHERE experiment_id = $1 FOR UPDATE",
    )
    .bind(lease.experiment_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(StoreError::Fenced)?;
    let live: bool = sqlx::query_scalar(
        "SELECT owner_id = $2 AND fence = $3 AND expires_at > clock_timestamp() \
         FROM proof_controller_lease WHERE experiment_id = $1",
    )
    .bind(lease.experiment_id)
    .bind(lease.owner_id)
    .bind(lease.fence)
    .fetch_one(&mut **tx)
    .await?;
    if !live {
        return Err(StoreError::Fenced);
    }
    Ok(())
}

pub(crate) async fn event(
    tx: &mut Tx,
    experiment: &Experiment,
    kind: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO proof_experiment_event (experiment_id, revision, kind, state) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(experiment.id)
    .bind(experiment.revision)
    .bind(kind)
    .bind(encode(&experiment.state)?)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn advance(
    tx: &mut Tx,
    experiment: &mut Experiment,
    next: ExperimentState,
    kind: &str,
) -> Result<(), StoreError> {
    experiment.state.transition(next)?;
    let revision = experiment
        .revision
        .checked_add(1)
        .ok_or(StoreError::Conflict)?;
    let changed = sqlx::query(
        "UPDATE proof_experiment SET state = $3, revision = $4, current_quote = $5 \
         WHERE id = $1 AND revision = $2",
    )
    .bind(experiment.id)
    .bind(experiment.revision)
    .bind(encode(&next)?)
    .bind(revision)
    .bind(experiment.current_quote)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(StoreError::Conflict);
    }
    experiment.state = next;
    experiment.revision = revision;
    event(tx, experiment, kind).await
}
