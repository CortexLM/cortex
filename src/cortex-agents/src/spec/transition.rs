//! Mode transition system for switching between operation modes.
//!
//! This module handles the transitions between Build, Plan, and Spec modes,
//! including the system prompt modifications needed for each mode.

use super::{OperationMode, SpecPlan};
use serde::{Deserialize, Serialize};

/// Represents a transition between operation modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeTransition {
    /// Mode transitioning from
    pub from: OperationMode,
    /// Mode transitioning to
    pub to: OperationMode,
    /// Optional plan to apply (for Spec -> Build transitions)
    pub plan: Option<SpecPlan>,
}

impl ModeTransition {
    /// Create a simple toggle transition to the next mode.
    pub fn toggle(current: OperationMode) -> Self {
        Self {
            from: current,
            to: current.next(),
            plan: None,
        }
    }

    /// Create a direct transition to a specific mode.
    pub fn to_mode(from: OperationMode, to: OperationMode) -> Self {
        Self {
            from,
            to,
            plan: None,
        }
    }

    /// Create a transition from Spec to Build with an approved plan.
    pub fn spec_to_build(plan: SpecPlan) -> Self {
        Self {
            from: OperationMode::Spec,
            to: OperationMode::Build,
            plan: Some(plan),
        }
    }

    /// Check if this transition carries a plan to execute.
    pub fn has_plan(&self) -> bool {
        self.plan.is_some()
    }

    /// Get the plan if present.
    pub fn take_plan(&mut self) -> Option<SpecPlan> {
        self.plan.take()
    }

    /// Apply the transition and return the modified system prompt.
    ///
    /// This prepends mode-specific instructions to the base system prompt.
    pub fn apply(&self, base_prompt: &str) -> String {
        match self.to {
            OperationMode::Build => {
                if let Some(plan) = &self.plan {
                    // Build mode with a plan to execute
                    format!(
                        "{}\n\n{}\n\n{}",
                        base_prompt,
                        BUILD_MODE_INSTRUCTIONS,
                        format_plan_instructions(plan)
                    )
                } else {
                    // Normal build mode
                    format!("{}\n\n{}", base_prompt, BUILD_MODE_INSTRUCTIONS)
                }
            }
            OperationMode::Plan => {
                format!("{}\n\n{}", base_prompt, PLAN_MODE_INSTRUCTIONS)
            }
            OperationMode::Spec => {
                format!("{}\n\n{}", base_prompt, SPEC_MODE_INSTRUCTIONS)
            }
        }
    }

    /// Get a description of the transition for logging.
    pub fn description(&self) -> String {
        let base = format!(
            "Mode transition: {} -> {}",
            self.from.display_name(),
            self.to.display_name()
        );
        if self.has_plan() {
            format!("{} (with plan)", base)
        } else {
            base
        }
    }
}

/// Instructions prepended to the system prompt in Build mode.
const BUILD_MODE_INSTRUCTIONS: &str = r#"## Mode: Build (Full Access)

You are in BUILD MODE with full access to modify the codebase.
- You CAN create, edit, and delete files
- You CAN execute shell commands
- You CAN make changes to implement features
- Always verify your changes work correctly"#;

/// Instructions prepended to the system prompt in Plan mode.
const PLAN_MODE_INSTRUCTIONS: &str = r#"## Mode: Plan (Read-Only)

You are in PLAN MODE. You CANNOT modify files or execute write commands.
Instead, describe HOW you would implement the requested changes:
- Explain your approach step by step
- List the files you would modify
- Show code snippets as examples
- DO NOT use Edit, Create, Delete, or Execute tools that modify files

You can still:
- Read files to understand the codebase
- Search for code patterns
- Execute read-only commands (ls, cat, grep, etc.)"#;

/// Instructions prepended to the system prompt in Spec mode.
const SPEC_MODE_INSTRUCTIONS: &str = r#"## Mode: Specification

You are in SPEC MODE. Generate a detailed implementation plan BEFORE making changes.

Your task:
1. Analyze the request thoroughly
2. Create a structured plan with numbered steps
3. List ALL files that will be affected
4. Describe each change in detail
5. Submit the plan for user approval using the SpecPlan tool
6. Wait for approval before proceeding to implementation

Plan format:
- Title: Brief description of the feature/change
- Summary: What the plan accomplishes
- Steps: Ordered list of implementation steps
- For each step, list the file changes (create/modify/delete)

DO NOT make any changes until the plan is approved.
After approval, you will transition to Build mode to implement the plan."#;

/// Format plan execution instructions for Build mode after Spec approval.
fn format_plan_instructions(plan: &SpecPlan) -> String {
    let mut instructions = String::from("## Approved Plan to Execute\n\n");
    instructions.push_str("The following plan has been approved. Implement it step by step:\n\n");
    instructions.push_str(&plan.to_markdown());
    instructions.push_str("\n\nExecute each step in order, verifying as you go.");
    instructions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toggle_transition() {
        let transition = ModeTransition::toggle(OperationMode::Build);
        assert_eq!(transition.from, OperationMode::Build);
        assert_eq!(transition.to, OperationMode::Plan);
        assert!(!transition.has_plan());
    }

    #[test]
    fn test_spec_to_build_transition() {
        let plan = SpecPlan::new("Test Plan");
        let transition = ModeTransition::spec_to_build(plan);

        assert_eq!(transition.from, OperationMode::Spec);
        assert_eq!(transition.to, OperationMode::Build);
        assert!(transition.has_plan());
    }

    #[test]
    fn test_apply_build_mode() {
        let transition = ModeTransition::to_mode(OperationMode::Plan, OperationMode::Build);
        let result = transition.apply("Base prompt");

        assert!(result.contains("Base prompt"));
        assert!(result.contains("BUILD MODE"));
        assert!(result.contains("full access"));
    }

    #[test]
    fn test_apply_plan_mode() {
        let transition = ModeTransition::to_mode(OperationMode::Build, OperationMode::Plan);
        let result = transition.apply("Base prompt");

        assert!(result.contains("Base prompt"));
        assert!(result.contains("PLAN MODE"));
        assert!(result.contains("CANNOT modify"));
    }

    #[test]
    fn test_apply_spec_mode() {
        let transition = ModeTransition::to_mode(OperationMode::Build, OperationMode::Spec);
        let result = transition.apply("Base prompt");

        assert!(result.contains("Base prompt"));
        assert!(result.contains("SPEC MODE"));
        assert!(result.contains("implementation plan"));
    }

    #[test]
    fn test_apply_with_plan() {
        let plan = SpecPlan::new("Feature X").with_summary("Implement feature X");
        let transition = ModeTransition::spec_to_build(plan);

        let result = transition.apply("Base prompt");

        assert!(result.contains("Base prompt"));
        assert!(result.contains("Approved Plan"));
        assert!(result.contains("Feature X"));
    }

    #[test]
    fn test_take_plan() {
        let plan = SpecPlan::new("Test");
        let mut transition = ModeTransition::spec_to_build(plan);

        assert!(transition.has_plan());
        let taken = transition.take_plan();
        assert!(taken.is_some());
        assert!(!transition.has_plan());
    }

    #[test]
    fn test_transition_description() {
        let transition = ModeTransition::toggle(OperationMode::Build);
        assert!(transition.description().contains("Build"));
        assert!(transition.description().contains("Plan"));

        let with_plan = ModeTransition::spec_to_build(SpecPlan::new("Test"));
        assert!(with_plan.description().contains("with plan"));
    }
}
