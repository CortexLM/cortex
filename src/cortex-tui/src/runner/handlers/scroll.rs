//! Scroll action handlers.

use crate::app::FocusTarget;

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle scroll action - scroll focused component by delta.
    ///
    /// Positive delta scrolls down, negative scrolls up.
    pub(crate) fn handle_scroll(&mut self, delta: i32) -> Result<bool> {
        match self.state.focus {
            FocusTarget::Chat => self.state.scroll_chat(delta),
            FocusTarget::Sidebar => self.state.scroll_sidebar(delta),
            _ => {}
        }
        Ok(true)
    }

    /// Handle scroll to top action.
    pub(crate) fn handle_scroll_to_top(&mut self) -> Result<bool> {
        self.state.scroll_chat_to_top();
        Ok(true)
    }

    /// Handle scroll to bottom action.
    pub(crate) fn handle_scroll_to_bottom(&mut self) -> Result<bool> {
        self.state.scroll_chat_to_bottom();
        Ok(true)
    }
}
