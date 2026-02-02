//! Token counting and rate limiting types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Token usage statistics.
#[derive(Debug, Clone, Deserialize, Serialize, Default, JsonSchema)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

/// Detailed token usage information with context.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TokenUsageInfo {
    pub total_token_usage: TokenUsage,
    pub last_token_usage: TokenUsage,
    pub model_context_window: Option<i64>,
    #[serde(default)]
    pub context_tokens: i64,
}

/// Token count event payload.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TokenCountEvent {
    pub info: Option<TokenUsageInfo>,
    pub rate_limits: Option<RateLimitSnapshot>,
}

/// Snapshot of rate limit state.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct RateLimitSnapshot {
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
    pub credits: Option<CreditsSnapshot>,
}

/// A single rate limit window.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct RateLimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
}

/// Credits balance snapshot.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct CreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}
