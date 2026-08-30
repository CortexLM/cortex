//! Q-Judger is the judge for Relearn T2I. This crate is its wire format.
//!
//! Q-Judger (`Qwen/Qwen-Image-Bench`, Apache-2.0, fine-tuned from Qwen3.6-27B)
//! is handed a bench prompt plus one generated image and replies with a
//! chain-of-thought preamble followed by a JSON score tree:
//!
//! ```text
//! { "<L1 pillar>": { "<L2 group>": { "<L3 item>": { "score": 0|1|2|"N/A" } } } }
//! ```
//!
//! The paper's mapping and aggregation are reproduced exactly: raw `0|1|2` map
//! to `0|60|100`, `N/A` is excluded rather than zeroed, level 3 averages into
//! level 2, level 2 into level 1, and the five level-1 pillars average into the
//! total. Zeroing `N/A` would quietly punish prompts where a pillar does not
//! apply, which is why exclusion is a correctness requirement here.
//!
//! No other judge model is accepted: see [`assert_judge_model`].

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

use std::collections::BTreeMap;

use relearn_t2i_task::{base_matches_pin, L1Dimension, JUDGE_MODEL_ID};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Mapped value for a raw `0` (Fail).
pub const MAPPED_FAIL: f64 = 0.0;

/// Mapped value for a raw `1` (Pass).
pub const MAPPED_PASS: f64 = 60.0;

/// Mapped value for a raw `2` (Excel).
pub const MAPPED_EXCEL: f64 = 100.0;

/// Highest mapped value; used to normalize the paper scale into `0..=1`.
pub const MAPPED_MAX: f64 = MAPPED_EXCEL;

/// Fixed Q-Judger inference parameters, straight from the model card.
///
/// These are part of the contract, not tuning knobs: a judge run at a
/// different temperature is not comparable with the champion's recorded run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeInference {
    /// Judge model id. Always Q-Judger.
    pub model: String,
    /// Sampling seed.
    pub seed: u32,
    /// Sampling temperature (greedy).
    pub temperature: f64,
    /// Top-k (greedy).
    pub top_k: u32,
    /// Top-p.
    pub top_p: f64,
    /// Repetition penalty.
    pub repetition_penalty: f64,
    /// Generation budget for the thinking trace plus JSON.
    pub max_new_tokens: u32,
    /// Chain-of-thought before the JSON.
    pub enable_thinking: bool,
    /// Harness batch size.
    pub max_batch_size: u32,
}

impl Default for JudgeInference {
    fn default() -> Self {
        Self {
            model: JUDGE_MODEL_ID.into(),
            seed: 42,
            temperature: 0.0,
            top_k: 1,
            top_p: 1.0,
            repetition_penalty: 1.05,
            max_new_tokens: 4096,
            enable_thinking: true,
            max_batch_size: 24,
        }
    }
}

/// One judge input row (the harness `ID` / `prompt` / `image_path` columns).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgeRequest {
    /// Bench prompt id (1..=1000).
    #[serde(rename = "ID")]
    pub id: u32,
    /// Frozen prompt string sent to the generator.
    pub prompt: String,
    /// Path to the generated image.
    pub image_path: String,
}

/// A raw Q-Judger level-3 verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawScore {
    /// `0` — the criterion is not met.
    Fail,
    /// `1` — the criterion is met.
    Pass,
    /// `2` — the criterion is met unusually well.
    Excel,
    /// `N/A` — the criterion does not apply to this prompt. Excluded, not zero.
    NotApplicable,
}

impl RawScore {
    /// Paper-scale value, or `None` for `N/A`.
    #[must_use]
    pub const fn mapped(self) -> Option<f64> {
        match self {
            Self::Fail => Some(MAPPED_FAIL),
            Self::Pass => Some(MAPPED_PASS),
            Self::Excel => Some(MAPPED_EXCEL),
            Self::NotApplicable => None,
        }
    }

    fn from_json(v: &serde_json::Value) -> Option<Self> {
        if let Some(n) = v.as_u64() {
            return match n {
                0 => Some(Self::Fail),
                1 => Some(Self::Pass),
                2 => Some(Self::Excel),
                _ => None,
            };
        }
        let s = v.as_str()?.trim();
        if s.eq_ignore_ascii_case("n/a") || s.eq_ignore_ascii_case("na") {
            return Some(Self::NotApplicable);
        }
        match s {
            "0" => Some(Self::Fail),
            "1" => Some(Self::Pass),
            "2" => Some(Self::Excel),
            _ => None,
        }
    }
}

/// Why a judge reply was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JudgeError {
    /// No JSON object could be located in the reply.
    #[error("no JSON object in judge reply")]
    NoJson,
    /// The JSON did not parse.
    #[error("judge JSON parse: {0}")]
    Json(String),
    /// The tree carried no recognizable L1 pillar.
    #[error("judge reply has no recognized L1 pillar")]
    NoPillars,
    /// Every level-3 item was `N/A`, so there is nothing to score.
    #[error("judge reply is entirely N/A")]
    AllNotApplicable,
    /// A level-3 value was neither `0|1|2` nor `N/A`.
    #[error("unparsable score {value:?} at {path}")]
    BadScore {
        /// Dotted path of the offending item.
        path: String,
        /// Raw value as text.
        value: String,
    },
    /// Someone tried to score with a model other than Q-Judger.
    #[error("judge must be {expected:?}, got {got:?}")]
    WrongJudge {
        /// Required judge id.
        expected: String,
        /// What the caller supplied.
        got: String,
    },
}

/// Refuse anything but Q-Judger as the T2I judge.
///
/// # Errors
///
/// [`JudgeError::WrongJudge`] when `model` is not `Qwen/Qwen-Image-Bench`.
pub fn assert_judge_model(model: &str) -> Result<(), JudgeError> {
    if base_matches_pin(model, JUDGE_MODEL_ID) {
        Ok(())
    } else {
        Err(JudgeError::WrongJudge {
            expected: JUDGE_MODEL_ID.into(),
            got: model.to_owned(),
        })
    }
}

/// Parsed score tree for one image: pillar → level-2 group → level-3 item.
pub type ScoreTree = BTreeMap<L1Dimension, BTreeMap<String, BTreeMap<String, RawScore>>>;

/// Aggregated scores for one image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageScore {
    /// Per-pillar averages on the paper scale (`0..=100`).
    pub per_l1: BTreeMap<L1Dimension, f64>,
    /// Mean of the present pillars — the paper's total.
    pub total: f64,
    /// Level-3 items that carried a usable score.
    pub scored_items: u32,
    /// Level-3 items reported `N/A`.
    pub na_items: u32,
}

impl ImageScore {
    /// Fraction of level-3 items the judge declined to score.
    #[must_use]
    pub fn na_rate(&self) -> f64 {
        let total = f64::from(self.scored_items) + f64::from(self.na_items);
        if total <= 0.0 {
            return 1.0;
        }
        f64::from(self.na_items) / total
    }

    /// Total normalized into `0..=1` for the paired displacement test.
    ///
    /// The paired test's dead zone is expressed in absolute metric units, so
    /// normalizing here makes one dead-zone unit equal one paper point.
    #[must_use]
    pub fn normalized_total(&self) -> f64 {
        self.total / MAPPED_MAX
    }

    /// One pillar normalized into `0..=1`.
    #[must_use]
    pub fn normalized_pillar(&self, dim: L1Dimension) -> Option<f64> {
        self.per_l1.get(&dim).map(|v| v / MAPPED_MAX)
    }
}

/// Locate the score JSON inside a reply that begins with a thinking trace.
///
/// Prefers a fenced ```json block, then falls back to the last balanced
/// top-level object — the thinking trace often contains braces of its own, and
/// the score tree is what the model emits last.
#[must_use]
pub fn extract_json_object(raw: &str) -> Option<&str> {
    if let Some(found) = fenced_json(raw) {
        return Some(found);
    }
    balanced_objects(raw).last().copied()
}

fn fenced_json(raw: &str) -> Option<&str> {
    for fence in ["```json", "```JSON"] {
        if let Some(start) = raw.find(fence) {
            let after = &raw[start + fence.len()..];
            let end = after.find("```")?;
            let body = after[..end].trim();
            if body.starts_with('{') {
                return Some(body);
            }
        }
    }
    None
}

/// Every balanced top-level `{…}` slice, string- and escape-aware.
fn balanced_objects(raw: &str) -> Vec<&str> {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, b) in bytes.iter().enumerate() {
        if in_str {
            if escaped {
                escaped = false;
            } else if *b == b'\\' {
                escaped = true;
            } else if *b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => {
                if depth == 0 {
                    start = i;
                }
                depth = depth.saturating_add(1);
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && i >= start {
                    if let Some(slice) = raw.get(start..=i) {
                        out.push(slice);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Parse a Q-Judger reply into a score tree.
///
/// Accepts either a full tree keyed by the five pillars, or a single-pillar
/// tree when `assume_pillar` names which pillar was requested.
///
/// # Errors
///
/// See [`JudgeError`]. Unknown score values are rejected rather than coerced,
/// so a judge that starts emitting a new vocabulary fails loudly.
pub fn parse_reply(raw: &str, assume_pillar: Option<L1Dimension>) -> Result<ScoreTree, JudgeError> {
    let body = extract_json_object(raw).ok_or(JudgeError::NoJson)?;
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| JudgeError::Json(e.to_string()))?;
    let root = value.as_object().ok_or(JudgeError::NoJson)?;

    let mut tree = ScoreTree::new();
    let looks_like_pillars = root.keys().any(|k| L1Dimension::parse(k).is_some());
    if looks_like_pillars {
        for (key, sub) in root {
            let Some(dim) = L1Dimension::parse(key) else {
                continue;
            };
            let groups = parse_groups(dim.as_str(), sub)?;
            if !groups.is_empty() {
                tree.insert(dim, groups);
            }
        }
    } else if let Some(dim) = assume_pillar {
        let groups = parse_groups(dim.as_str(), &value)?;
        if !groups.is_empty() {
            tree.insert(dim, groups);
        }
    }

    if tree.is_empty() {
        return Err(JudgeError::NoPillars);
    }
    Ok(tree)
}

fn parse_groups(
    pillar: &str,
    value: &serde_json::Value,
) -> Result<BTreeMap<String, BTreeMap<String, RawScore>>, JudgeError> {
    let obj = value.as_object().ok_or(JudgeError::NoJson)?;
    let mut groups = BTreeMap::new();
    for (l2, l2_val) in obj {
        let Some(items) = l2_val.as_object() else {
            continue;
        };
        let mut leaves = BTreeMap::new();
        for (l3, l3_val) in items {
            let raw = l3_val
                .get("score")
                .or_else(|| l3_val.get("Score"))
                .unwrap_or(l3_val);
            let parsed = RawScore::from_json(raw).ok_or_else(|| JudgeError::BadScore {
                path: format!("{pillar}.{l2}.{l3}"),
                value: raw.to_string(),
            })?;
            leaves.insert(l3.clone(), parsed);
        }
        if !leaves.is_empty() {
            groups.insert(l2.clone(), leaves);
        }
    }
    Ok(groups)
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let n = values.len() as f64;
    Some(values.iter().sum::<f64>() / n)
}

/// Aggregate a score tree the way the paper does.
///
/// Level 3 → level 2 averages only non-`N/A` items; a level-2 group whose items
/// are all `N/A` drops out entirely; level 2 → level 1 averages the surviving
/// groups; the total averages the surviving pillars.
///
/// # Errors
///
/// [`JudgeError::AllNotApplicable`] when nothing survived — the caller must
/// treat that as a failed judge run, never as a score of zero.
pub fn aggregate(tree: &ScoreTree) -> Result<ImageScore, JudgeError> {
    let mut per_l1 = BTreeMap::new();
    let mut scored_items = 0u32;
    let mut na_items = 0u32;

    for (dim, groups) in tree {
        let mut group_means = Vec::new();
        for leaves in groups.values() {
            let mut usable = Vec::new();
            for raw in leaves.values() {
                match raw.mapped() {
                    Some(v) => {
                        usable.push(v);
                        scored_items = scored_items.saturating_add(1);
                    }
                    None => na_items = na_items.saturating_add(1),
                }
            }
            if let Some(m) = mean(&usable) {
                group_means.push(m);
            }
        }
        if let Some(m) = mean(&group_means) {
            per_l1.insert(*dim, m);
        }
    }

    let pillar_means: Vec<f64> = per_l1.values().copied().collect();
    let total = mean(&pillar_means).ok_or(JudgeError::AllNotApplicable)?;
    Ok(ImageScore {
        per_l1,
        total,
        scored_items,
        na_items,
    })
}

/// Parse and aggregate in one step.
///
/// # Errors
///
/// See [`parse_reply`] and [`aggregate`].
pub fn score_reply(
    raw: &str,
    assume_pillar: Option<L1Dimension>,
) -> Result<ImageScore, JudgeError> {
    aggregate(&parse_reply(raw, assume_pillar)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_REPLY: &str = r#"
Let me look at the image. The cube is red {not JSON} and sharp.
```json
{
  "Quality": {
    "Realism": {"Physical Logic": {"score": 1}, "Material Texture": {"score": 2}},
    "Detail": {"Noise": {"score": 1}, "Edge Clarity": {"score": 1}, "Naturalness": {"score": 1}},
    "Resolution": {"Resolution": {"score": 2}}
  },
  "Aesthetics": {
    "Composition": {"Composition": {"score": 2}},
    "Color Harmony": {"Color Harmony": {"score": 1}}
  },
  "Alignment": {
    "Attributes": {"Color": {"score": 2}, "Quantity": {"score": "N/A"}}
  },
  "Real-world Fidelity": {
    "Safety & Compliance": {"Safety & Compliance": {"score": 1}}
  },
  "Creative Generation": {
    "Text Rendering": {"Text Accuracy": {"score": "N/A"}, "Font": {"score": "N/A"}},
    "Imagination": {"Imagination": {"score": 2}}
  }
}
```
"#;

    #[test]
    fn only_q_judger_may_judge() {
        assert_judge_model("Qwen/Qwen-Image-Bench").expect("pinned judge");
        assert_judge_model("qwen/qwen-image-bench").expect("case-insensitive");
        let err = assert_judge_model("gpt-4o").expect_err("must refuse");
        assert!(matches!(err, JudgeError::WrongJudge { .. }));
    }

    #[test]
    fn inference_params_match_the_card() {
        let p = JudgeInference::default();
        assert_eq!(p.model, "Qwen/Qwen-Image-Bench");
        assert_eq!(p.seed, 42);
        assert!(p.temperature.abs() < f64::EPSILON);
        assert_eq!(p.top_k, 1);
        assert!((p.top_p - 1.0).abs() < f64::EPSILON);
        assert!((p.repetition_penalty - 1.05).abs() < 1e-12);
        assert_eq!(p.max_new_tokens, 4096);
        assert!(p.enable_thinking);
        assert_eq!(p.max_batch_size, 24);
    }

    #[test]
    fn raw_scores_map_to_paper_scale() {
        assert_eq!(RawScore::Fail.mapped(), Some(0.0));
        assert_eq!(RawScore::Pass.mapped(), Some(60.0));
        assert_eq!(RawScore::Excel.mapped(), Some(100.0));
        assert_eq!(RawScore::NotApplicable.mapped(), None);
    }

    #[test]
    fn parses_thinking_preamble_then_json() {
        let tree = parse_reply(FULL_REPLY, None).expect("parse");
        assert_eq!(tree.len(), 5);
        let quality = &tree[&L1Dimension::Quality];
        assert_eq!(quality["Resolution"]["Resolution"], RawScore::Excel);
        assert_eq!(
            tree[&L1Dimension::Alignment]["Attributes"]["Quantity"],
            RawScore::NotApplicable
        );
    }

    #[test]
    fn aggregation_follows_the_paper() {
        let score = score_reply(FULL_REPLY, None).expect("score");
        // Quality: Realism (60+100)/2 = 80, Detail 60, Resolution 100 → 80.
        assert!((score.per_l1[&L1Dimension::Quality] - 80.0).abs() < 1e-9);
        // Aesthetics: Composition 100, Color Harmony 60 → 80.
        assert!((score.per_l1[&L1Dimension::Aesthetics] - 80.0).abs() < 1e-9);
        // Alignment: Attributes averages only Color (100); Quantity is N/A.
        assert!((score.per_l1[&L1Dimension::Alignment] - 100.0).abs() < 1e-9);
        // Creative Generation: Text Rendering is entirely N/A and drops out,
        // leaving Imagination = 100.
        assert!((score.per_l1[&L1Dimension::CreativeGeneration] - 100.0).abs() < 1e-9);
        let expected = (80.0 + 80.0 + 100.0 + 60.0 + 100.0) / 5.0;
        assert!((score.total - expected).abs() < 1e-9, "{}", score.total);
    }

    #[test]
    fn na_is_excluded_not_zeroed() {
        let with_na =
            r#"{"Quality": {"Detail": {"Noise": {"score": 2}, "Edge Clarity": {"score": "N/A"}}}}"#;
        let zeroed =
            r#"{"Quality": {"Detail": {"Noise": {"score": 2}, "Edge Clarity": {"score": 0}}}}"#;
        let a = score_reply(with_na, None).expect("na");
        let b = score_reply(zeroed, None).expect("zero");
        assert!((a.total - 100.0).abs() < 1e-9);
        assert!((b.total - 50.0).abs() < 1e-9);
        assert!(a.total > b.total, "N/A must not behave like a zero");
        assert_eq!(a.na_items, 1);
        assert_eq!(a.scored_items, 1);
    }

    #[test]
    fn na_rate_and_normalization() {
        let score = score_reply(FULL_REPLY, None).expect("score");
        assert_eq!(score.na_items, 3);
        assert!(score.na_rate() > 0.0 && score.na_rate() < 1.0);
        assert!((score.normalized_total() - score.total / 100.0).abs() < 1e-12);
        assert!(score.normalized_pillar(L1Dimension::Quality).is_some());
    }

    #[test]
    fn single_pillar_reply_needs_the_assumed_pillar() {
        let body = r#"{"Realism": {"Physical Logic": {"score": 1}}}"#;
        assert!(matches!(
            parse_reply(body, None),
            Err(JudgeError::NoPillars)
        ));
        let tree = parse_reply(body, Some(L1Dimension::Quality)).expect("assumed pillar");
        assert_eq!(tree.len(), 1);
        assert!(tree.contains_key(&L1Dimension::Quality));
    }

    #[test]
    fn unknown_score_vocabulary_fails_loudly() {
        let body = r#"{"Quality": {"Detail": {"Noise": {"score": "excellent"}}}}"#;
        let err = parse_reply(body, None).expect_err("must refuse");
        assert!(matches!(err, JudgeError::BadScore { .. }), "{err:?}");
    }

    #[test]
    fn all_na_is_a_failed_run_not_a_zero() {
        let body = r#"{"Quality": {"Detail": {"Noise": {"score": "N/A"}}}}"#;
        let err = score_reply(body, None).expect_err("must refuse");
        assert_eq!(err, JudgeError::AllNotApplicable);
    }

    #[test]
    fn missing_json_is_refused() {
        assert_eq!(
            parse_reply("I could not evaluate this image.", None).expect_err("no json"),
            JudgeError::NoJson
        );
    }

    #[test]
    fn unfenced_reply_uses_the_last_balanced_object() {
        let raw = concat!(
            "thinking: the layout {looks} fine, and I considered {\"score\": 0} briefly.\n",
            "{\"Quality\": {\"Detail\": {\"Noise\": {\"score\": 2}}}}"
        );
        let score = score_reply(raw, None).expect("score");
        assert!((score.total - 100.0).abs() < 1e-9);
    }

    #[test]
    fn bare_leaf_scores_are_accepted() {
        let body = r#"{"Quality": {"Detail": {"Noise": 1, "Naturalness": 2}}}"#;
        let score = score_reply(body, None).expect("score");
        assert!((score.total - 80.0).abs() < 1e-9);
    }
}
