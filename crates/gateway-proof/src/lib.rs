//! Opt-in durable v2 receiver. No in-memory proof cache, no GPU or chain writes.
#![forbid(unsafe_code)]

mod evidence;
mod executor;
mod seal;
mod stores;

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chain_live::{FinalizedSnapshot, LiveChainClient};
use gateway::{ChallengesBody, ParticipantPolicy, Stores};
use parity_scale_codec::Encode;
use proof_publication::{Receipt, RoundPublication, MAX_WIRE_BYTES, ROUTE};
use sqlx::{PgConnection, PgPool, Row};

use executor::Executor;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Proof round or pinned chain inputs")]
    Invalid,
    #[error("Proof publication signature is not authorized")]
    Unauthorized,
    #[error("Proof round conflicts with the durable publication head")]
    Conflict,
    #[error("Proof publication is unavailable")]
    Unavailable,
}

impl From<sqlx::Error> for Error {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
impl From<proof_publication::Error> for Error {
    fn from(value: proof_publication::Error) -> Self {
        match value {
            proof_publication::Error::Unauthorized => Self::Unauthorized,
            _ => Self::Invalid,
        }
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Invalid => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Conflict => StatusCode::CONFLICT,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status, Json(serde_json::json!({"error": self.to_string()}))).into_response()
    }
}

/// Trusted read-only chain adapter; only local tests use a fake.
pub trait FinalizedSource: Send + Sync {
    /// # Errors
    /// Unfinalized block, missing pinned state or transport failure.
    fn snapshot(&self, block: u64) -> Result<FinalizedSnapshot, Error>;
}
impl FinalizedSource for LiveChainClient {
    fn snapshot(&self, block: u64) -> Result<FinalizedSnapshot, Error> {
        self.finalized_snapshot(block)
            .map_err(|_| Error::Unavailable)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub netuid: u16,
    pub anchor_block: u64,
    pub proof_public_key: [u8; 32],
}

pub struct Receiver {
    exec: Executor,
    source: Arc<dyn FinalizedSource>,
    config: Config,
    challenges: ChallengesBody,
    legacy: Stores,
    chain: gateway::SharedChain,
}

impl Receiver {
    /// Activate sticky v2 configuration and open both stores on the SAME database.
    /// Migrations must already be applied. A changed key/netuid/anchor fails closed.
    ///
    /// # Errors
    /// Bad trust root, conflicting configuration, runtime or database failure.
    pub fn connect(
        url: &str,
        source: Arc<dyn FinalizedSource>,
        chain: gateway::SharedChain,
        config: Config,
        challenges: ChallengesBody,
    ) -> Result<Arc<Self>, Error> {
        let proof = challenges.get(b"proof").ok_or(Error::Invalid)?;
        if proof.public_key != config.proof_public_key
            || proof.policy != ParticipantPolicy::AllMetagraphHotkeys
            || proof.emission_share_bps == 0
        {
            return Err(Error::Invalid);
        }
        let exec = Executor::connect(url)?;
        let pinned = config.clone();
        exec.run(move |pool| async move {
            let mut tx = pool.begin().await?;
            lock(&mut tx).await?;
            sqlx::query(
                "INSERT INTO gateway_proof_config (netuid, anchor_block, public_key) \
                 VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            )
            .bind(i32::from(pinned.netuid))
            .bind(integer(pinned.anchor_block)?)
            .bind(pinned.proof_public_key.as_slice())
            .execute(&mut *tx)
            .await?;
            let row =
                sqlx::query("SELECT netuid, anchor_block, public_key FROM gateway_proof_config")
                    .fetch_one(&mut *tx)
                    .await?;
            if row.try_get::<i32, _>("netuid")? != i32::from(pinned.netuid)
                || row.try_get::<i64, _>("anchor_block")? != integer(pinned.anchor_block)?
                || row.try_get::<Vec<u8>, _>("public_key")? != pinned.proof_public_key
            {
                return Err(Error::Conflict);
            }
            tx.commit().await?;
            Ok(())
        })?;
        let legacy = gateway_store_pg::stores(url).map_err(|_| Error::Unavailable)?;
        Ok(Arc::new(Self {
            exec,
            source,
            config,
            challenges,
            legacy,
            chain,
        }))
    }

    /// Always check before starting in legacy mode; activation survives restarts.
    ///
    /// # Errors
    /// Database failure (never interpreted as disabled).
    pub async fn is_enabled(pool: &PgPool) -> Result<bool, Error> {
        Ok(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM gateway_proof_config)")
                .fetch_one(pool)
                .await?,
        )
    }

    #[must_use]
    pub fn stores(self: &Arc<Self>) -> Stores {
        stores::wrap(self.clone())
    }

    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route(ROUTE, post(post_round))
            .route(&format!("{ROUTE}/{{round}}"), get(get_round))
            .layer(DefaultBodyLimit::max(MAX_WIRE_BYTES))
            .with_state(self.clone())
            .merge(self.evidence_router())
    }

    /// Validate outside the lock, then recheck the durable head while committing.
    /// Cancellation/late completion can never replace an already newer round.
    ///
    /// # Errors
    /// Bad authentication/chain/batch, old/conflicting round or DB uncertainty.
    pub fn accept(&self, wire: &[u8]) -> Result<Receipt, Error> {
        let document = RoundPublication::from_wire(wire)?;
        document.verify(&self.config.proof_public_key)?;
        let receipt = document.receipt()?;
        // Exact committed retry needs no historical RPC (which may be pruned).
        let candidate = document.clone();
        if self.exec.run(move |pool| async move {
            let mut tx = pool.begin().await?;
            lock(&mut tx).await?;
            let same = check_head(&mut tx, &candidate).await?;
            tx.commit().await?;
            Ok(same)
        })? {
            return Ok(receipt);
        }
        self.validate_chain(&document)?;
        let candidate = document;
        self.exec.run(move |pool| async move {
            let mut tx = pool.begin().await?;
            lock(&mut tx).await?;
            if !check_head(&mut tx, &candidate).await? {
                sqlx::query(
                    "INSERT INTO gateway_proof_round (round, chain_epoch, block_number, digest, wire) \
                     VALUES ($1, $2, $3, $4, $5)",
                ).bind(integer(candidate.round)?).bind(integer(candidate.chain_epoch)?)
                    .bind(integer(candidate.block)?).bind(candidate.receipt()?.digest)
                    .bind(candidate.encode()).execute(&mut *tx).await?;
            }
            // Read back the entire batch inside the same transaction.
            if !check_head(&mut tx, &candidate).await? {
                return Err(Error::Unavailable);
            }
            tx.commit().await?;
            Ok(())
        })?;
        Ok(receipt)
    }

    /// Current head only: archived rounds never acknowledge an old delivery.
    ///
    /// # Errors
    /// Missing/stale round, corrupt stored document or unavailable database.
    pub fn readback(&self, round: u64) -> Result<Vec<u8>, Error> {
        let document = self.exec.run(move |pool| async move {
            let mut conn = pool.acquire().await?;
            let document = head(&mut conn).await?.ok_or(Error::Conflict)?;
            if document.round != round {
                return Err(Error::Conflict);
            }
            Ok(document)
        })?;
        document.verify(&self.config.proof_public_key)?;
        Ok(document.encode())
    }

    fn validate_chain(&self, document: &RoundPublication) -> Result<FinalizedSnapshot, Error> {
        let snapshot = self.source.snapshot(document.block)?;
        let roster: Vec<[u8; 32]> = snapshot
            .metagraph
            .hotkeys
            .iter()
            .map(|h| h.as_slice().try_into().map_err(|_| Error::Invalid))
            .collect::<Result<_, _>>()?;
        if snapshot.block != document.block
            || hex::encode(snapshot.hash) != document.block_hash
            || snapshot.chain_epoch != document.chain_epoch
            || snapshot.metagraph.netuid != self.config.netuid
        {
            return Err(Error::Invalid);
        }
        document.validate_roster(
            &self.config.proof_public_key,
            self.config.netuid,
            self.config.anchor_block,
            &roster,
        )?;
        Ok(snapshot)
    }
}

async fn post_round(State(receiver): State<Arc<Receiver>>, bytes: Bytes) -> Response {
    match tokio::task::spawn_blocking(move || receiver.accept(&bytes)).await {
        Ok(Ok(receipt)) => ([(header::CACHE_CONTROL, "no-store")], Json(receipt)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(_) => Error::Unavailable.into_response(),
    }
}

async fn get_round(State(receiver): State<Arc<Receiver>>, Path(round): Path<u64>) -> Response {
    match tokio::task::spawn_blocking(move || receiver.readback(round)).await {
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

pub(crate) fn integer(n: u64) -> Result<i64, Error> {
    i64::try_from(n).map_err(|_| Error::Invalid)
}

async fn lock(conn: &mut PgConnection) -> Result<(), Error> {
    sqlx::query("SET LOCAL lock_timeout = '10s'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '15s'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext(current_schema()), 2525)")
        .execute(conn)
        .await?;
    Ok(())
}

async fn head(conn: &mut PgConnection) -> Result<Option<RoundPublication>, Error> {
    let row = sqlx::query("SELECT * FROM gateway_proof_round ORDER BY round DESC LIMIT 1")
        .fetch_optional(conn)
        .await?;
    row.map(|row| {
        let document = RoundPublication::from_wire(&row.try_get::<Vec<u8>, _>("wire")?)?;
        if row.try_get::<String, _>("digest")? != document.receipt()?.digest
            || row.try_get::<i64, _>("round")? != integer(document.round)?
            || row.try_get::<i64, _>("chain_epoch")? != integer(document.chain_epoch)?
            || row.try_get::<i64, _>("block_number")? != integer(document.block)?
        {
            return Err(Error::Unavailable);
        }
        Ok(document)
    })
    .transpose()
}

async fn check_head(conn: &mut PgConnection, document: &RoundPublication) -> Result<bool, Error> {
    let Some(previous) = head(conn).await? else {
        return Ok(false);
    };
    if previous.round == document.round && previous == *document {
        return Ok(true);
    }
    if previous.round >= document.round
        || previous.chain_epoch > document.chain_epoch
        || previous.block >= document.block
    {
        return Err(Error::Conflict);
    }
    Ok(false)
}
