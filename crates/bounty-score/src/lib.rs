//! Precision scoring for Bounty Challenge.
//!
//! Miner score is displacement vs the previous bounty champion on a holdout
//! of adjudicated reports. Volume does not help — stuffing junk lowers
//! precision and cannot crown a champion.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use bounty_challenge_task::SCORE_MAX;
use serde::{Deserialize, Serialize};

/// Credit applied to a valid unique reproducing bug.
pub const VALID_CREDIT: i64 = 100;

/// Already-fixed-not-prod: ack only.
pub const ALREADY_FIXED_CREDIT: i64 = 0;

/// Malicious / fabricated / does-not-exist: penalty (burns toward uid 0).
pub const MALICIOUS_CREDIT: i64 = -100;

/// Duplicate of an open report: no extra reward, no penalty.
pub const DUPLICATE_CREDIT: i64 = 0;

/// Minimum valid+malicious holdout items before a miner can displace.
pub const MIN_HOLDOUT_DECIDED: u64 = 3;

/// Operator verdict on one report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adjudication {
    /// Unique bug that reproduces. Reward.
    Valid,
    /// Bug already fixed, not yet in prod. Ack only.
    AlreadyFixedNotProd,
    /// Fabricated, malicious, or does not exist. Penalty.
    InvalidMalicious,
    /// Duplicate of an open report. Small/no reward, no penalty.
    Duplicate,
}

impl Adjudication {
    /// Lattice credit for this verdict (can be negative).
    #[must_use]
    pub fn credit(self) -> i64 {
        match self {
            Self::Valid => VALID_CREDIT,
            Self::AlreadyFixedNotProd => ALREADY_FIXED_CREDIT,
            Self::InvalidMalicious => MALICIOUS_CREDIT,
            Self::Duplicate => DUPLICATE_CREDIT,
        }
    }

    /// True when the item is a precision-decided holdout example.
    #[must_use]
    pub fn counts_for_precision(self) -> bool {
        matches!(self, Self::Valid | Self::InvalidMalicious)
    }
}

/// Per-miner holdout tallies from adjudicated reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinerHoldout {
    /// Unique reproducing bugs.
    pub valid: u64,
    /// Already-fixed-not-prod acks (ignored for precision).
    pub already_fixed: u64,
    /// Malicious / fabricated.
    pub malicious: u64,
    /// Duplicates of an open report.
    pub duplicate: u64,
}

impl MinerHoldout {
    /// Apply one verdict.
    pub fn record(&mut self, v: Adjudication) {
        match v {
            Adjudication::Valid => self.valid = self.valid.saturating_add(1),
            Adjudication::AlreadyFixedNotProd => {
                self.already_fixed = self.already_fixed.saturating_add(1);
            }
            Adjudication::InvalidMalicious => self.malicious = self.malicious.saturating_add(1),
            Adjudication::Duplicate => self.duplicate = self.duplicate.saturating_add(1),
        }
    }

    /// `valid + malicious` — the precision denominator.
    #[must_use]
    pub fn decided(&self) -> u64 {
        self.valid.saturating_add(self.malicious)
    }

    /// Precision in bps: `valid / (valid + malicious)`. `None` if no decided items.
    ///
    /// Duplicates and already-fixed-not-prod do not inflate precision.
    #[must_use]
    pub fn precision_bps(&self) -> Option<u64> {
        let d = self.decided();
        if d == 0 {
            return None;
        }
        Some((self.valid.saturating_mul(10_000)) / d)
    }

    /// Net credit across all verdicts (malicious is negative).
    #[must_use]
    pub fn net_credit(&self) -> i64 {
        let v = i64::try_from(self.valid).unwrap_or(i64::MAX);
        let m = i64::try_from(self.malicious).unwrap_or(i64::MAX);
        let a = i64::try_from(self.already_fixed).unwrap_or(i64::MAX);
        let d = i64::try_from(self.duplicate).unwrap_or(i64::MAX);
        v.saturating_mul(VALID_CREDIT)
            .saturating_add(m.saturating_mul(MALICIOUS_CREDIT))
            .saturating_add(a.saturating_mul(ALREADY_FIXED_CREDIT))
            .saturating_add(d.saturating_mul(DUPLICATE_CREDIT))
    }
}

/// Gate that blocked champion displacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateFail {
    /// Not enough decided holdout items.
    ThinHoldout,
    /// Challenger precision is not strictly better.
    NoPrecisionWin,
    /// Challenger net credit is negative (penalty / burn).
    Penalty,
    /// Champion hold — challenger did not displace.
    Regression,
}

/// Champion / challenger verdict. Consensus-critical once leaves are signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChampionVerdict {
    /// Challenger may become the live champion.
    pub eligible: bool,
    /// Challenger precision (bps) when decided.
    pub challenger_precision_bps: Option<u64>,
    /// Champion precision (bps) when decided.
    pub champion_precision_bps: Option<u64>,
    /// Gates that failed (empty ⇒ displace).
    pub failed: Vec<GateFail>,
    /// Lattice to emit if this hotkey is the live champion.
    pub lattice: u64,
}

/// Map precision bps onto the score lattice.
#[must_use]
pub fn lattice_from_precision(precision_bps: u64) -> u64 {
    let clamped = precision_bps.min(10_000);
    let num = u128::from(SCORE_MAX).saturating_mul(u128::from(clamped));
    u64::try_from(num / 10_000).unwrap_or(0)
}

/// Incumbent keeps a positive lattice so a rejected challenger does not burn
/// the whole bounty share.
#[must_use]
pub fn champion_hold_lattice() -> u64 {
    SCORE_MAX / 2
}

/// Judge challenger vs champion on holdout precision. Never crowns a regression.
#[must_use]
pub fn judge_challenger(champion: &MinerHoldout, challenger: &MinerHoldout) -> ChampionVerdict {
    let mut failed = Vec::new();
    let chall_p = challenger.precision_bps();
    let champ_p = champion.precision_bps();

    if challenger.decided() < MIN_HOLDOUT_DECIDED {
        failed.push(GateFail::ThinHoldout);
    }
    if challenger.net_credit() < 0 {
        failed.push(GateFail::Penalty);
    }

    match (chall_p, champ_p) {
        (Some(c), Some(h)) if c > h => {}
        (Some(_), None) => {}
        (Some(_), Some(_)) => {
            failed.push(GateFail::NoPrecisionWin);
            failed.push(GateFail::Regression);
        }
        (None, _) => {
            if !failed.contains(&GateFail::ThinHoldout) {
                failed.push(GateFail::ThinHoldout);
            }
            failed.push(GateFail::NoPrecisionWin);
        }
    }

    failed.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    failed.dedup();

    let eligible = failed.is_empty();
    let lattice = if eligible {
        chall_p.map_or(0, lattice_from_precision)
    } else {
        0
    };

    ChampionVerdict {
        eligible,
        challenger_precision_bps: chall_p,
        champion_precision_bps: champ_p,
        failed,
        lattice,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold(valid: u64, already: u64, malicious: u64, duplicate: u64) -> MinerHoldout {
        MinerHoldout {
            valid,
            already_fixed: already,
            malicious,
            duplicate,
        }
    }

    #[test]
    fn credits_match_spec() {
        assert_eq!(Adjudication::Valid.credit(), VALID_CREDIT);
        assert!(Adjudication::Valid.credit() > 0);
        assert_eq!(Adjudication::AlreadyFixedNotProd.credit(), 0);
        assert!(Adjudication::InvalidMalicious.credit() < 0);
        assert_eq!(Adjudication::Duplicate.credit(), 0);
    }

    #[test]
    fn valid_unique_bug_rewards() {
        let mut h = MinerHoldout::default();
        h.record(Adjudication::Valid);
        h.record(Adjudication::Valid);
        h.record(Adjudication::Valid);
        assert_eq!(h.precision_bps(), Some(10_000));
        assert_eq!(h.net_credit(), 3 * VALID_CREDIT);
    }

    #[test]
    fn already_fixed_not_prod_is_ack_only() {
        let mut h = MinerHoldout::default();
        h.record(Adjudication::AlreadyFixedNotProd);
        h.record(Adjudication::AlreadyFixedNotProd);
        assert_eq!(h.precision_bps(), None);
        assert_eq!(h.net_credit(), 0);
        assert_eq!(h.decided(), 0);
    }

    #[test]
    fn malicious_is_penalty() {
        let mut h = MinerHoldout::default();
        h.record(Adjudication::InvalidMalicious);
        h.record(Adjudication::InvalidMalicious);
        h.record(Adjudication::InvalidMalicious);
        assert_eq!(h.precision_bps(), Some(0));
        assert!(h.net_credit() < 0);
        let champ = hold(3, 0, 0, 0);
        let v = judge_challenger(&champ, &h);
        assert!(!v.eligible);
        assert!(v.failed.contains(&GateFail::Penalty));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn duplicate_no_reward_no_penalty() {
        let mut h = MinerHoldout::default();
        h.record(Adjudication::Duplicate);
        h.record(Adjudication::Duplicate);
        assert_eq!(h.net_credit(), 0);
        assert_eq!(h.precision_bps(), None);
    }

    #[test]
    fn spam_volume_does_not_displace() {
        let champ = hold(6, 0, 1, 0);
        // Lots of junk + one valid: precision collapses.
        let spam = hold(1, 0, 20, 40);
        let v = judge_challenger(&champ, &spam);
        assert!(!v.eligible);
        assert!(
            v.failed.contains(&GateFail::NoPrecisionWin)
                || v.failed.contains(&GateFail::Penalty)
                || v.failed.contains(&GateFail::Regression)
        );
    }

    #[test]
    fn higher_precision_displaces() {
        let champ = hold(4, 2, 2, 1);
        let better = hold(8, 0, 1, 0);
        let v = judge_challenger(&champ, &better);
        assert!(v.eligible, "failed={:?}", v.failed);
        assert!(v.lattice > 0);
        assert!(v.challenger_precision_bps.unwrap_or(0) > v.champion_precision_bps.unwrap_or(0));
    }

    #[test]
    fn already_fixed_does_not_inflate_precision() {
        let mut padded = hold(3, 50, 0, 0);
        padded.record(Adjudication::AlreadyFixedNotProd);
        assert_eq!(padded.precision_bps(), Some(10_000));
        assert_eq!(padded.decided(), 3);
    }

    #[test]
    fn lattice_maps_precision() {
        assert_eq!(lattice_from_precision(0), 0);
        assert_eq!(lattice_from_precision(10_000), SCORE_MAX);
        assert_eq!(lattice_from_precision(5_000), SCORE_MAX / 2);
    }

    #[test]
    fn thin_holdout_cannot_crown() {
        let champ = hold(5, 0, 0, 0);
        let thin = hold(2, 0, 0, 0);
        let v = judge_challenger(&champ, &thin);
        assert!(v.failed.contains(&GateFail::ThinHoldout));
        assert!(!v.eligible);
    }
}
