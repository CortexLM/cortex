//! Configuration types shared across the protocol.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reasoning effort level for models that support it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    #[default]
    Medium,
    High,
}

/// How reasoning summaries should be delivered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummary {
    /// No reasoning summary.
    #[default]
    None,
    /// Brief summary of reasoning.
    Brief,
    /// Detailed reasoning summary.
    Detailed,
    /// Auto-select based on model.
    Auto,
}

/// Sandbox mode configuration.
/// NOTE: Cortex uses DangerFullAccess by default (no sandbox).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    /// Full access, no restrictions (DEFAULT for Cortex).
    #[default]
    DangerFullAccess,
    /// Read-only filesystem access.
    ReadOnly,
    /// Read access + write to workspace.
    WorkspaceWrite,
}

/// Trust level for projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    /// Project is trusted - relaxed approval policies.
    Trusted,
    /// Project is untrusted - strict approval required.
    Untrusted,
}

impl std::fmt::Display for TrustLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrustLevel::Trusted => write!(f, "trusted"),
            TrustLevel::Untrusted => write!(f, "untrusted"),
        }
    }
}

/// Forced login method restriction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ForcedLoginMethod {
    /// Only allow API key authentication.
    ApiKey,
    /// Only allow ChatGPT login.
    ChatGpt,
}

/// Verbosity level for model output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    /// Minimal output.
    Terse,
    /// Standard output.
    Normal,
    /// Detailed output.
    Verbose,
}

/// Format for reasoning summaries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummaryFormat {
    /// Plain text format.
    #[default]
    Text,
    /// Structured format with sections.
    Structured,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reasoning_effort_serde() {
        let effort = ReasoningEffort::High;
        let json = serde_json::to_string(&effort).expect("serialize");
        assert_eq!(json, "\"high\"");

        let parsed: ReasoningEffort = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(effort, parsed);
    }

    #[test]
    fn test_sandbox_mode_serde() {
        // Default is now DangerFullAccess
        let mode = SandboxMode::default();
        let json = serde_json::to_string(&mode).expect("serialize");
        assert_eq!(json, "\"danger-full-access\"");

        let mode = SandboxMode::WorkspaceWrite;
        let json = serde_json::to_string(&mode).expect("serialize");
        assert_eq!(json, "\"workspace-write\"");
    }
}
