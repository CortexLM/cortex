//! Spec mode system for cortex-agents.
//!
//! This module provides the specification mode functionality, allowing agents to
//! generate detailed implementation plans before executing changes.
//!
//! ## Overview
//!
//! The spec system includes:
//! - [`OperationMode`] - The agent operation mode (Build, Plan, Spec)
//! - [`SpecPlan`] - A detailed specification plan
//! - [`ApprovalManager`] - Manages approval flow for spec plans
//! - [`ModeTransition`] - Handles mode transitions

pub mod approval;
pub mod plan;
pub mod transition;

pub use approval::{ApprovalDecision, ApprovalManager, ApprovalRequest};
pub use plan::{ChangeType, FileChange, SpecPlan, SpecStep};
pub use transition::ModeTransition;

use serde::{Deserialize, Serialize};

/// Agent operation mode controlling what actions the agent can perform.
///
/// This is distinct from [`crate::AgentMode`] which controls whether an agent
/// is a primary agent or subagent. `OperationMode` controls the agent's
/// capabilities within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OperationMode {
    /// Build mode - full access, can modify files and execute commands
    #[default]
    Build,
    /// Plan mode - read-only, suggests changes without applying them
    Plan,
    /// Spec mode - generates a detailed plan before implementation
    Spec,
}

impl OperationMode {
    /// Returns true if the mode allows writing to files.
    pub fn can_write(&self) -> bool {
        matches!(self, OperationMode::Build)
    }

    /// Returns true if the mode allows executing commands.
    pub fn can_execute(&self) -> bool {
        matches!(self, OperationMode::Build)
    }

    /// Returns true if approval is needed before building.
    pub fn needs_approval_before_build(&self) -> bool {
        matches!(self, OperationMode::Spec)
    }

    /// Returns the display name for UI.
    pub fn display_name(&self) -> &'static str {
        match self {
            OperationMode::Build => "Build",
            OperationMode::Plan => "Plan",
            OperationMode::Spec => "Spec",
        }
    }

    /// Returns an emoji indicator for the mode.
    pub fn indicator(&self) -> &'static str {
        match self {
            OperationMode::Build => "[B]",
            OperationMode::Plan => "[P]",
            OperationMode::Spec => "[S]",
        }
    }

    /// Returns a short description of the mode.
    pub fn description(&self) -> &'static str {
        match self {
            OperationMode::Build => "Full access - can modify files and execute commands",
            OperationMode::Plan => "Read-only - describes changes without applying them",
            OperationMode::Spec => "Specification - generates a plan for approval before building",
        }
    }

    /// Cycle to the next mode (Tab toggle).
    ///
    /// Order: Build -> Plan -> Spec -> Build
    pub fn next(&self) -> Self {
        match self {
            OperationMode::Build => OperationMode::Plan,
            OperationMode::Plan => OperationMode::Spec,
            OperationMode::Spec => OperationMode::Build,
        }
    }

    /// Cycle to the previous mode.
    ///
    /// Order: Build -> Spec -> Plan -> Build
    pub fn prev(&self) -> Self {
        match self {
            OperationMode::Build => OperationMode::Spec,
            OperationMode::Plan => OperationMode::Build,
            OperationMode::Spec => OperationMode::Plan,
        }
    }

    /// Returns the color associated with this mode for UI display.
    ///
    /// Returns RGB tuple (r, g, b).
    pub fn color(&self) -> (u8, u8, u8) {
        match self {
            OperationMode::Build => (0, 255, 163), // Green (success color)
            OperationMode::Plan => (255, 200, 87), // Yellow/amber (warning color)
            OperationMode::Spec => (139, 92, 246), // Purple (spec color)
        }
    }
}

impl std::fmt::Display for OperationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_toggle_cycle() {
        let mode = OperationMode::Build;
        assert_eq!(mode.next(), OperationMode::Plan);
        assert_eq!(mode.next().next(), OperationMode::Spec);
        assert_eq!(mode.next().next().next(), OperationMode::Build);
    }

    #[test]
    fn test_mode_permissions() {
        assert!(OperationMode::Build.can_write());
        assert!(OperationMode::Build.can_execute());

        assert!(!OperationMode::Plan.can_write());
        assert!(!OperationMode::Plan.can_execute());

        assert!(!OperationMode::Spec.can_write());
        assert!(!OperationMode::Spec.can_execute());
    }

    #[test]
    fn test_mode_approval() {
        assert!(!OperationMode::Build.needs_approval_before_build());
        assert!(!OperationMode::Plan.needs_approval_before_build());
        assert!(OperationMode::Spec.needs_approval_before_build());
    }

    #[test]
    fn test_mode_display() {
        assert_eq!(OperationMode::Build.display_name(), "Build");
        assert_eq!(OperationMode::Plan.display_name(), "Plan");
        assert_eq!(OperationMode::Spec.display_name(), "Spec");
    }

    #[test]
    fn test_mode_indicator() {
        assert_eq!(OperationMode::Build.indicator(), "[B]");
        assert_eq!(OperationMode::Plan.indicator(), "[P]");
        assert_eq!(OperationMode::Spec.indicator(), "[S]");
    }

    #[test]
    fn test_default_is_build() {
        assert_eq!(OperationMode::default(), OperationMode::Build);
    }

    #[test]
    fn test_mode_serialization() {
        let build = OperationMode::Build;
        let serialized = serde_json::to_string(&build).unwrap();
        assert_eq!(serialized, "\"build\"");

        let deserialized: OperationMode = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, build);
    }
}
