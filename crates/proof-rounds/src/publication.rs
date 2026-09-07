use std::time::Duration;

use async_trait::async_trait;
use parity_scale_codec::{DecodeAll, Encode};
use proof_autonomy::{commitment, AtlasDecision};
use proof_research::artifact_digest;
use sqlx::{types::Json, PgConnection, Row};

pub use proof_publication::{Client as GatewayRoundPublisher, RoundPublication};

use crate::{
    integer,
    store::{guard, load_frozen, serial},
    AwardHistory, FrozenRound, RoundError, RoundLease, RoundStore,
};

#[derive(Debug, Clone)]
pub struct DecidedRound {
    pub frozen: FrozenRound,
    pub decision: AtlasDecision,
    pub history: AwardHistory,
    pub leaves: Vec<u8>,
    pub publication_signature: String,
}

impl DecidedRound {
    /// # Errors
    /// Invalid canonical commitments.
    pub fn publication(&self) -> Result<RoundPublication, RoundError> {
        Ok(RoundPublication {
            round: self.frozen.snapshot.round,
            chain_epoch: self.frozen.snapshot.chain_epoch,
            netuid: self.frozen.config.netuid,
            block: self.frozen.snapshot.finalized_block,
            block_hash: self.frozen.snapshot.finalized_hash.clone(),
            frozen_digest: commitment(&self.frozen)?,
            decision_digest: commitment(&self.decision)?,
            leaves: self.leaves.clone(),
            signature: self.publication_signature.clone(),
        })
    }

    pub(crate) fn receipt_digest(&self) -> Result<String, RoundError> {
        Ok(commitment(&self.publication()?)?)
    }

    /// # Errors
    /// Stored allocation, history, roster, signature or wire-byte corruption.
    pub fn validate(&self) -> Result<(), RoundError> {
        self.frozen.validate()?;
        let allocation = self
            .decision
            .validate(&self.frozen.snapshot, &self.frozen.contributions)?;
        if history(&self.frozen, &allocation.awards) != self.history
            || self.leaves.len() > 8 * 1024 * 1024
        {
            return Err(RoundError::Invalid);
        }
        let scores = allocation.emission_scores(
            &self.frozen.expected(),
            &self.frozen.participants.iter().copied().collect(),
        )?;
        let leaves = Vec::<bundle::LeafV1>::decode_all(&mut self.leaves.as_slice())
            .map_err(|_| RoundError::Invalid)?;
        let public: [u8; 32] = hex::decode(&self.frozen.config.proof_public_key)
            .map_err(|_| RoundError::Invalid)?
            .try_into()
            .map_err(|_| RoundError::Invalid)?;
        self.publication()?
            .verify(&public)
            .map_err(|_| RoundError::Invalid)?;
        if leaves.len() != scores.len() {
            return Err(RoundError::Invalid);
        }
        for (leaf, (key, score)) in leaves.iter().zip(&scores) {
            if leaf.challenge_id != b"proof"
                || leaf.epoch != self.frozen.snapshot.chain_epoch
                || leaf.miner_hotkey != *key
                || leaf.score_or_absence != *score
            {
                return Err(RoundError::Invalid);
            }
            challenge_common::verify_leaf_sig(leaf, &public).map_err(|_| RoundError::Invalid)?;
        }
        Ok(())
    }
}

fn history(
    frozen: &FrozenRound,
    awards: &std::collections::BTreeMap<String, proof_autonomy::PreviousAward>,
) -> AwardHistory {
    frozen
        .contributions
        .iter()
        .filter_map(|(digest, contribution)| {
            awards.get(digest).map(|award| {
                let mut entry = contribution.clone();
                entry.previous = Some(award.clone());
                (digest.clone(), entry)
            })
        })
        .collect()
}

/// Trusted transport must atomically reject an older round replacing newer
/// weights, upsert exact bytes, and verify the complete batch remotely before
/// returning its canonical commitment. Plain POST success or HTTP 409 is NOT
/// confirmation. The legacy raw-weight HTTP client does not meet this contract.
#[async_trait]
pub trait RoundPublisher: Send + Sync {
    async fn publish(&self, document: &RoundPublication) -> Result<String, RoundError>;
}

#[async_trait]
impl RoundPublisher for GatewayRoundPublisher {
    async fn publish(&self, document: &RoundPublication) -> Result<String, RoundError> {
        GatewayRoundPublisher::publish(self, document)
            .await
            .map_err(|_| RoundError::Publication)
    }
}

impl RoundStore {
    /// Validate a proposal, sign once outside the agent, and atomically retain
    /// the decision/history/bytes/outbox under current controller ownership.
    ///
    /// # Errors
    /// Stale lease, wrong signer, inadmissible credit, conflicting decision or DB failure.
    pub async fn decide(
        &self,
        lease: &RoundLease,
        decision: &AtlasDecision,
        secret: &[u8; 32],
    ) -> Result<DecidedRound, RoundError> {
        let mut tx = serial(&self.pool).await?;
        guard(&mut tx, lease).await?;
        let frozen = load_frozen(&mut tx, lease.round)
            .await?
            .ok_or(RoundError::Invalid)?;
        self.check(&frozen)?;
        let public =
            challenge_common::public_key_from_secret(secret).map_err(|_| RoundError::Invalid)?;
        if hex::encode(public) != self.config.proof_public_key {
            return Err(RoundError::Invalid);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_atlas_decision WHERE round = $1)",
        )
        .bind(integer(lease.round)?)
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            let existing = self.load_decision(&mut tx, lease.round).await?;
            if existing.decision != *decision {
                return Err(RoundError::Invalid);
            }
            guard(&mut tx, lease).await?;
            tx.commit().await?;
            return Ok(existing);
        }
        let allocation = decision.validate(&frozen.snapshot, &frozen.contributions)?;
        self.check_credit(&frozen, decision).await?;
        let scores = allocation.emission_scores(
            &frozen.expected(),
            &frozen.participants.iter().copied().collect(),
        )?;
        let leaves: Vec<_> = challenge_common::emit_signed_leaf_set(
            secret,
            b"proof",
            frozen.snapshot.chain_epoch,
            &frozen.expected(),
            &scores,
        )
        .map_err(|_| RoundError::Invalid)?
        .into_values()
        .collect();
        let mut decided = DecidedRound {
            history: history(&frozen, &allocation.awards),
            frozen,
            decision: decision.clone(),
            leaves: leaves.encode(),
            publication_signature: String::new(),
        };
        let mut publication = decided.publication()?;
        publication.sign(secret).map_err(|_| RoundError::Invalid)?;
        decided.publication_signature = publication.signature;
        decided.validate()?;
        sqlx::query(
            "INSERT INTO proof_atlas_decision (round, digest, decision, history, chain_epoch, leaves, leaves_digest, publication_signature) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        ).bind(integer(lease.round)?).bind(commitment(decision)?).bind(Json(decision)).bind(Json(&decided.history))
        .bind(integer(decided.frozen.snapshot.chain_epoch)?).bind(&decided.leaves).bind(artifact_digest(&decided.leaves))
        .bind(&decided.publication_signature)
        .execute(&mut *tx).await?;
        sqlx::query("INSERT INTO proof_atlas_publication (round) VALUES ($1)")
            .bind(integer(lease.round)?)
            .execute(&mut *tx)
            .await?;
        guard(&mut tx, lease).await?;
        tx.commit().await?;
        Ok(decided)
    }

    /// # Errors
    /// Missing or corrupt frozen decision, history or signed leaves.
    pub async fn decision(&self, round: u64) -> Result<DecidedRound, RoundError> {
        let mut conn = self.pool.acquire().await?;
        self.load_decision(&mut conn, round).await
    }

    pub(crate) async fn load_decision(
        &self,
        conn: &mut PgConnection,
        round: u64,
    ) -> Result<DecidedRound, RoundError> {
        let frozen = load_frozen(conn, round).await?.ok_or(RoundError::Invalid)?;
        self.check(&frozen)?;
        let row = sqlx::query("SELECT * FROM proof_atlas_decision WHERE round = $1")
            .bind(integer(round)?)
            .fetch_one(conn)
            .await?;
        let decision: Json<AtlasDecision> = row.try_get("decision")?;
        let history: Json<AwardHistory> = row.try_get("history")?;
        let leaves: Vec<u8> = row.try_get("leaves")?;
        if commitment(&decision.0)? != row.try_get::<String, _>("digest")?
            || artifact_digest(&leaves) != row.try_get::<String, _>("leaves_digest")?
            || integer(frozen.snapshot.chain_epoch)? != row.try_get::<i64, _>("chain_epoch")?
        {
            return Err(RoundError::Invalid);
        }
        let decided = DecidedRound {
            frozen,
            decision: decision.0,
            history: history.0,
            leaves,
            publication_signature: row.try_get("publication_signature")?,
        };
        decided.validate()?;
        Ok(decided)
    }

    /// Replay retained bytes only. This never publishes a new model proposal.
    ///
    /// # Errors
    /// Corrupt bytes, active delivery, transport uncertainty, bad receipt or fencing.
    pub async fn publish(
        &self,
        round: u64,
        publisher: &dyn RoundPublisher,
    ) -> Result<(), RoundError> {
        let decided = self.decision(round).await?;
        self.check_credit(&decided.frozen, &decided.decision)
            .await?;
        let document = decided.publication()?;
        let expected = commitment(&document)?;
        let delivered: Option<String> = sqlx::query_scalar(
            "SELECT confirmed_digest FROM proof_atlas_publication WHERE round = $1 AND delivered",
        )
        .bind(integer(round)?)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        if let Some(delivered) = delivered {
            return if delivered == expected {
                Ok(())
            } else {
                Err(RoundError::Publication)
            };
        }
        let fence: i64 = sqlx::query_scalar(
            "UPDATE proof_atlas_publication SET fence = fence + 1, expires_at = clock_timestamp() + interval '60 seconds' \
             WHERE round = $1 AND NOT delivered AND expires_at <= clock_timestamp() RETURNING fence",
        ).bind(integer(round)?).fetch_optional(&self.pool).await?.ok_or(RoundError::Publication)?;
        let result =
            tokio::time::timeout(Duration::from_secs(30), publisher.publish(&document)).await;
        if !matches!(result, Ok(Ok(ref receipt)) if receipt == &expected) {
            sqlx::query("UPDATE proof_atlas_publication SET expires_at = clock_timestamp() WHERE round = $1 AND fence = $2 AND NOT delivered")
                .bind(integer(round)?).bind(fence).execute(&self.pool).await?;
            return Err(RoundError::Publication);
        }
        let changed = sqlx::query(
            "UPDATE proof_atlas_publication SET delivered = true, confirmed_digest = $3 \
             WHERE round = $1 AND fence = $2 AND NOT delivered AND expires_at > clock_timestamp()",
        )
        .bind(integer(round)?)
        .bind(fence)
        .bind(expected)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(RoundError::Fenced);
        }
        Ok(())
    }

    async fn check_credit(
        &self,
        frozen: &FrozenRound,
        decision: &AtlasDecision,
    ) -> Result<(), RoundError> {
        for award in &decision.awards {
            for digest in &award.evidence_digests {
                if self.research.rewardable(digest).await?
                    != *frozen.evidence.get(digest).ok_or(RoundError::Invalid)?
                {
                    return Err(RoundError::Invalid);
                }
            }
        }
        Ok(())
    }
}
