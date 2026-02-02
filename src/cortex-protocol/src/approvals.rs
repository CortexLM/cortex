//! Approval-related types.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::{SandboxCommandAssessment, SandboxRiskLevel};

/// Request for approval to apply a patch.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ApplyPatchApprovalRequestEvent {
    /// Unique identifier for this approval request.
    pub call_id: String,
    /// Turn ID this request belongs to.
    pub turn_id: String,
    /// The patch content to apply.
    pub patch: String,
    /// Files that will be modified.
    pub files: Vec<PathBuf>,
    /// Summary of changes.
    pub summary: PatchSummary,
}

/// Summary of a patch.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
pub struct PatchSummary {
    /// Number of files added.
    pub files_added: usize,
    /// Number of files modified.
    pub files_modified: usize,
    /// Number of files deleted.
    pub files_deleted: usize,
    /// Total lines added.
    pub lines_added: usize,
    /// Total lines removed.
    pub lines_removed: usize,
}

/// Request for MCP elicitation (user input required by MCP server).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ElicitationRequestEvent {
    /// Name of the MCP server requesting elicitation.
    pub server_name: String,
    /// Request ID from the MCP server.
    pub request_id: String,
    /// Message to display to the user.
    pub message: String,
    /// Schema for the expected input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
}

/// Re-exports from protocol for convenience.
pub use crate::protocol::{ElicitationAction, ExecApprovalRequestEvent, ReviewDecision};

impl PatchSummary {
    /// Create a new empty patch summary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the patch is empty.
    pub fn is_empty(&self) -> bool {
        self.files_added == 0 && self.files_modified == 0 && self.files_deleted == 0
    }

    /// Get total number of files affected.
    pub fn total_files(&self) -> usize {
        self.files_added + self.files_modified + self.files_deleted
    }

    /// Get total line changes.
    pub fn total_lines_changed(&self) -> usize {
        self.lines_added + self.lines_removed
    }
}

/// Assessment of command risk level.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CommandRiskAssessment {
    /// Overall risk level.
    pub level: SandboxRiskLevel,
    /// Explanation of the risk assessment.
    pub explanation: String,
    /// Specific concerns identified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concerns: Vec<String>,
    /// Whether the command is reversible.
    pub reversible: bool,
}

impl CommandRiskAssessment {
    /// Create a low-risk assessment.
    pub fn low(explanation: impl Into<String>) -> Self {
        Self {
            level: SandboxRiskLevel::Low,
            explanation: explanation.into(),
            concerns: vec![],
            reversible: true,
        }
    }

    /// Create a medium-risk assessment.
    pub fn medium(explanation: impl Into<String>, concerns: Vec<String>) -> Self {
        Self {
            level: SandboxRiskLevel::Medium,
            explanation: explanation.into(),
            concerns,
            reversible: true,
        }
    }

    /// Create a high-risk assessment.
    pub fn high(explanation: impl Into<String>, concerns: Vec<String>) -> Self {
        Self {
            level: SandboxRiskLevel::High,
            explanation: explanation.into(),
            concerns,
            reversible: false,
        }
    }
}

impl From<CommandRiskAssessment> for SandboxCommandAssessment {
    fn from(assessment: CommandRiskAssessment) -> Self {
        Self {
            risk_level: assessment.level,
            explanation: assessment.explanation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_summary() {
        let mut summary = PatchSummary::new();
        assert!(summary.is_empty());

        summary.files_added = 2;
        summary.lines_added = 100;
        assert!(!summary.is_empty());
        assert_eq!(summary.total_files(), 2);
        assert_eq!(summary.total_lines_changed(), 100);
    }

    #[test]
    fn test_command_risk_assessment() {
        let low = CommandRiskAssessment::low("Safe read-only command");
        assert_eq!(low.level, SandboxRiskLevel::Low);
        assert!(low.reversible);

        let high =
            CommandRiskAssessment::high("Deletes files", vec!["Permanent deletion".to_string()]);
        assert_eq!(high.level, SandboxRiskLevel::High);
        assert!(!high.reversible);
    }
}
