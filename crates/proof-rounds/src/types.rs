use std::collections::{BTreeMap, BTreeSet};

use chain_live::{FinalizedSnapshot, LiveChainClient};
use proof_autonomy::{commitment, is_digest, AdmittedContribution, RoundSnapshot, ROUND_BLOCKS};
use proof_research::PublicEvidence;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::RoundError;

/// Trusted chain adapter. Tests may simulate it; agents cannot implement it.
pub trait FinalizedRoundSource {
    /// # Errors
    /// Unfinalized block, missing hash-pinned state or transport failure.
    fn boundary(&self, block: u64) -> Result<FinalizedSnapshot, RoundError>;
}

impl FinalizedRoundSource for LiveChainClient {
    fn boundary(&self, block: u64) -> Result<FinalizedSnapshot, RoundError> {
        self.finalized_snapshot(block)
            .map_err(|_| RoundError::Invalid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundConfig {
    pub netuid: u16,
    pub anchor_block: u64,
    pub policy_digest: String,
    pub runtime_digest: String,
    pub proof_public_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenRound {
    pub snapshot: RoundSnapshot,
    pub config: RoundConfig,
    pub participants: Vec<([u8; 32], u16)>,
    pub contributions: BTreeMap<String, AdmittedContribution>,
    pub evidence: BTreeMap<String, PublicEvidence>,
    pub history_digest: String,
    pub cutoff_ms: u64,
}

impl FrozenRound {
    pub(crate) fn corpus_digest(&self) -> Result<String, RoundError> {
        Ok(commitment(&(
            &self.participants,
            &self.contributions,
            &self.evidence,
            &self.history_digest,
            self.cutoff_ms,
        ))?)
    }

    /// # Errors
    /// Corrupt/mismatched commitments, roster, evidence membership or boundary.
    pub fn validate(&self) -> Result<(), RoundError> {
        self.snapshot.validate()?;
        if self.snapshot.anchor_block != self.config.anchor_block
            || self.snapshot.policy_digest != self.config.policy_digest
            || self.snapshot.runtime_digest != self.config.runtime_digest
            || self.snapshot.chain_epoch == 0
            || self.snapshot.finalized_hash == "0".repeat(64)
            || self.cutoff_ms == 0
            || !is_digest(&self.config.proof_public_key)
            || !is_digest(&self.history_digest)
            || self.snapshot.corpus_digest != self.corpus_digest()?
            || self.participants.is_empty()
            || self.participants.len() > 16_384
            || self
                .participants
                .iter()
                .map(|(h, _)| h)
                .collect::<BTreeSet<_>>()
                .len()
                != self.participants.len()
            || self
                .participants
                .iter()
                .map(|(_, u)| u)
                .collect::<BTreeSet<_>>()
                .len()
                != self.participants.len()
            || !self.participants.iter().any(|(_, uid)| *uid == 0)
            || self.contributions.len() > 10_000
            || self.evidence.len() > 10_000
            || self.contributions.iter().any(|(id, c)| {
                !is_digest(id)
                    || c.evidence_digests.len() > 256
                    || (c.rewardable
                        && (c.evidence_digests.is_empty()
                            || !self
                                .participants
                                .iter()
                                .any(|(key, uid)| key == &c.miner_hotkey && *uid != 0)))
                    || c.evidence_digests
                        .iter()
                        .any(|d| !self.evidence.contains_key(d))
            })
            || self.evidence.iter().any(|(digest, e)| {
                digest != &e.evidence_digest
                    || !is_digest(digest)
                    || !e.passed
                    || !e.primary_mean.is_finite()
                    || !e.primary_standard_error.is_finite()
            })
        {
            return Err(RoundError::Invalid);
        }
        Ok(())
    }

    #[must_use]
    pub fn expected(&self) -> BTreeSet<[u8; 32]> {
        self.participants.iter().map(|(h, _)| *h).collect()
    }
}

pub type AwardHistory = BTreeMap<String, AdmittedContribution>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundLease {
    pub round: u64,
    pub owner: Uuid,
    pub fence: i64,
}

pub(crate) fn integer(value: u64) -> Result<i64, RoundError> {
    i64::try_from(value).map_err(|_| RoundError::Invalid)
}

pub(crate) fn boundary_block(config: &RoundConfig, round: u64) -> Result<u64, RoundError> {
    round
        .checked_add(1)
        .and_then(|r| r.checked_mul(ROUND_BLOCKS))
        .and_then(|b| b.checked_add(config.anchor_block))
        .ok_or(RoundError::Invalid)
}
