//! Operation mode handlers.

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle toggle operation mode action - cycle Build -> Plan -> Spec -> Build.
    ///
    /// This changes the agent's operation mode:
    /// - Build: Full access, can modify files and execute commands
    /// - Plan: Read-only, describes changes without applying them
    /// - Spec: Generates a detailed plan for approval before building
    pub(crate) fn handle_toggle_operation_mode(&mut self) -> Result<bool> {
        let old_mode = self.state.get_operation_mode();
        self.state.toggle_operation_mode();
        let new_mode = self.state.get_operation_mode();

        tracing::info!(
            "Operation mode changed: {} ({}) -> {} ({})",
            old_mode.name(),
            old_mode.indicator(),
            new_mode.name(),
            new_mode.indicator()
        );

        // Show toast notification for mode change
        self.state.toasts.info(format!(
            "Mode: {} {}",
            new_mode.indicator(),
            new_mode.name()
        ));

        Ok(true)
    }

    /// Handle approve spec action - approve a spec plan and transition to build mode.
    ///
    /// Currently transitions to Build mode and shows a success toast. Full spec
    /// plan approval flow is planned for future implementation, which will involve:
    /// - Getting the pending spec plan from the approval manager
    /// - Validating the plan before transition
    /// - Applying the approved plan with proper error handling
    pub(crate) async fn handle_approve_spec(&mut self) -> Result<bool> {
        if !self.state.is_spec_mode() {
            tracing::debug!("Approve spec called but not in spec mode");
            return Ok(false);
        }

        // Feature placeholder: full spec plan approval flow (planned for future implementation)

        tracing::info!("Spec plan approved - transitioning to Build mode");
        self.state
            .set_operation_mode(crate::app::OperationMode::Build);
        self.state
            .toasts
            .success("Plan approved - now in Build mode");

        Ok(true)
    }

    /// Handle reject spec action - reject the spec plan and stay in spec mode.
    ///
    /// Currently shows a warning toast and stays in Spec mode. Full spec plan
    /// rejection flow is planned for future implementation to clear the pending
    /// plan and optionally provide feedback to the agent.
    pub(crate) fn handle_reject_spec(&mut self) -> Result<bool> {
        if !self.state.is_spec_mode() {
            tracing::debug!("Reject spec called but not in spec mode");
            return Ok(false);
        }

        // Feature placeholder: spec plan rejection flow (planned for future implementation)
        tracing::info!("Spec plan rejected");
        self.state
            .toasts
            .warning("Plan rejected - staying in Spec mode");

        Ok(true)
    }
}
