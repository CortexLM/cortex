//! Core action handlers (quit, help, cancel).

use crate::app::AppView;

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle quit action - shutdown session and exit application.
    pub(crate) async fn handle_quit(&mut self) -> Result<bool> {
        // If streaming, interrupt first
        if self.state.streaming.is_streaming {
            self.handle_cancel().await?;
        }

        // Shutdown session if active
        if let Some(session) = self.session
            && let Err(e) = session.shutdown().await
        {
            tracing::warn!("Error during session shutdown: {}", e);
        }

        self.state.running = false;
        Ok(true)
    }

    /// Handle help action - show help view.
    pub(crate) fn handle_help(&mut self) -> Result<bool> {
        self.state.set_view(AppView::Help);
        Ok(true)
    }

    /// Handle cancel action - context-dependent cancellation.
    ///
    /// Behavior depends on current state:
    /// - If pending approval: reject it
    /// - If streaming: interrupt the stream
    /// - If in modal view: go back to previous view
    pub(crate) async fn handle_cancel(&mut self) -> Result<bool> {
        // Cancel depends on current state
        if self.state.pending_approval.is_some() {
            self.handle_reject().await?;
        } else if self.state.streaming.is_streaming {
            if let Some(session) = self.session
                && let Err(e) = session.interrupt().await
            {
                tracing::warn!("Error interrupting session: {}", e);
            }
            self.stream.interrupt();
            self.state.stop_streaming();
        } else if self.state.view != AppView::Session {
            self.state.go_back();
        }
        Ok(true)
    }
}
