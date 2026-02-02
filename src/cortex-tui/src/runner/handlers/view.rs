//! View management handlers.

use crate::app::AppView;

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle toggle sidebar action.
    pub(crate) fn handle_toggle_sidebar(&mut self) -> Result<bool> {
        self.state.toggle_sidebar();
        Ok(true)
    }

    /// Handle settings action - show settings view.
    pub(crate) fn handle_settings(&mut self) -> Result<bool> {
        self.state.set_view(AppView::Settings);
        Ok(true)
    }

    /// Handle cycle permission mode action - cycle through permission modes.
    pub(crate) fn handle_cycle_permission_mode(&mut self) -> Result<bool> {
        self.state.cycle_permission_mode();
        Ok(true)
    }

    /// Handle toggle tool details action - toggle tool details visibility.
    ///
    /// Currently logs the request for debugging purposes. Proper tool details
    /// toggling is planned for future implementation to show/hide expanded
    /// information about tool executions in the chat view.
    pub(crate) fn handle_toggle_tool_details(&mut self) -> Result<bool> {
        // Feature placeholder: tool details toggling (planned for future implementation)
        tracing::debug!("Toggle tool details requested");
        Ok(true)
    }
}
