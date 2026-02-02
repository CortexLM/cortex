//! Model switching handlers.

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle switch model action - show model picker.
    ///
    /// Currently logs the request for tracking purposes. A model picker dialog
    /// is planned for future implementation to allow users to interactively
    /// select from available AI models.
    pub(crate) fn handle_switch_model(&mut self) -> Result<bool> {
        // Feature placeholder: model picker dialog (planned for future implementation)
        tracing::info!("Switch model requested");
        Ok(true)
    }

    // handle_switch_provider removed: provider is now always "cortex"
}
