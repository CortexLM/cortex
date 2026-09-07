//! Local CPU capacity as a strict `MinerProvider`. A "rental" is the durable
//! claim of one configured slot on one trusted Docker daemon; nothing is
//! charged and no remote provider or credential is involved. Containers are
//! created only by `proof-executor` under journaled intents; this crate
//! observes and removes them by experiment label during cleanup.

#![forbid(unsafe_code)]

mod docker;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use proof_autonomy::{commitment, is_digest, DeletionResult, MachineQuote, ProvisionResult};
use proof_autonomy_pg::{Experiment, MinerAccount, Resource};
use proof_broker::{MinerProvider, ProviderError};
use proof_worker::{QuoteSource, WorkerError};
use sqlx::PgPool;
use uuid::Uuid;

use docker::Daemon;

/// Quote fields that describe local CPU capacity. The quote schema requires a
/// nonzero GPU and nonzero hourly price; these are placeholders documented in
/// the runbook, never real hardware or a charge.
pub const LOCAL_IMAGE_NAME: &str = "local-docker-cpu";
pub const LOCAL_GPU_TYPE: &str = "none-local-cpu";
pub const PLACEHOLDER_HOURLY_MICROUSD: u64 = 1;
pub const MAX_SLOTS: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LocalError {
    #[error("local provider configuration is invalid")]
    Invalid,
    #[error("trusted local Docker daemon or pinned image unavailable")]
    Target,
    #[error("local slot database unavailable")]
    Database,
}
impl From<sqlx::Error> for LocalError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<LocalError> for WorkerError {
    fn from(error: LocalError) -> Self {
        match error {
            LocalError::Invalid => Self::Invalid,
            _ => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalProviderConfig {
    pub docker_socket: PathBuf,
    /// Exact local image ID (`sha256:…`), identical to the executor target.
    pub image_id: String,
    pub slots: u32,
    pub lifetime_seconds: u64,
    pub quote_seconds: u64,
    pub ram_mib: u64,
}

impl LocalProviderConfig {
    /// # Errors
    /// Unbounded slots, lifetimes, quote windows or an unpinned image.
    pub fn validate(&self) -> Result<(), LocalError> {
        if !(1..=MAX_SLOTS).contains(&self.slots)
            || !(60..=86_400).contains(&self.lifetime_seconds)
            || !(30..=3_600).contains(&self.quote_seconds)
            || self.ram_mib == 0
            || !self.docker_socket.is_absolute()
            || !self.image_id.strip_prefix("sha256:").is_some_and(is_digest)
        {
            return Err(LocalError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct LocalProvider {
    pool: PgPool,
    daemon: Daemon,
    config: LocalProviderConfig,
    fingerprint: String,
    offer_id: String,
}

#[derive(sqlx::FromRow)]
struct Outcome {
    kind: String,
    resource_id: Option<String>,
}

impl LocalProvider {
    /// Observe the trusted daemon and pinned image. Writes nothing.
    ///
    /// # Errors
    /// Invalid configuration, missing exact image or unreachable daemon.
    pub async fn connect(pool: PgPool, config: LocalProviderConfig) -> Result<Self, LocalError> {
        config.validate()?;
        let daemon = Daemon::connect(&config.docker_socket, &config.image_id).await?;
        let fingerprint = commitment(&("local-docker", &daemon.engine_id, &config.image_id))
            .map_err(|_| LocalError::Invalid)?;
        let offer_id = format!("local-{}-{}", &fingerprint[..32], config.slots);
        Ok(Self {
            pool,
            daemon,
            config,
            fingerprint,
            offer_id,
        })
    }

    #[must_use]
    pub fn engine_id(&self) -> &str {
        &self.daemon.engine_id
    }

    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.config.docker_socket
    }

    /// Refuse an owner connection or a schema where slot history is mutable.
    ///
    /// # Errors
    /// Missing tables, elevated role or unexpected mutable columns.
    pub async fn ready(&self) -> Result<(), LocalError> {
        let valid: bool = sqlx::query_scalar(
            "SELECT NOT has_table_privilege(current_user, 'proof_local_event', 'UPDATE,DELETE') \
             AND NOT has_table_privilege(current_user, 'proof_local_slot', 'DELETE') \
             AND NOT EXISTS(SELECT 1 FROM information_schema.columns c \
                 WHERE c.table_schema = current_schema() AND c.table_name = 'proof_local_slot' \
                 AND c.column_name NOT IN ('intent_id', 'resource_id') \
                 AND has_column_privilege(current_user, c.table_name, c.column_name, 'UPDATE'))",
        )
        .fetch_one(&self.pool)
        .await?;
        if !valid {
            return Err(LocalError::Invalid);
        }
        Ok(())
    }

    /// Materialize the configured slot rows. Lowering `slots` later leaves
    /// higher rows unused; it never deletes a claimed slot.
    ///
    /// # Errors
    /// Database failure.
    pub async fn ensure_slots(&self) -> Result<(), LocalError> {
        for slot in 0..self.config.slots {
            sqlx::query(
                "INSERT INTO proof_local_slot (engine_id, slot) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(&self.daemon.engine_id)
            .bind(i32::try_from(slot).map_err(|_| LocalError::Invalid)?)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    fn issued_here(&self, quote: &MachineQuote) -> bool {
        quote.image == LOCAL_IMAGE_NAME
            && quote.image_digest == self.config.image_id
            && quote.provider_fingerprint == self.fingerprint
            && quote.offer_id == self.offer_id
            && quote.hourly_total_microusd == PLACEHOLDER_HOURLY_MICROUSD
            && quote.lifetime_seconds == self.config.lifetime_seconds
    }

    async fn now(&self) -> Result<u64, LocalError> {
        let now: i64 =
            sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint")
                .fetch_one(&self.pool)
                .await?;
        u64::try_from(now).map_err(|_| LocalError::Database)
    }

    async fn outcome(&self, intent: Uuid) -> Result<Option<Outcome>, LocalError> {
        Ok(sqlx::query_as(
            "SELECT kind, resource_id FROM proof_local_event WHERE intent_id = $1 \
             AND kind IN ('claimed', 'refused')",
        )
        .bind(intent)
        .fetch_optional(&self.pool)
        .await?)
    }

    fn resolve(outcome: Option<Outcome>) -> ProvisionResult {
        match outcome {
            Some(Outcome {
                kind,
                resource_id: Some(resource_id),
            }) if kind == "claimed" => ProvisionResult::Confirmed { resource_id },
            Some(Outcome { kind, .. }) if kind == "refused" => ProvisionResult::NotCreated,
            _ => ProvisionResult::Uncertain,
        }
    }

    async fn claim(
        &self,
        account: &MinerAccount,
        intent: Uuid,
    ) -> Result<ProvisionResult, LocalError> {
        let mut tx = self.pool.begin().await?;
        let resource_id = format!("local-{}", intent.simple());
        let slot: Option<i32> = sqlx::query_scalar(
            "UPDATE proof_local_slot SET intent_id = $1, resource_id = $2 \
             WHERE engine_id = $3 AND slot = (SELECT slot FROM proof_local_slot \
                 WHERE engine_id = $3 AND intent_id IS NULL AND slot < $4 \
                 ORDER BY slot FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING slot",
        )
        .bind(intent)
        .bind(&resource_id)
        .bind(&self.daemon.engine_id)
        .bind(i32::try_from(self.config.slots).map_err(|_| LocalError::Invalid)?)
        .fetch_optional(&mut *tx)
        .await?;
        // The partial unique index makes this outcome the only one for the intent.
        sqlx::query(
            "INSERT INTO proof_local_event (id, engine_id, image_id, intent_id, account_id, slot, resource_id, kind) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::new_v4())
        .bind(&self.daemon.engine_id)
        .bind(&self.config.image_id)
        .bind(intent)
        .bind(account.id)
        .bind(slot)
        .bind(slot.map(|_| &resource_id))
        .bind(if slot.is_some() { "claimed" } else { "refused" })
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(match slot {
            Some(_) => ProvisionResult::Confirmed { resource_id },
            None => ProvisionResult::NotCreated,
        })
    }

    /// Resource ownership is the claimed event, not the resource string.
    async fn owned(&self, account: &MinerAccount, resource: &Resource) -> Result<bool, LocalError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_local_event WHERE intent_id = $1 AND resource_id = $2 \
             AND account_id = $3 AND engine_id = $4 AND kind = 'claimed')",
        )
        .bind(resource.intent_id)
        .bind(&resource.resource_id)
        .bind(account.id)
        .bind(&self.daemon.engine_id)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn released(&self, resource: &Resource) -> Result<bool, LocalError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_local_event WHERE intent_id = $1 AND kind = 'released') \
             AND NOT EXISTS(SELECT 1 FROM proof_local_slot WHERE intent_id = $1 OR resource_id = $2)",
        )
        .bind(resource.intent_id)
        .bind(&resource.resource_id)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn release(&self, account: &MinerAccount, resource: &Resource) -> Result<(), LocalError> {
        let mut tx = self.pool.begin().await?;
        let slot: Option<i32> = sqlx::query_scalar(
            "UPDATE proof_local_slot SET intent_id = NULL, resource_id = NULL \
             WHERE engine_id = $1 AND intent_id = $2 AND resource_id = $3 RETURNING slot",
        )
        .bind(&self.daemon.engine_id)
        .bind(resource.intent_id)
        .bind(&resource.resource_id)
        .fetch_optional(&mut *tx)
        .await?;
        let slot =
            match slot {
                Some(slot) => slot,
                None => sqlx::query_scalar(
                    "SELECT slot FROM proof_local_event WHERE intent_id = $1 AND kind = 'claimed'",
                )
                .bind(resource.intent_id)
                .fetch_one(&mut *tx)
                .await?,
            };
        sqlx::query(
            "INSERT INTO proof_local_event (id, engine_id, image_id, intent_id, account_id, slot, resource_id, kind) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'released') ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(&self.daemon.engine_id)
        .bind(&self.config.image_id)
        .bind(resource.intent_id)
        .bind(account.id)
        .bind(slot)
        .bind(&resource.resource_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[async_trait]
impl QuoteSource for LocalProvider {
    async fn quote(&self, experiment: &Experiment) -> Result<Option<MachineQuote>, WorkerError> {
        self.daemon.observe_engine().await?;
        let now = self.now().await?;
        let lifetime = self.config.lifetime_seconds;
        let quote = MachineQuote {
            schema_version: 1,
            id: Uuid::new_v4(),
            experiment_id: experiment.id,
            miner_hotkey: experiment.miner_hotkey.clone(),
            account_id: experiment.account_id,
            recipe_digest: experiment.recipe_digest.clone(),
            offer_id: self.offer_id.clone(),
            gpu_type: LOCAL_GPU_TYPE.into(),
            gpu_count: 1,
            gpu_memory_mib: 1,
            ram_mib: self.config.ram_mib,
            disk_gib: 1,
            image: LOCAL_IMAGE_NAME.into(),
            image_digest: self.config.image_id.clone(),
            hourly_total_microusd: PLACEHOLDER_HOURLY_MICROUSD,
            maximum_total_microusd: PLACEHOLDER_HOURLY_MICROUSD
                .saturating_mul(lifetime)
                .div_ceil(3_600)
                .max(1),
            lifetime_seconds: lifetime,
            issued_at: now,
            expires_at: now.saturating_add(self.config.quote_seconds),
            provider_fingerprint: self.fingerprint.clone(),
        };
        quote.validate(now)?;
        Ok(Some(quote))
    }
}

#[async_trait]
impl MinerProvider for LocalProvider {
    async fn preflight(
        &self,
        _: &MinerAccount,
        quote: &MachineQuote,
    ) -> Result<String, ProviderError> {
        let engine = self
            .daemon
            .observe_engine()
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        let now = self.now().await.map_err(|_| ProviderError::Unavailable)?;
        if engine != self.daemon.engine_id || !self.issued_here(quote) {
            return Err(ProviderError::Unsupported);
        }
        quote
            .validate(now)
            .map_err(|_| ProviderError::Unsupported)?;
        commitment(quote).map_err(|_| ProviderError::Unsupported)
    }

    async fn rent(
        &self,
        account: &MinerAccount,
        quote: &MachineQuote,
        intent: Uuid,
    ) -> ProvisionResult {
        let Ok(now) = self.now().await else {
            return ProvisionResult::Uncertain;
        };
        match self.outcome(intent).await {
            Ok(Some(outcome)) => return Self::resolve(Some(outcome)),
            Ok(None) => {}
            Err(_) => return ProvisionResult::Uncertain,
        }
        // Nothing has been claimed yet, so refusing here is certifiable.
        if !self.issued_here(quote)
            || quote.validate(now).is_err()
            || quote.account_id != account.id
            || self.daemon.observe_engine().await.is_err()
        {
            return ProvisionResult::NotCreated;
        }
        match self.claim(account, intent).await {
            Ok(result) => result,
            // The commit may have landed; reconcile by intent, never claim again.
            Err(_) => ProvisionResult::Uncertain,
        }
    }

    async fn reconcile(&self, _: &MinerAccount, _: &MachineQuote, intent: Uuid) -> ProvisionResult {
        // Sealing a refusal first makes a late original claim impossible.
        let sealed = sqlx::query(
            "INSERT INTO proof_local_event (id, engine_id, image_id, intent_id, account_id, kind) \
             SELECT $1, $2, $3, $4, i.account_id, 'refused' FROM (SELECT e.account_id \
                 FROM proof_service_intent s JOIN proof_experiment e ON e.id = s.experiment_id \
                 WHERE s.id = $4) i ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(&self.daemon.engine_id)
        .bind(&self.config.image_id)
        .bind(intent)
        .execute(&self.pool)
        .await;
        if sealed.is_err() {
            return ProvisionResult::Uncertain;
        }
        match self.outcome(intent).await {
            Ok(outcome) => Self::resolve(outcome),
            Err(_) => ProvisionResult::Uncertain,
        }
    }

    async fn delete(
        &self,
        account: &MinerAccount,
        resource: &Resource,
    ) -> Result<(), ProviderError> {
        if !self
            .owned(account, resource)
            .await
            .map_err(|_| ProviderError::Unavailable)?
        {
            return Err(ProviderError::Unauthorized);
        }
        let containers = self
            .daemon
            .labelled(resource.experiment_id)
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        for (id, pinned) in containers {
            if pinned {
                self.daemon
                    .force_remove(&id)
                    .await
                    .map_err(|_| ProviderError::Unavailable)?;
            }
        }
        // Release the slot only after the daemon no longer runs this experiment.
        self.release(account, resource)
            .await
            .map_err(|_| ProviderError::Unavailable)
    }

    async fn deletion_status(&self, account: &MinerAccount, resource: &Resource) -> DeletionResult {
        match self.owned(account, resource).await {
            Ok(true) => {}
            Ok(false) => return DeletionResult::Unauthorized,
            Err(_) => return DeletionResult::Unavailable,
        }
        let Ok(released) = self.released(resource).await else {
            return DeletionResult::Unavailable;
        };
        match self.daemon.labelled(resource.experiment_id).await {
            Ok(containers) if containers.is_empty() && released => DeletionResult::Confirmed,
            Ok(_) => DeletionResult::Pending,
            Err(_) => DeletionResult::Unavailable,
        }
    }
}
