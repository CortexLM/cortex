//! Sampling types for MCP protocol.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::content::Content;
use crate::prompts::Role;

/// Sampling request for LLM completion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SamplingRequest {
    /// Messages for the LLM.
    pub messages: Vec<SamplingMessage>,
    /// Model preferences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_preferences: Option<ModelPreferences>,
    /// System prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Include context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_context: Option<IncludeContext>,
    /// Temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Maximum tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Stop sequences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    /// Additional metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, Value>>,
}

/// Sampling message.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SamplingMessage {
    /// Message role.
    pub role: Role,
    /// Message content.
    pub content: Content,
}

/// Model preferences.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferences {
    /// Hints about which models to use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<Vec<ModelHint>>,
    /// Cost priority (0.0 to 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_priority: Option<f64>,
    /// Speed priority (0.0 to 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_priority: Option<f64>,
    /// Intelligence priority (0.0 to 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intelligence_priority: Option<f64>,
}

/// Model hint.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelHint {
    /// Model name hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Include context option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IncludeContext {
    /// No context.
    None,
    /// This server's context.
    ThisServer,
    /// All servers' context.
    AllServers,
}

/// Sampling result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SamplingResult {
    /// Model used.
    pub model: String,
    /// Stop reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// Message role.
    pub role: Role,
    /// Content.
    pub content: Content,
}

/// Stop reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    /// End turn.
    EndTurn,
    /// Stop sequence hit.
    StopSequence,
    /// Max tokens reached.
    MaxTokens,
}
