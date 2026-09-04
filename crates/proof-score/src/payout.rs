//! Epoch payout: WTA and discovery over per-topic pass gates.
//!
//! Pass / reject is still [`crate::judge_topic`]. This module turns those
//! binary lattices plus the primary metric into a **sum** of topic masses
//! (not a mean of 0/SCORE_MAX bits). Empty open set is a host problem —
//! callers must not emit a paid 0.

use std::collections::BTreeMap;

use proof_task::{
    MetricDirection, MetricFamily, PayoutMode, TopicDocument, BPS_DENOM, METRIC_STEP_LATENCY_MS,
    METRIC_TOKENS_PER_SEC, PRIMARY_HOLDOUT_NLL, SCORE_MAX,
};

use crate::{HarnessMetrics, SealedBaseline};

/// Proof challenge emission share in basis points. Topic pools split this
/// equally across currently open ids; the leaf lattice still lives on
/// [`SCORE_MAX`].
pub const PROOF_SHARE_BPS: u16 = 8_000;

/// One miner's best attempt on one topic this epoch.
#[derive(Debug, Clone, PartialEq)]
pub struct MinerTopicRun {
    /// Harness + agent gates passed.
    pub pass: bool,
    /// Primary metric the family scores. Missing → cannot win WTA / novelty.
    pub primary: Option<f64>,
    /// Artifact digest (exact match ⇒ near-duplicate of another pass).
    pub artifact_digest: String,
    /// Agent- or operator-flagged near-duplicate (floor only, no novelty).
    pub near_duplicate: bool,
}

/// Primary the harness measured for this topic family.
#[must_use]
pub fn primary_from_harness(topic: &TopicDocument, harness: &HarnessMetrics) -> Option<f64> {
    primary_metric(
        topic,
        harness.holdout_nll,
        PrimaryExtras {
            tokens_per_sec: harness.tokens_per_sec,
            step_latency_ms: harness.step_latency_ms,
            custom_value: harness.custom_value,
        },
    )
}

/// Primary the harness measured for this topic family.
#[must_use]
pub fn primary_metric(
    topic: &TopicDocument,
    holdout_nll: f64,
    extra: PrimaryExtras,
) -> Option<f64> {
    let value = match topic.metric.family {
        MetricFamily::Nll => {
            if topic.metric.primary.trim() == PRIMARY_HOLDOUT_NLL {
                Some(holdout_nll)
            } else {
                None
            }
        }
        MetricFamily::Throughput => match topic.metric.primary.trim() {
            METRIC_TOKENS_PER_SEC => extra.tokens_per_sec,
            METRIC_STEP_LATENCY_MS => extra.step_latency_ms,
            _ => None,
        },
        MetricFamily::Custom => extra.custom_value,
    };
    value.filter(|v| v.is_finite())
}

/// Optional family-specific primaries (throughput / custom).
#[derive(Debug, Clone, Copy, Default)]
pub struct PrimaryExtras {
    /// Throughput tokens/sec.
    pub tokens_per_sec: Option<f64>,
    /// Throughput step latency.
    pub step_latency_ms: Option<f64>,
    /// Custom metric value.
    pub custom_value: Option<f64>,
}

/// Sealed (or champion) primary used as the discovery novelty bar.
#[must_use]
pub fn sealed_primary(topic: &TopicDocument, sealed: &SealedBaseline) -> Option<f64> {
    primary_metric(
        topic,
        sealed.holdout_nll,
        PrimaryExtras {
            tokens_per_sec: sealed.tokens_per_sec,
            step_latency_ms: sealed.step_latency_ms,
            custom_value: sealed.custom_value,
        },
    )
}

/// Equal split of the challenge's emission share across `n` open topics.
#[must_use]
pub fn topic_share_bps(n: usize) -> u16 {
    if n == 0 {
        return 0;
    }
    u16::try_from(u32::from(PROOF_SHARE_BPS) / u32::try_from(n).unwrap_or(u32::MAX)).unwrap_or(0)
}

/// Per-topic lattice masses that sum to [`SCORE_MAX`].
#[must_use]
pub fn topic_masses(n: usize) -> Vec<u64> {
    if n == 0 {
        return Vec::new();
    }
    let n_u = n as u64;
    let base = SCORE_MAX / n_u;
    let rem = SCORE_MAX % n_u;
    (0..n)
        .map(|i| {
            if (i as u64) < rem {
                base.saturating_add(1)
            } else {
                base
            }
        })
        .collect()
}

/// Paid lattice per miner: **sum** of per-topic masses (WTA / discovery).
///
/// `runs` is miner hex → topic id → attempt. A skipped topic contributes 0.
/// Near-duplicates of another accepted artifact keep the pass floor (discovery)
/// and get 0 novelty weight. Exact WTA ties split the topic mass equally.
#[must_use]
pub fn payout_lattices(
    topics: &[TopicDocument],
    sealed: &BTreeMap<String, SealedBaseline>,
    champion_primary: &BTreeMap<String, f64>,
    runs: &BTreeMap<String, BTreeMap<String, MinerTopicRun>>,
) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    if topics.is_empty() {
        return out;
    }
    let masses = topic_masses(topics.len());
    for (topic, mass) in topics.iter().zip(masses) {
        let pieces = score_one_topic(
            topic,
            mass,
            sealed.get(&topic.id),
            champion_primary.get(&topic.id).copied(),
            runs,
        );
        for (miner, piece) in pieces {
            out.entry(miner)
                .and_modify(|v| *v = v.saturating_add(piece))
                .or_insert(piece);
        }
    }
    for v in out.values_mut() {
        *v = (*v).min(SCORE_MAX);
    }
    out
}

fn score_one_topic(
    topic: &TopicDocument,
    mass: u64,
    sealed: Option<&SealedBaseline>,
    champion: Option<f64>,
    runs: &BTreeMap<String, BTreeMap<String, MinerTopicRun>>,
) -> BTreeMap<String, u64> {
    let mut passers: Vec<(String, MinerTopicRun)> = Vec::new();
    for (miner, per_topic) in runs {
        if let Some(run) = per_topic.get(&topic.id) {
            if run.pass {
                passers.push((miner.clone(), run.clone()));
            }
        }
    }
    if passers.is_empty() || mass == 0 {
        return BTreeMap::new();
    }
    mark_digest_duplicates(&mut passers);
    match topic.payout_mode {
        PayoutMode::Wta => wta(topic.metric.direction, mass, &passers),
        PayoutMode::Discovery => discovery(topic, mass, sealed, champion, &passers),
    }
}

fn mark_digest_duplicates(passers: &mut [(String, MinerTopicRun)]) {
    passers.sort_by(|a, b| a.0.cmp(&b.0));
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (miner, run) in passers.iter_mut() {
        let digest = run.artifact_digest.trim().to_ascii_lowercase();
        if digest.is_empty() {
            continue;
        }
        if let Some(first) = seen.get(&digest) {
            if first != miner {
                run.near_duplicate = true;
            }
        } else {
            seen.insert(digest, miner.clone());
        }
    }
}

fn wta(
    direction: MetricDirection,
    mass: u64,
    passers: &[(String, MinerTopicRun)],
) -> BTreeMap<String, u64> {
    let ranked: Vec<&(String, MinerTopicRun)> = passers
        .iter()
        .filter(|(_, r)| r.primary.is_some_and(f64::is_finite))
        .collect();
    let Some(best) = ranked
        .iter()
        .filter_map(|(_, r)| r.primary)
        .reduce(|a, b| match direction {
            MetricDirection::Max => a.max(b),
            MetricDirection::Min => a.min(b),
        })
    else {
        return BTreeMap::new();
    };
    let winners: Vec<(String, u128)> = ranked
        .into_iter()
        .filter(|(_, r)| r.primary.is_some_and(|p| exact_eq(p, best)))
        .map(|(m, _)| (m.clone(), 1))
        .collect();
    allocate(mass, &winners)
}

fn discovery(
    topic: &TopicDocument,
    mass: u64,
    sealed: Option<&SealedBaseline>,
    champion: Option<f64>,
    passers: &[(String, MinerTopicRun)],
) -> BTreeMap<String, u64> {
    let floor_bps = u32::from(topic.discovery.pass_floor_share_bps);
    let novelty_bps = u32::from(topic.discovery.novelty_pool_share_bps);
    let floor_mass = share_of(mass, floor_bps);
    let novelty_mass = share_of(mass, novelty_bps);
    let floor_weights: Vec<(String, u128)> = passers.iter().map(|(m, _)| (m.clone(), 1)).collect();
    let mut out = allocate(floor_mass, &floor_weights);

    let bar = novelty_bar(topic, sealed, champion);
    let novelty_weights: Vec<(String, u128)> = passers
        .iter()
        .map(|(miner, run)| {
            let w = if run.near_duplicate {
                0
            } else {
                novelty_weight(run.primary, bar, topic.metric.direction)
            };
            (miner.clone(), w)
        })
        .collect();
    let novelty = allocate(novelty_mass, &novelty_weights);
    for (miner, piece) in novelty {
        out.entry(miner)
            .and_modify(|v| *v = v.saturating_add(piece))
            .or_insert(piece);
    }
    out
}

fn novelty_bar(
    topic: &TopicDocument,
    sealed: Option<&SealedBaseline>,
    champion: Option<f64>,
) -> Option<f64> {
    let sealed_p = sealed.and_then(|s| sealed_primary(topic, s));
    match (
        sealed_p,
        champion.filter(|c| c.is_finite()),
        topic.metric.direction,
    ) {
        (None, None, _) => None,
        (Some(s), None, _) | (None, Some(s), _) => Some(s),
        (Some(s), Some(c), MetricDirection::Max) => Some(s.max(c)),
        (Some(s), Some(c), MetricDirection::Min) => Some(s.min(c)),
    }
}

fn novelty_weight(primary: Option<f64>, bar: Option<f64>, direction: MetricDirection) -> u128 {
    let (Some(p), Some(b)) = (primary, bar) else {
        return 0;
    };
    if !p.is_finite() || !b.is_finite() {
        return 0;
    }
    let delta = match direction {
        MetricDirection::Max => p - b,
        MetricDirection::Min => b - p,
    };
    if delta <= 0.0 {
        return 0;
    }
    // Micro-units so small relative wins still rank. Capped so the u128 stays tidy.
    let scaled = (delta * 1_000_000_000.0).round().max(0.0);
    u128::from(scaled as u64)
}

fn share_of(mass: u64, bps: u32) -> u64 {
    let raw = u128::from(mass) * u128::from(bps) / u128::from(BPS_DENOM);
    u64::try_from(raw).unwrap_or(u64::MAX)
}

fn exact_eq(a: f64, b: f64) -> bool {
    a == b
}

fn allocate(mass: u64, weights: &[(String, u128)]) -> BTreeMap<String, u64> {
    let total: u128 = weights.iter().map(|(_, w)| *w).sum();
    if mass == 0 || total == 0 {
        return BTreeMap::new();
    }
    let mut rows: Vec<(String, u64, u128)> = Vec::new();
    let mut assigned = 0u64;
    for (k, w) in weights {
        if *w == 0 {
            continue;
        }
        let raw = u128::from(mass) * *w;
        let floor = u64::try_from(raw / total).unwrap_or(u64::MAX);
        let rem = raw % total;
        assigned = assigned.saturating_add(floor);
        rows.push((k.clone(), floor, rem));
    }
    let mut leftover = mass.saturating_sub(assigned);
    rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let mut out = BTreeMap::new();
    for (k, v, _) in rows {
        let extra = if leftover > 0 {
            leftover = leftover.saturating_sub(1);
            1
        } else {
            0
        };
        out.insert(k, v.saturating_add(extra));
    }
    out
}

#[cfg(test)]
mod tests {
    use proof_task::{
        default_adamw, holdout_commitment, synthetic_holdout, MetricDirection, MetricFamily,
        MetricSpec, PayoutMode, TopicDocument, TopicStatus, FLOPS_BUDGET_MAX,
        METRIC_TOKENS_PER_SEC,
    };

    use super::*;
    use crate::flat_nll;

    fn sealed_adamw() -> proof_task::Baseline {
        let mut b = default_adamw(FLOPS_BUDGET_MAX);
        b.script_sha256 = "11".repeat(32);
        b.metrics_commitment = "22".repeat(32);
        b
    }

    fn nll_topic() -> TopicDocument {
        TopicDocument {
            id: "adamw-beater-v0".into(),
            statement: "Beat sealed AdamW holdout NLL.".into(),
            payout_mode: PayoutMode::Discovery,
            baseline: sealed_adamw(),
            holdout_commitment: holdout_commitment(&synthetic_holdout(24, 1)),
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    fn wta_topic() -> TopicDocument {
        let mut t = nll_topic();
        t.id = "dt-no-ib-v0".into();
        t.payout_mode = PayoutMode::Wta;
        t.metric = MetricSpec {
            family: MetricFamily::Throughput,
            primary: METRIC_TOKENS_PER_SEC.into(),
            direction: MetricDirection::Max,
            unit: "tokens_per_second".into(),
            epsilon_rel: 0.05,
            quality_floor_nll: 0.02,
            wall_budget_s: 14_400,
            custom_id: String::new(),
        };
        t
    }

    fn run(pass: bool, primary: f64, digest: &str) -> MinerTopicRun {
        MinerTopicRun {
            pass,
            primary: Some(primary),
            artifact_digest: digest.into(),
            near_duplicate: false,
        }
    }

    #[test]
    fn wta_winner_takes_the_topic_and_ties_split() {
        let topic = wta_topic();
        let mut runs = BTreeMap::new();
        runs.insert("aa".into(), {
            let mut m = BTreeMap::new();
            m.insert(topic.id.clone(), run(true, 120.0, "d1"));
            m
        });
        runs.insert("bb".into(), {
            let mut m = BTreeMap::new();
            m.insert(topic.id.clone(), run(true, 110.0, "d2"));
            m
        });
        runs.insert("cc".into(), {
            let mut m = BTreeMap::new();
            m.insert(topic.id.clone(), run(false, 200.0, "d3"));
            m
        });
        let paid = payout_lattices(
            std::slice::from_ref(&topic),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &runs,
        );
        assert_eq!(paid.get("aa").copied(), Some(SCORE_MAX));
        assert!(!paid.contains_key("bb"));
        assert!(!paid.contains_key("cc"));

        runs.insert("bb".into(), {
            let mut m = BTreeMap::new();
            m.insert(topic.id.clone(), run(true, 120.0, "d2"));
            m
        });
        let tied = payout_lattices(&[topic], &BTreeMap::new(), &BTreeMap::new(), &runs);
        assert_eq!(tied.get("aa").copied(), Some(SCORE_MAX / 2));
        assert_eq!(tied.get("bb").copied(), Some(SCORE_MAX / 2));
    }

    #[test]
    fn discovery_splits_floor_then_novelty_and_duplicates_get_floor_only() {
        let topic = nll_topic();
        assert_eq!(topic.discovery.pass_floor_share_bps, 3_000);
        assert_eq!(topic.discovery.novelty_pool_share_bps, 7_000);
        let mut sealed = BTreeMap::new();
        sealed.insert(topic.id.clone(), flat_nll(3.0));
        let mut runs = BTreeMap::new();
        // Lower NLL is better. aa improves more than bb. cc is a near-dup of aa.
        runs.insert("aa".into(), {
            let mut m = BTreeMap::new();
            m.insert(topic.id.clone(), run(true, 2.5, "same"));
            m
        });
        runs.insert("bb".into(), {
            let mut m = BTreeMap::new();
            m.insert(topic.id.clone(), run(true, 2.8, "other"));
            m
        });
        runs.insert("cc".into(), {
            let mut m = BTreeMap::new();
            m.insert(topic.id.clone(), run(true, 2.4, "same"));
            m
        });
        let paid = payout_lattices(&[topic], &sealed, &BTreeMap::new(), &runs);
        let floor = share_of(SCORE_MAX, 3_000) / 3;
        let aa = paid.get("aa").copied().unwrap_or(0);
        let bb = paid.get("bb").copied().unwrap_or(0);
        let cc = paid.get("cc").copied().unwrap_or(0);
        assert!(aa >= floor, "aa={aa} floor={floor}");
        assert!(bb >= floor, "bb={bb}");
        assert!(cc >= floor, "cc={cc}");
        // aa has the only novelty weight besides maybe cc which is dup of same digest.
        // cc is a duplicate (same digest as aa, later in sort? aa < cc so aa original).
        assert!(
            aa > bb,
            "novelty should prefer the larger NLL win: aa={aa} bb={bb}"
        );
        assert!(aa > cc, "duplicate must not take novelty: aa={aa} cc={cc}");
        assert_eq!(aa.saturating_add(bb).saturating_add(cc), SCORE_MAX);
    }

    #[test]
    fn empty_open_set_pays_nobody() {
        let paid = payout_lattices(&[], &BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new());
        assert!(paid.is_empty());
        assert_eq!(topic_share_bps(0), 0);
        assert!(topic_masses(0).is_empty());
        assert_eq!(topic_share_bps(2), 4_000);
        assert_eq!(topic_masses(2).iter().sum::<u64>(), SCORE_MAX);
    }

    #[test]
    fn two_open_topics_sum_not_mean() {
        let a = nll_topic();
        let mut b = wta_topic();
        b.id = "other-v0".into();
        let mut runs = BTreeMap::new();
        runs.insert("aa".into(), {
            let mut m = BTreeMap::new();
            m.insert(a.id.clone(), run(true, 2.5, "d1"));
            m.insert(b.id.clone(), run(true, 200.0, "d2"));
            m
        });
        let mut sealed = BTreeMap::new();
        sealed.insert(a.id.clone(), flat_nll(3.0));
        let paid = payout_lattices(&[a, b], &sealed, &BTreeMap::new(), &runs);
        // Sole passer/winner on both topics collects the full lattice.
        assert_eq!(paid.get("aa").copied(), Some(SCORE_MAX));
    }
}
