use std::collections::{BTreeMap, BTreeSet};

use proof_autonomy::{commitment, AdmittedContribution, RoundSnapshot};
use proof_research::ResearchStore;
use sqlx::{types::Json, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    boundary_block, integer, AwardHistory, FinalizedRoundSource, FrozenRound, RoundConfig,
    RoundError, RoundLease,
};

#[derive(Clone)]
pub struct RoundStore {
    pub(crate) pool: PgPool,
    pub(crate) research: ResearchStore,
    pub(crate) config: RoundConfig,
}

impl RoundStore {
    #[must_use]
    pub fn new(pool: PgPool, research: ResearchStore, config: RoundConfig) -> Self {
        Self {
            pool,
            research,
            config,
        }
    }

    /// # Errors
    /// Missing schema or overprivileged service connection.
    pub async fn ready(&self) -> Result<(), RoundError> {
        let valid: bool = sqlx::query_scalar(
            "SELECT current_user = 'base_app' \
             AND NOT has_table_privilege(current_user, 'proof_atlas_round', 'UPDATE,DELETE') \
             AND NOT has_table_privilege(current_user, 'proof_atlas_decision', 'UPDATE,DELETE') \
             AND NOT has_table_privilege(current_user, 'proof_atlas_lease', 'UPDATE,DELETE') \
             AND NOT has_table_privilege(current_user, 'proof_atlas_publication', 'UPDATE,DELETE')",
        )
        .fetch_one(&self.pool)
        .await?;
        if !valid {
            return Err(RoundError::Invalid);
        }
        Ok(())
    }

    /// Freeze one contiguous 360-block round before starting Atlas. A previous
    /// round must be decided and publication-confirmed, not merely proposed.
    /// The chain call happens before opening any database transaction.
    ///
    /// # Errors
    /// Unfinalized boundary, skipped round, conflicting configuration, invalid
    /// corpus, unpublished predecessor or database failure.
    pub async fn freeze(
        &self,
        source: &dyn FinalizedRoundSource,
        round: u64,
    ) -> Result<FrozenRound, RoundError> {
        if let Some(existing) = self.find(round).await? {
            return Ok(existing);
        }
        let block = boundary_block(&self.config, round)?;
        let chain = source.boundary(block)?;
        if chain.block != block
            || chain.metagraph.netuid != self.config.netuid
            || chain.timestamp_ms == 0
        {
            return Err(RoundError::Invalid);
        }
        let corpus = self
            .research
            .corpus(chain.chain_epoch, chain.timestamp_ms)
            .await?;
        let mut tx = serial(&self.pool).await?;
        if let Some(existing) = load_frozen(&mut tx, round).await? {
            self.check(&existing)?;
            return Ok(existing);
        }
        let history = self
            .predecessor(&mut tx, round, chain.chain_epoch, chain.timestamp_ms)
            .await?;
        let history_digest = commitment(&history)?;
        let mut contributions = history;
        for entry in contributions.values_mut() {
            entry.rewardable = false;
            entry.evidence_digests.clear();
        }
        let participants = chain
            .metagraph
            .hotkeys
            .into_iter()
            .enumerate()
            .map(|(uid, key)| {
                Ok((
                    key.try_into().map_err(|_| RoundError::Invalid)?,
                    u16::try_from(uid).map_err(|_| RoundError::Invalid)?,
                ))
            })
            .collect::<Result<Vec<_>, RoundError>>()?;
        let mut evidence = BTreeMap::new();
        for item in corpus {
            let entry = contributions
                .entry(item.contribution_digest)
                .or_insert_with(|| AdmittedContribution {
                    miner_hotkey: item.miner_hotkey,
                    evidence_digests: BTreeSet::new(),
                    rewardable: false,
                    previous: None,
                });
            // A later copy, new account, or rental cannot transfer ownership.
            if entry.miner_hotkey != item.miner_hotkey {
                continue;
            }
            entry.rewardable = participants
                .iter()
                .any(|(key, uid)| key == &entry.miner_hotkey && *uid != 0);
            entry
                .evidence_digests
                .insert(item.summary.evidence_digest.clone());
            evidence.insert(item.summary.evidence_digest.clone(), item.summary);
        }
        let mut frozen = FrozenRound {
            snapshot: RoundSnapshot {
                round,
                anchor_block: self.config.anchor_block,
                finalized_block: block,
                finalized_hash: hex::encode(chain.hash),
                chain_epoch: chain.chain_epoch,
                corpus_digest: String::new(),
                policy_digest: self.config.policy_digest.clone(),
                runtime_digest: self.config.runtime_digest.clone(),
            },
            config: self.config.clone(),
            participants,
            contributions,
            evidence,
            history_digest,
            cutoff_ms: chain.timestamp_ms,
        };
        frozen.snapshot.corpus_digest = frozen.corpus_digest()?;
        frozen.validate()?;
        sqlx::query("INSERT INTO proof_atlas_round (round, digest, document) VALUES ($1, $2, $3)")
            .bind(integer(round)?)
            .bind(commitment(&frozen)?)
            .bind(Json(&frozen))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(frozen)
    }

    async fn predecessor(
        &self,
        conn: &mut PgConnection,
        round: u64,
        epoch: u64,
        cutoff: u64,
    ) -> Result<AwardHistory, RoundError> {
        let latest: Option<i64> = sqlx::query_scalar("SELECT max(round) FROM proof_atlas_round")
            .fetch_one(&mut *conn)
            .await?;
        let Some(latest) = latest else {
            return if round == 0 {
                Ok(AwardHistory::new())
            } else {
                Err(RoundError::Invalid)
            };
        };
        if latest.checked_add(1) != Some(integer(round)?) {
            return Err(RoundError::Invalid);
        }
        let previous = self
            .load_decision(
                conn,
                u64::try_from(latest).map_err(|_| RoundError::Invalid)?,
            )
            .await?;
        let receipt: Option<String> = sqlx::query_scalar(
            "SELECT confirmed_digest FROM proof_atlas_publication WHERE round = $1 AND delivered",
        )
        .bind(latest)
        .fetch_optional(conn)
        .await?
        .flatten();
        if receipt.as_deref() != Some(previous.receipt_digest()?.as_str())
            || previous.frozen.snapshot.chain_epoch > epoch
            || previous.frozen.cutoff_ms > cutoff
        {
            return Err(RoundError::Publication);
        }
        Ok(previous.history)
    }

    /// # Errors
    /// Missing/corrupted round or mismatched operator configuration.
    pub async fn frozen(&self, round: u64) -> Result<FrozenRound, RoundError> {
        self.find(round).await?.ok_or(RoundError::Invalid)
    }

    async fn find(&self, round: u64) -> Result<Option<FrozenRound>, RoundError> {
        let mut conn = self.pool.acquire().await?;
        let frozen = load_frozen(&mut conn, round).await?;
        if let Some(frozen) = &frozen {
            self.check(frozen)?;
        }
        Ok(frozen)
    }

    pub(crate) fn check(&self, frozen: &FrozenRound) -> Result<(), RoundError> {
        frozen.validate()?;
        if frozen.config != self.config {
            return Err(RoundError::Invalid);
        }
        Ok(())
    }

    /// # Errors
    /// Invalid owner/duration, another live owner, decided round or DB failure.
    pub async fn acquire(
        &self,
        round: u64,
        owner: Uuid,
        seconds: u32,
    ) -> Result<RoundLease, RoundError> {
        if owner.is_nil() || !(1..=300).contains(&seconds) {
            return Err(RoundError::Invalid);
        }
        let mut tx = serial(&self.pool).await?;
        self.check(
            &load_frozen(&mut tx, round)
                .await?
                .ok_or(RoundError::Invalid)?,
        )?;
        let fence: i64 = sqlx::query_scalar(
            "INSERT INTO proof_atlas_lease (round, owner, fence, expires_at) \
             SELECT $1, $2, 1, clock_timestamp() + make_interval(secs => $3) \
             WHERE NOT EXISTS(SELECT 1 FROM proof_atlas_decision WHERE round = $1) \
             ON CONFLICT (round) DO UPDATE SET owner = excluded.owner, fence = proof_atlas_lease.fence + 1, \
             expires_at = excluded.expires_at WHERE proof_atlas_lease.expires_at <= clock_timestamp() RETURNING fence",
        ).bind(integer(round)?).bind(owner).bind(f64::from(seconds))
        .fetch_optional(&mut *tx).await?.ok_or(RoundError::Fenced)?;
        tx.commit().await?;
        Ok(RoundLease {
            round,
            owner,
            fence,
        })
    }

    /// # Errors
    /// Expired/superseded ownership, invalid duration or database failure.
    pub async fn renew(&self, lease: &RoundLease, seconds: u32) -> Result<(), RoundError> {
        if !(1..=300).contains(&seconds) {
            return Err(RoundError::Invalid);
        }
        let changed = sqlx::query(
            "UPDATE proof_atlas_lease SET expires_at = clock_timestamp() + make_interval(secs => $4) \
             WHERE round = $1 AND owner = $2 AND fence = $3 AND expires_at > clock_timestamp()",
        ).bind(integer(lease.round)?).bind(lease.owner).bind(lease.fence).bind(f64::from(seconds))
        .execute(&self.pool).await?.rows_affected();
        if changed != 1 {
            return Err(RoundError::Fenced);
        }
        Ok(())
    }

    /// # Errors
    /// Expired/superseded ownership or corrupt frozen input.
    pub async fn authorize(&self, lease: &RoundLease) -> Result<FrozenRound, RoundError> {
        let mut tx = serial(&self.pool).await?;
        guard(&mut tx, lease).await?;
        let frozen = load_frozen(&mut tx, lease.round)
            .await?
            .ok_or(RoundError::Invalid)?;
        self.check(&frozen)?;
        guard(&mut tx, lease).await?;
        tx.commit().await?;
        Ok(frozen)
    }
}

pub(crate) async fn serial(pool: &PgPool) -> Result<Transaction<'static, Postgres>, RoundError> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':atlas-rounds', 0))",
    )
    .execute(&mut *tx)
    .await?;
    Ok(tx)
}

pub(crate) async fn guard(conn: &mut PgConnection, lease: &RoundLease) -> Result<(), RoundError> {
    let valid: Option<i64> = sqlx::query_scalar(
        "SELECT fence FROM proof_atlas_lease WHERE round = $1 AND owner = $2 AND fence = $3 \
         AND expires_at > clock_timestamp() FOR UPDATE",
    )
    .bind(integer(lease.round)?)
    .bind(lease.owner)
    .bind(lease.fence)
    .fetch_optional(conn)
    .await?;
    if valid.is_none() {
        return Err(RoundError::Fenced);
    }
    Ok(())
}

pub(crate) async fn load_frozen(
    conn: &mut PgConnection,
    round: u64,
) -> Result<Option<FrozenRound>, RoundError> {
    let row: Option<(String, Json<FrozenRound>)> =
        sqlx::query_as("SELECT digest, document FROM proof_atlas_round WHERE round = $1")
            .bind(integer(round)?)
            .fetch_optional(conn)
            .await?;
    row.map(|(digest, frozen)| {
        frozen.validate()?;
        if commitment(&frozen.0)? != digest || frozen.snapshot.round != round {
            return Err(RoundError::Invalid);
        }
        Ok(frozen.0)
    })
    .transpose()
}
