//! Input handling (submit, newline, clear, history).

use crate::app::AppView;
use cortex_core::widgets::Message;

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle submit action - send user message to backend.
    ///
    /// This method:
    /// 1. Gets the text from the input widget
    /// 2. Checks for slash commands
    /// 3. Adds a user message to the conversation
    /// 4. Sends the message to the backend
    /// 5. Starts streaming state
    pub(crate) async fn handle_submit(&mut self) -> Result<bool> {
        let text = self.state.input.submit();
        if text.is_empty() {
            return Ok(false);
        }

        // Check for slash commands
        if text.starts_with('/') {
            return self.handle_slash_command(&text).await;
        }

        // Add user message
        let message = Message::user(&text);
        self.state.add_message(message);

        // Send to backend
        if let Some(session) = self.session {
            session.send_message(text).await?;
            self.state.start_streaming(None);
            self.stream.start_processing();
        }

        // Ensure we're in session view when sending messages
        if self.state.view == AppView::Session && self.state.messages.is_empty() {
            self.state.set_view(AppView::Session);
        }

        Ok(true)
    }

    /// Handle newline action - insert newline in multiline input.
    ///
    /// In multiline mode, this inserts a newline character.
    /// For single-line mode, this does nothing (submit handles Enter).
    ///
    /// Currently returns false (unhandled). Multiline input mode detection
    /// and newline insertion is planned for future implementation.
    pub(crate) fn handle_newline(&mut self) -> Result<bool> {
        // Feature placeholder: multiline input mode support (planned for future implementation)
        Ok(false)
    }

    /// Handle clear action - clear the input field.
    pub(crate) fn handle_clear(&mut self) -> Result<bool> {
        self.state.input.clear();
        Ok(true)
    }

    /// Handle history previous action - navigate to previous input.
    pub(crate) fn handle_history_prev(&mut self) -> Result<bool> {
        self.state.input.history_prev();
        Ok(true)
    }

    /// Handle history next action - navigate to next input.
    pub(crate) fn handle_history_next(&mut self) -> Result<bool> {
        self.state.input.history_next();
        Ok(true)
    }
}
