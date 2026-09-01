//! Precision scoring for Bounty Challenge.
//!
//! Miner score is displacement vs the previous bounty champion on the set of
//! **adjudicated** reports. The three farming strategies this has to survive
//! are all volume plays, and each gets its own answer:
//!
//! - *Flood the queue with junk and keep whatever sticks.* Precision is
//!   `valid / (valid + malicious)`, so junk is subtracted, not ignored, and a
//!   net-negative miner burns toward uid 0.
//! - *File many real but worthless bugs.* Pay is precision **times impact**,
//!   where impact is the severity operators assigned. A hundred cosmetic typos
//!   are worth less than one authentication bypass, and the arithmetic says so.
//! - *Split one bug across many reports, or re-file known ones.* Duplicates and
//!   already-fixed reports earn nothing, and their share of a miner's
//!   adjudications is a **triage-noise canary** that is deliberately absent
//!   from the paid number: a miner tuning their visible precision cannot see
//!   it, and crossing [`MAX_TRIAGE_NOISE_BPS`] is a hard zero.
//!
//! Evidence is required to be **paid**, never to be **penalized**. A `valid`
//! row with no severity is not creditable, because nobody can say what it was
//! worth; a malicious row always counts against the miner who filed it.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;

use bounty_challenge_task::SCORE_MAX;
use serde::{Deserialize, Serialize};

mod public;
pub use public::{
    holdouts_from_reports, parse_leaderboard, parse_reports, rank_leaderboard, scorable,
    score_plan_from_snapshot, LeaderboardRow, PublicReport, PublicScorePlan, PublicSnapshot,
    PublicStatus,
};

/// Credit applied to a valid unique reproducing bug, before severity weighting.
pub const VALID_CREDIT: i64 = 100;

/// Already-fixed-not-prod: ack only.
pub const ALREADY_FIXED_CREDIT: i64 = 0;

/// Malicious / fabricated / does-not-exist: penalty (burns toward uid 0).
pub const MALICIOUS_CREDIT: i64 = -100;

/// Duplicate of an open report: no extra reward, no penalty.
pub const DUPLICATE_CREDIT: i64 = 0;

/// Minimum valid+malicious adjudications before a miner can displace.
pub const MIN_HOLDOUT_DECIDED: u64 = 3;

/// Precision floor for a champion, in bps.
///
/// Displacing on precision alone is not enough: a miner who beats a sloppy
/// incumbent while still filing mostly junk should not hold the emission.
pub const MIN_PRECISION_BPS: u64 = 6_000;

/// Largest share of a miner's adjudications that may be duplicates or
/// already-fixed re-files, in bps.
///
/// This is the canary, and it is **off** the paid number on purpose. Precision
/// ignores duplicates entirely, so without this a miner can re-file the same
/// finding indefinitely at zero visible cost while burning the triage capacity
/// the whole challenge runs on.
pub const MAX_TRIAGE_NOISE_BPS: u64 = 5_000;

/// Operator severity for a valid report.
///
/// Assigned at adjudication and published on the backend feed. A `valid` row
/// without one is not creditable: an unpriced bug cannot be paid for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Cosmetic or trivial; real, but close to worthless.
    Trivial,
    /// Limited blast radius, no data or consensus exposure.
    Minor,
    /// Breaks a documented behaviour or degrades a live path.
    Major,
    /// Consensus, funds, key material, or availability of a live path.
    Critical,
}

impl Severity {
    /// All severities, cheapest first.
    pub const ALL: [Self; 4] = [Self::Trivial, Self::Minor, Self::Major, Self::Critical];

    /// Impact weight in bps of [`Self::Critical`].
    #[must_use]
    pub const fn weight_bps(self) -> u64 {
        match self {
            Self::Trivial => 625,
            Self::Minor => 2_500,
            Self::Major => 5_000,
            Self::Critical => 10_000,
        }
    }

    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trivial => "trivial",
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Critical => "critical",
        }
    }
}

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

/// Per-miner tallies from adjudicated reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinerHoldout {
    /// Unique reproducing bugs, by operator severity.
    pub valid_by_severity: BTreeMap<Severity, u64>,
    /// Valid reports the operator published without a severity. Not creditable.
    pub valid_unpriced: u64,
    /// Already-fixed-not-prod acks (ignored for precision, counted as noise).
    pub already_fixed: u64,
    /// Malicious / fabricated.
    pub malicious: u64,
    /// Duplicates of an open report (noise).
    pub duplicate: u64,
}

impl MinerHoldout {
    /// Apply one verdict. `severity` is read only for [`Adjudication::Valid`].
    pub fn record(&mut self, v: Adjudication, severity: Option<Severity>) {
        match v {
            Adjudication::Valid => match severity {
                Some(s) => {
                    let slot = self.valid_by_severity.entry(s).or_insert(0);
                    *slot = slot.saturating_add(1);
                }
                None => self.valid_unpriced = self.valid_unpriced.saturating_add(1),
            },
            Adjudication::AlreadyFixedNotProd => {
                self.already_fixed = self.already_fixed.saturating_add(1);
            }
            Adjudication::InvalidMalicious => self.malicious = self.malicious.saturating_add(1),
            Adjudication::Duplicate => self.duplicate = self.duplicate.saturating_add(1),
        }
    }

    /// Valid reports that carry a severity, and so can be paid for.
    #[must_use]
    pub fn valid(&self) -> u64 {
        self.valid_by_severity
            .values()
            .fold(0u64, |a, b| a.saturating_add(*b))
    }

    /// `valid + malicious` — the precision denominator.
    #[must_use]
    pub fn decided(&self) -> u64 {
        self.valid().saturating_add(self.malicious)
    }

    /// Every adjudicated report, priced or not.
    #[must_use]
    pub fn adjudicated(&self) -> u64 {
        self.decided()
            .saturating_add(self.valid_unpriced)
            .saturating_add(self.already_fixed)
            .saturating_add(self.duplicate)
    }

    /// Precision in bps: `valid / (valid + malicious)`. `None` if none decided.
    ///
    /// Duplicates and already-fixed-not-prod do not inflate precision; the
    /// triage-noise canary is what looks at those.
    #[must_use]
    pub fn precision_bps(&self) -> Option<u64> {
        let d = self.decided();
        if d == 0 {
            return None;
        }
        Some(self.valid().saturating_mul(10_000) / d)
    }

    /// Mean severity of the credited reports, in bps of `Critical`.
    ///
    /// This is what stops a hundred cosmetic findings from outranking one
    /// authentication bypass. `None` when nothing is creditable.
    #[must_use]
    pub fn impact_bps(&self) -> Option<u64> {
        let n = self.valid();
        if n == 0 {
            return None;
        }
        let total: u128 = self
            .valid_by_severity
            .iter()
            .map(|(s, count)| u128::from(s.weight_bps()).saturating_mul(u128::from(*count)))
            .sum();
        u64::try_from(total / u128::from(n)).ok()
    }

    /// Share of this miner's adjudications that were duplicates or re-files of
    /// already-fixed issues, in bps. `None` when nothing was adjudicated.
    #[must_use]
    pub fn triage_noise_bps(&self) -> Option<u64> {
        let total = self.adjudicated();
        if total == 0 {
            return None;
        }
        let noise = self.duplicate.saturating_add(self.already_fixed);
        Some(noise.saturating_mul(10_000) / total)
    }

    /// Net credit across all verdicts, severity-weighted (malicious negative).
    #[must_use]
    pub fn net_credit(&self) -> i64 {
        let weighted: i64 = self
            .valid_by_severity
            .iter()
            .map(|(s, count)| {
                let per = VALID_CREDIT
                    .saturating_mul(i64::try_from(s.weight_bps()).unwrap_or(i64::MAX))
                    / 10_000;
                per.saturating_mul(i64::try_from(*count).unwrap_or(i64::MAX))
            })
            .fold(0i64, i64::saturating_add);
        let m = i64::try_from(self.malicious).unwrap_or(i64::MAX);
        let a = i64::try_from(self.already_fixed).unwrap_or(i64::MAX);
        let d = i64::try_from(self.duplicate).unwrap_or(i64::MAX);
        weighted
            .saturating_add(m.saturating_mul(MALICIOUS_CREDIT))
            .saturating_add(a.saturating_mul(ALREADY_FIXED_CREDIT))
            .saturating_add(d.saturating_mul(DUPLICATE_CREDIT))
    }
}

/// Gate that blocked champion displacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateFail {
    /// Not enough decided adjudications.
    ThinHoldout,
    /// Challenger precision is not strictly better.
    NoPrecisionWin,
    /// Challenger precision is below [`MIN_PRECISION_BPS`].
    PrecisionFloor {
        /// Observed precision in bps.
        precision_bps: u64,
    },
    /// Challenger net credit is negative (penalty / burn).
    Penalty,
    /// Champion hold — challenger did not displace.
    Regression,
    /// Valid reports were published without a severity, so nothing about them
    /// can be priced (fail-closed; an unpriced bug is not a free one).
    SeverityEvidenceMissing,
    /// Duplicates and already-fixed re-files dominate this miner's queue.
    ///
    /// Off the visible score: precision cannot see it, so a miner tuning
    /// precision cannot tune this away.
    TriageNoise {
        /// Observed noise share in bps.
        ratio_bps: u64,
    },
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
    /// Challenger mean severity (bps of `Critical`).
    pub challenger_impact_bps: Option<u64>,
    /// Challenger duplicate / already-fixed share (bps). Reported, never paid.
    pub challenger_noise_bps: Option<u64>,
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

/// Map precision and severity onto the score lattice.
///
/// Precision says how much of what this miner filed was real. Impact says how
/// much it was worth. Paying on the product is what makes one critical finding
/// beat a pile of cosmetic ones.
#[must_use]
pub fn lattice_from_precision_and_impact(precision_bps: u64, impact_bps: u64) -> u64 {
    let p = u128::from(precision_bps.min(10_000));
    let i = u128::from(impact_bps.min(10_000));
    let num = u128::from(SCORE_MAX).saturating_mul(p).saturating_mul(i);
    u64::try_from(num / 100_000_000).unwrap_or(0)
}

/// Incumbent keeps a positive lattice so a rejected challenger does not burn
/// the whole bounty share.
#[must_use]
pub fn champion_hold_lattice() -> u64 {
    SCORE_MAX / 2
}

/// Judge challenger vs champion on adjudicated precision and severity.
///
/// Never crowns a regression, and never crowns volume: the lattice is
/// `precision × impact`, and the triage-noise canary can zero a miner whose
/// visible precision looks perfect.
#[must_use]
pub fn judge_challenger(champion: &MinerHoldout, challenger: &MinerHoldout) -> ChampionVerdict {
    let mut failed = Vec::new();
    let chall_p = challenger.precision_bps();
    let champ_p = champion.precision_bps();
    let impact = challenger.impact_bps();
    let noise = challenger.triage_noise_bps();

    if challenger.decided() < MIN_HOLDOUT_DECIDED {
        failed.push(GateFail::ThinHoldout);
    }
    if challenger.net_credit() < 0 {
        failed.push(GateFail::Penalty);
    }
    // Evidence is needed to be paid, not to be penalized: an unpriced valid
    // report cannot be credited, but a malicious one still counts against.
    if challenger.valid_unpriced > 0 {
        failed.push(GateFail::SeverityEvidenceMissing);
    }
    if let Some(p) = chall_p {
        if p < MIN_PRECISION_BPS {
            failed.push(GateFail::PrecisionFloor { precision_bps: p });
        }
    }
    if let Some(n) = noise {
        if n > MAX_TRIAGE_NOISE_BPS {
            failed.push(GateFail::TriageNoise { ratio_bps: n });
        }
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
    let lattice = match (eligible, chall_p, impact) {
        (true, Some(p), Some(i)) => lattice_from_precision_and_impact(p, i),
        _ => 0,
    };

    ChampionVerdict {
        eligible,
        challenger_precision_bps: chall_p,
        champion_precision_bps: champ_p,
        challenger_impact_bps: impact,
        challenger_noise_bps: noise,
        failed,
        lattice,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tally builder: `valid` reports all at `severity`.
    fn hold(
        valid: u64,
        severity: Severity,
        already: u64,
        malicious: u64,
        duplicate: u64,
    ) -> MinerHoldout {
        let mut h = MinerHoldout {
            already_fixed: already,
            malicious,
            duplicate,
            ..MinerHoldout::default()
        };
        if valid > 0 {
            h.valid_by_severity.insert(severity, valid);
        }
        h
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
    fn severity_weights_are_ordered_and_capped_at_critical() {
        assert_eq!(Severity::Critical.weight_bps(), 10_000);
        for pair in Severity::ALL.windows(2) {
            assert!(
                pair[0].weight_bps() < pair[1].weight_bps(),
                "{pair:?} must be ordered"
            );
        }
    }

    #[test]
    fn valid_unique_bug_rewards() {
        let mut h = MinerHoldout::default();
        for _ in 0..3 {
            h.record(Adjudication::Valid, Some(Severity::Critical));
        }
        assert_eq!(h.precision_bps(), Some(10_000));
        assert_eq!(h.impact_bps(), Some(10_000));
        assert_eq!(h.net_credit(), 3 * VALID_CREDIT);
    }

    #[test]
    fn already_fixed_not_prod_is_ack_only() {
        let mut h = MinerHoldout::default();
        h.record(Adjudication::AlreadyFixedNotProd, None);
        h.record(Adjudication::AlreadyFixedNotProd, None);
        assert_eq!(h.precision_bps(), None);
        assert_eq!(h.net_credit(), 0);
        assert_eq!(h.decided(), 0);
    }

    #[test]
    fn malicious_is_penalty() {
        let mut h = MinerHoldout::default();
        for _ in 0..3 {
            h.record(Adjudication::InvalidMalicious, None);
        }
        assert_eq!(h.precision_bps(), Some(0));
        assert!(h.net_credit() < 0);
        let v = judge_challenger(&hold(3, Severity::Major, 0, 0, 0), &h);
        assert!(!v.eligible);
        assert!(v.failed.contains(&GateFail::Penalty));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn duplicate_no_reward_no_penalty() {
        let mut h = MinerHoldout::default();
        h.record(Adjudication::Duplicate, None);
        h.record(Adjudication::Duplicate, None);
        assert_eq!(h.net_credit(), 0);
        assert_eq!(h.precision_bps(), None);
    }

    #[test]
    fn spam_volume_does_not_displace() {
        let champ = hold(6, Severity::Major, 0, 1, 0);
        // Lots of junk + one valid: precision collapses.
        let spam = hold(1, Severity::Major, 0, 20, 40);
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
        let champ = hold(4, Severity::Major, 2, 2, 1);
        let better = hold(8, Severity::Major, 0, 1, 0);
        let v = judge_challenger(&champ, &better);
        assert!(v.eligible, "failed={:?}", v.failed);
        assert!(v.lattice > 0);
        assert!(v.challenger_precision_bps.unwrap_or(0) > v.champion_precision_bps.unwrap_or(0));
    }

    /// The volume play the unweighted score allowed: file cosmetic findings
    /// forever at perfect precision. Severity is what prices them.
    #[test]
    fn a_pile_of_trivia_pays_less_than_one_real_bug() {
        let champ = hold(1, Severity::Trivial, 0, 2, 0);
        let trivia = hold(40, Severity::Trivial, 0, 0, 0);
        let real = hold(4, Severity::Critical, 0, 0, 0);

        let t = judge_challenger(&champ, &trivia);
        let r = judge_challenger(&champ, &real);
        assert!(t.eligible, "{:?}", t.failed);
        assert!(r.eligible, "{:?}", r.failed);
        assert_eq!(t.challenger_precision_bps, r.challenger_precision_bps);
        assert!(
            r.lattice > t.lattice * 4,
            "critical={} trivial={}",
            r.lattice,
            t.lattice
        );
    }

    /// The canary: duplicates are invisible in precision, so a miner tuning
    /// the paid number cannot tune this away.
    #[test]
    fn duplicate_farming_is_a_hard_zero_the_paid_number_cannot_see() {
        let champ = hold(3, Severity::Major, 0, 2, 0);
        let farmer = hold(6, Severity::Major, 4, 0, 30);
        assert_eq!(
            farmer.precision_bps(),
            Some(10_000),
            "precision cannot see the duplicates"
        );
        let v = judge_challenger(&champ, &farmer);
        assert!(
            v.failed
                .iter()
                .any(|f| matches!(f, GateFail::TriageNoise { .. })),
            "{:?}",
            v.failed
        );
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn an_unpriced_valid_report_cannot_be_paid_for() {
        let champ = hold(3, Severity::Major, 0, 2, 0);
        let mut unpriced = hold(6, Severity::Major, 0, 0, 0);
        unpriced.record(Adjudication::Valid, None);
        assert_eq!(unpriced.valid_unpriced, 1);
        let v = judge_challenger(&champ, &unpriced);
        assert!(v.failed.contains(&GateFail::SeverityEvidenceMissing));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn beating_a_sloppy_incumbent_still_needs_the_precision_floor() {
        let sloppy = hold(1, Severity::Major, 0, 9, 0);
        let barely = hold(4, Severity::Major, 0, 6, 0);
        assert_eq!(barely.precision_bps(), Some(4_000));
        let v = judge_challenger(&sloppy, &barely);
        assert!(
            v.failed
                .iter()
                .any(|f| matches!(f, GateFail::PrecisionFloor { .. })),
            "{:?}",
            v.failed
        );
        assert!(!v.eligible);
    }

    #[test]
    fn already_fixed_does_not_inflate_precision() {
        let mut padded = hold(3, Severity::Major, 3, 0, 0);
        padded.record(Adjudication::AlreadyFixedNotProd, None);
        assert_eq!(padded.precision_bps(), Some(10_000));
        assert_eq!(padded.decided(), 3);
    }

    #[test]
    fn lattice_maps_precision_and_impact() {
        assert_eq!(lattice_from_precision(0), 0);
        assert_eq!(lattice_from_precision(10_000), SCORE_MAX);
        assert_eq!(lattice_from_precision(5_000), SCORE_MAX / 2);
        assert_eq!(
            lattice_from_precision_and_impact(10_000, 10_000),
            SCORE_MAX
        );
        assert_eq!(
            lattice_from_precision_and_impact(10_000, 5_000),
            SCORE_MAX / 2
        );
        assert_eq!(lattice_from_precision_and_impact(0, 10_000), 0);
    }

    #[test]
    fn thin_holdout_cannot_crown() {
        let champ = hold(5, Severity::Major, 0, 0, 0);
        let thin = hold(2, Severity::Major, 0, 0, 0);
        let v = judge_challenger(&champ, &thin);
        assert!(v.failed.contains(&GateFail::ThinHoldout));
        assert!(!v.eligible);
    }
}
