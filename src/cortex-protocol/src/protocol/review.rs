//! Code review related types.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Review request from user.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct ReviewRequest {
    pub prompt: String,
    pub user_facing_hint: String,
    #[serde(default)]
    pub append_to_original_thread: bool,
}

/// Event when review mode is exited.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExitedReviewModeEvent {
    pub review_output: Option<ReviewOutputEvent>,
}

/// Output from a code review.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema, Default)]
pub struct ReviewOutputEvent {
    pub findings: Vec<ReviewFinding>,
    pub overall_correctness: String,
    pub overall_explanation: String,
    pub overall_confidence_score: f32,
}

/// A single review finding.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct ReviewFinding {
    pub title: String,
    pub body: String,
    pub confidence_score: f32,
    pub priority: i32,
    pub code_location: ReviewCodeLocation,
}

/// Location of code referenced in a finding.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct ReviewCodeLocation {
    pub absolute_file_path: PathBuf,
    pub line_range: ReviewLineRange,
}

/// Line range for review findings.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct ReviewLineRange {
    pub start: u32,
    pub end: u32,
}
