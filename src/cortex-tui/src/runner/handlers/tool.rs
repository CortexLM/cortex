//! Tool action handlers.

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle cancel tool action - cancel current tool execution.
    pub(crate) async fn handle_cancel_tool(&mut self) -> Result<bool> {
        if let Some(session) = self.session {
            session.interrupt().await?;
        }
        self.stream.interrupt();
        Ok(true)
    }

    /// Handle retry tool action - retry last failed tool.
    ///
    /// Currently logs the request for tracking purposes. Retry functionality
    /// is planned for future implementation to allow users to re-execute
    /// the last failed tool with the same or modified parameters.
    pub(crate) async fn handle_retry_tool(&mut self) -> Result<bool> {
        // Feature placeholder: retry failed tool execution (planned for future implementation)
        tracing::info!("Retry tool requested");
        Ok(true)
    }
}
