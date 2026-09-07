use std::collections::{BTreeMap, BTreeSet};

use bundle::{NoScoreReasonCode, ScoreOrAbsence};
use challenge_common::Hotkey;
use proof_task::SCORE_MAX;
use serde::{Deserialize, Serialize};

use crate::{commitment, is_digest, ContractError};

pub const ATLAS_SCORING_VERSION: u16 = 2;
pub const ROUND_BLOCKS: u64 = 360;
pub const PPM: u32 = 1_000_000;

/// Chain-derived input, persisted before invoking Atlas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundSnapshot {
    pub round: u64,
    pub anchor_block: u64,
    pub finalized_block: u64,
    pub finalized_hash: String,
    pub chain_epoch: u64,
    pub corpus_digest: String,
    pub policy_digest: String,
    pub runtime_digest: String,
}

impl RoundSnapshot {
    /// # Errors
    /// Reject an unclosed window or a missing commitment.
    pub fn validate(&self) -> Result<(), ContractError> {
        let end = self
            .round
            .checked_add(1)
            .and_then(|r| r.checked_mul(ROUND_BLOCKS))
            .and_then(|offset| self.anchor_block.checked_add(offset))
            .ok_or(ContractError::Invalid("round overflow"))?;
        if self.finalized_block != end
            || ![
                &self.finalized_hash,
                &self.corpus_digest,
                &self.policy_digest,
                &self.runtime_digest,
            ]
            .into_iter()
            .all(|s| is_digest(s))
        {
            return Err(ContractError::Invalid("round snapshot"));
        }
        Ok(())
    }
}

/// Atlas selects these parameters per discovery, not a global half-life.
/// The controller never permits a later submission to reset the start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecayPlan {
    pub first_round: u64,
    pub initial_units: u64,
    pub retention_ppm: u32,
    pub expires_round: u64,
}

impl DecayPlan {
    /// # Errors
    /// Invalid or non-decreasing schedule.
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.initial_units == 0
            || self.initial_units > SCORE_MAX
            || self.retention_ppm >= PPM
            || self.expires_round <= self.first_round
        {
            return Err(ContractError::Invalid("decay"));
        }
        Ok(())
    }

    /// Deterministic fixed-point exponentiation, rounding toward less credit.
    #[must_use]
    pub fn cap_at(&self, round: u64) -> u64 {
        if round < self.first_round || round >= self.expires_round {
            return 0;
        }
        let mut age = round - self.first_round;
        let denominator = u128::from(PPM);
        let mut factor = u128::from(self.retention_ppm);
        let mut retained = denominator;
        while age > 0 {
            if age & 1 == 1 {
                retained = retained * factor / denominator;
            }
            factor = factor * factor / denominator;
            age >>= 1;
        }
        u64::try_from(u128::from(self.initial_units) * retained / denominator).unwrap_or(0)
    }
}

/// Controller-owned record. A model cannot set its own admissibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedContribution {
    pub miner_hotkey: Hotkey,
    pub evidence_digests: BTreeSet<String>,
    pub rewardable: bool,
    pub previous: Option<PreviousAward>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PreviousAward {
    pub round: u64,
    pub units: u64,
    pub decay: DecayPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContributionAward {
    pub contribution_digest: String,
    pub miner_hotkey: String,
    pub units: u64,
    pub evidence_digests: Vec<String>,
    pub rationale: String,
    pub decay: DecayPlan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay_revision: Option<DecayRevision>,
}

/// A revision binds the exact prior award, not just a convenient old schedule.
/// The controller retains this inside the immutable decision audit record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecayRevision {
    pub previous_award_digest: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AtlasDecision {
    pub schema_version: u32,
    pub scoring_version: u16,
    pub snapshot: RoundSnapshot,
    pub awards: Vec<ContributionAward>,
    pub rationale: String,
}

/// Validated absolute allocation. The UID-0 residual prevents the existing
/// per-challenge normalization from cancelling an across-the-board decay.
#[derive(Debug, Clone)]
pub struct Allocation {
    pub miner_units: BTreeMap<Hotkey, u64>,
    pub burn_units: u64,
    pub awards: BTreeMap<String, PreviousAward>,
}

impl AtlasDecision {
    /// Admit a proposal against a frozen controller-owned corpus and history.
    ///
    /// # Errors
    /// Stale snapshot, missing evidence, duplicate credit or invalid allocation.
    pub fn validate(
        &self,
        snapshot: &RoundSnapshot,
        contributions: &BTreeMap<String, AdmittedContribution>,
    ) -> Result<Allocation, ContractError> {
        snapshot.validate()?;
        if self.schema_version != 1
            || self.scoring_version != ATLAS_SCORING_VERSION
            || self.snapshot != *snapshot
        {
            return Err(ContractError::Stale);
        }
        validate_rationale(&self.rationale)?;
        if self.awards.len() > 10_000 {
            return Err(ContractError::Allocation);
        }
        let mut allocation = Allocation {
            miner_units: BTreeMap::new(),
            burn_units: SCORE_MAX,
            awards: BTreeMap::new(),
        };
        for award in &self.awards {
            if !is_digest(&award.contribution_digest)
                || allocation.awards.contains_key(&award.contribution_digest)
            {
                return Err(ContractError::Evidence);
            }
            let admitted = contributions
                .get(&award.contribution_digest)
                .filter(|record| record.rewardable)
                .ok_or(ContractError::Evidence)?;
            if award.miner_hotkey != hex::encode(admitted.miner_hotkey)
                || award.evidence_digests.is_empty()
                || award.evidence_digests.len() > 256
                || award.evidence_digests.iter().collect::<BTreeSet<_>>().len()
                    != award.evidence_digests.len()
                || !award
                    .evidence_digests
                    .iter()
                    .all(|digest| is_digest(digest) && admitted.evidence_digests.contains(digest))
            {
                return Err(ContractError::Evidence);
            }
            validate_rationale(&award.rationale)?;
            validate_decay(award, admitted.previous.as_ref(), snapshot.round)?;
            allocation.burn_units = allocation
                .burn_units
                .checked_sub(award.units)
                .ok_or(ContractError::Allocation)?;
            let units = allocation
                .miner_units
                .entry(admitted.miner_hotkey)
                .or_default();
            *units = units
                .checked_add(award.units)
                .ok_or(ContractError::Allocation)?;
            allocation.awards.insert(
                award.contribution_digest.clone(),
                PreviousAward {
                    round: snapshot.round,
                    units: award.units,
                    decay: award.decay.clone(),
                },
            );
        }
        // Omission is zero credit, not forgotten history that can revive an
        // older, larger award.
        for (digest, contribution) in contributions {
            if let Some(previous) = &contribution.previous {
                allocation
                    .awards
                    .entry(digest.clone())
                    .or_insert_with(|| PreviousAward {
                        round: snapshot.round,
                        units: 0,
                        decay: previous.decay.clone(),
                    });
            }
        }
        Ok(allocation)
    }
}

fn validate_decay(
    award: &ContributionAward,
    previous: Option<&PreviousAward>,
    round: u64,
) -> Result<(), ContractError> {
    award.decay.validate()?;
    if let Some(previous) = previous {
        previous.decay.validate()?;
        if previous.round >= round
            || previous.decay.first_round != award.decay.first_round
            || previous.decay.initial_units != award.decay.initial_units
        {
            return Err(ContractError::Stale);
        }
        match (&award.decay_revision, previous.decay == award.decay) {
            (None, true) => {}
            (Some(revision), false) if revision.previous_award_digest == commitment(previous)? => {
                validate_rationale(&revision.rationale)?;
            }
            _ => return Err(ContractError::Stale),
        }
        if award.units > previous.units || (previous.decay.cap_at(round) == 0 && award.units != 0) {
            return Err(ContractError::Allocation);
        }
    } else if award.decay.first_round != round
        || award.decay.initial_units != award.units
        || award.decay_revision.is_some()
    {
        return Err(ContractError::Invalid("first award"));
    }
    if award.units > award.decay.cap_at(round) {
        return Err(ContractError::Allocation);
    }
    Ok(())
}

impl Allocation {
    /// Produce exact-E scores using the *real* UID-0 hotkey from the pinned
    /// metagraph. No synthetic identity or consensus change is necessary.
    ///
    /// # Errors
    /// UID 0 missing, duplicate UIDs, out-of-set award, or attempted UID-0 pay.
    pub fn emission_scores(
        &self,
        expected: &BTreeSet<Hotkey>,
        uid_map: &BTreeMap<Hotkey, u16>,
    ) -> Result<BTreeMap<Hotkey, ScoreOrAbsence>, ContractError> {
        let mut seen = BTreeSet::new();
        if uid_map.values().any(|uid| !seen.insert(*uid)) {
            return Err(ContractError::Scope);
        }
        let burn_key = uid_map
            .iter()
            .find_map(|(key, uid)| (*uid == 0).then_some(*key))
            .filter(|key| expected.contains(key))
            .ok_or(ContractError::Scope)?;
        if self
            .miner_units
            .keys()
            .any(|key| key == &burn_key || !expected.contains(key) || !uid_map.contains_key(key))
        {
            return Err(ContractError::Scope);
        }
        let total = self
            .miner_units
            .values()
            .try_fold(self.burn_units, |total, units| total.checked_add(*units));
        if total != Some(SCORE_MAX) {
            return Err(ContractError::Allocation);
        }
        Ok(expected
            .iter()
            .map(|key| {
                let units = if *key == burn_key {
                    self.burn_units
                } else {
                    self.miner_units.get(key).copied().unwrap_or(0)
                };
                let score = if units == 0 {
                    ScoreOrAbsence::NoScore {
                        reason: NoScoreReasonCode::NotAttempted,
                    }
                } else {
                    ScoreOrAbsence::Score { value: units }
                };
                (*key, score)
            })
            .collect())
    }
}

fn validate_rationale(text: &str) -> Result<(), ContractError> {
    if text.trim().is_empty() || text.len() > 16_384 {
        return Err(ContractError::Invalid("rationale"));
    }
    Ok(())
}
