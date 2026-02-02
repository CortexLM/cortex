//! Focus navigation handlers.

use crate::app::FocusTarget;

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle focus next action - cycle focus to next element.
    pub(crate) fn handle_focus_next(&mut self) -> Result<bool> {
        self.state.focus_next();
        Ok(true)
    }

    /// Handle focus previous action - cycle focus to previous element.
    pub(crate) fn handle_focus_prev(&mut self) -> Result<bool> {
        self.state.focus_prev();
        Ok(true)
    }

    /// Handle direct focus action - set focus to specific target.
    pub(crate) fn handle_focus(&mut self, target: FocusTarget) -> Result<bool> {
        self.state.set_focus(target);
        Ok(true)
    }
}
