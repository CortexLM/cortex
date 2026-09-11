//! Promotion rule: the best artefact is promoted when a **passing** run's
//! primary beats the current bar by the topic's relative epsilon **and** the
//! anti-cheat checklist was green. The bar is the sealed baseline or the
//! reigning best, whichever is better (`proof_score::novelty_bar`). Direction
//! comes from the topic, so a `min` metric promotes on a lower primary.

use proof_task::MetricDirection;

use crate::relative_win;
use serde::{Deserialize, Serialize};

/// Why a run was not promoted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeepReason {
    /// The harness gates did not pass.
    NotPassed,
    /// The checklist was red or incomplete (ids listed).
    ChecklistRed(Vec<String>),
    /// The report did not yield a primary.
    NoPrimary,
    /// Nothing sealed and no best: no bar to beat (fail-closed).
    NoBar,
    /// Passed, but not by `epsilon_rel` over the bar.
    BelowBar {
        /// Measured primary.
        primary: f64,
        /// Bar it had to clear.
        bar: f64,
        /// Required relative win.
        epsilon_rel: f64,
    },
}

/// Promote or keep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromoteDecision {
    /// Crown this run; its artefact becomes the topic's best.
    Promote {
        /// Measured primary.
        primary: f64,
        /// Bar it cleared.
        bar: f64,
    },
    /// Keep the current best.
    Keep(KeepReason),
}

impl PromoteDecision {
    /// Whether this is a promotion.
    #[must_use]
    pub fn is_promote(&self) -> bool {
        matches!(self, Self::Promote { .. })
    }
}

/// Decide promotion for one scored run.
///
/// `pass` is the harness verdict, `checklist_red` the red rule ids (empty and
/// `checklist_complete` for green), `primary` the report's primary, `bar`
/// the current novelty bar, `direction` / `epsilon_rel` the topic's.
#[must_use]
pub fn decide_promote(
    pass: bool,
    checklist_complete: bool,
    checklist_red: &[String],
    primary: Option<f64>,
    bar: Option<f64>,
    direction: MetricDirection,
    epsilon_rel: f64,
) -> PromoteDecision {
    if !pass {
        return PromoteDecision::Keep(KeepReason::NotPassed);
    }
    if !checklist_complete || !checklist_red.is_empty() {
        return PromoteDecision::Keep(KeepReason::ChecklistRed(checklist_red.to_vec()));
    }
    let Some(primary) = primary.filter(|p| p.is_finite()) else {
        return PromoteDecision::Keep(KeepReason::NoPrimary);
    };
    let Some(bar) = bar.filter(|b| b.is_finite()) else {
        return PromoteDecision::Keep(KeepReason::NoBar);
    };
    if relative_win(primary, bar, direction, epsilon_rel) {
        PromoteDecision::Promote { primary, bar }
    } else {
        PromoteDecision::Keep(KeepReason::BelowBar {
            primary,
            bar,
            epsilon_rel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: MetricDirection = MetricDirection::Max;

    #[test]
    fn promote_needs_pass_green_and_a_relative_win_over_the_bar() {
        assert_eq!(
            decide_promote(true, true, &[], Some(0.60), Some(0.50), MAX, 0.02),
            PromoteDecision::Promote {
                primary: 0.60,
                bar: 0.50
            }
        );
        assert!(decide_promote(true, true, &[], Some(0.55), Some(0.50), MAX, 0.02).is_promote());
        assert_eq!(
            decide_promote(true, true, &[], Some(0.50), Some(0.50), MAX, 0.02),
            PromoteDecision::Keep(KeepReason::BelowBar {
                primary: 0.50,
                bar: 0.50,
                epsilon_rel: 0.02
            })
        );
        assert_eq!(
            decide_promote(false, true, &[], Some(0.9), Some(0.5), MAX, 0.02),
            PromoteDecision::Keep(KeepReason::NotPassed)
        );
        assert_eq!(
            decide_promote(true, true, &[], None, Some(0.5), MAX, 0.02),
            PromoteDecision::Keep(KeepReason::NoPrimary)
        );
        assert_eq!(
            decide_promote(true, true, &[], Some(0.9), None, MAX, 0.02),
            PromoteDecision::Keep(KeepReason::NoBar)
        );
    }

    /// Checklist green is a hard condition even when the numbers win.
    #[test]
    fn a_red_or_incomplete_checklist_never_promotes() {
        let red = vec!["rule_b".to_owned()];
        assert_eq!(
            decide_promote(true, true, &red, Some(1.0), Some(0.1), MAX, 0.02),
            PromoteDecision::Keep(KeepReason::ChecklistRed(red.clone()))
        );
        assert_eq!(
            decide_promote(true, false, &[], Some(1.0), Some(0.1), MAX, 0.02),
            PromoteDecision::Keep(KeepReason::ChecklistRed(Vec::new()))
        );
    }

    /// Direction comes from the topic; a zero bar is unbeatable relatively.
    #[test]
    fn direction_is_topic_data_and_a_zero_bar_is_not_beatable() {
        assert!(decide_promote(
            true,
            true,
            &[],
            Some(2.0),
            Some(3.0),
            MetricDirection::Min,
            0.05
        )
        .is_promote());
        assert!(!decide_promote(
            true,
            true,
            &[],
            Some(3.0),
            Some(2.0),
            MetricDirection::Min,
            0.05
        )
        .is_promote());
        assert!(matches!(
            decide_promote(true, true, &[], Some(0.5), Some(0.0), MAX, 0.02),
            PromoteDecision::Keep(KeepReason::BelowBar { .. })
        ));
        let json = serde_json::to_string(&decide_promote(
            true,
            true,
            &[],
            Some(0.6),
            Some(0.5),
            MAX,
            0.02,
        ))
        .expect("json");
        assert!(json.contains("\"promote\""), "{json}");
    }
}
