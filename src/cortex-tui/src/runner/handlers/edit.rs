//! Edit action handlers (copy, paste, select all).

use crate::app::FocusTarget;

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle copy action - copy selection to clipboard.
    ///
    /// If text is selected in the chat area, copies it to clipboard.
    /// This is triggered by Ctrl+Shift+C or right-click.
    pub(crate) fn handle_copy(&mut self) -> Result<bool> {
        // Check if we have a text selection
        if self.state.text_selection.has_selection() {
            // The actual copy is done in event_loop via copy_selection_to_clipboard
            // Here we just signal that copy was requested
            tracing::debug!("Copy from selection requested");
            // Note: The actual clipboard copy happens in event_loop because
            // we need access to the terminal buffer which isn't available here.
            // We return true to indicate the action was handled.
            return Ok(true);
        }

        // If focus is on input, copy from input field
        if self.state.focus == FocusTarget::Input {
            // tui-textarea handles its own copy with Ctrl+C
            // For Ctrl+Shift+C, we let it pass through
            tracing::debug!("Copy from input requested");
        }

        Ok(true)
    }

    /// Handle paste action - paste from clipboard.
    ///
    /// Pastes clipboard content into the input field.
    pub(crate) fn handle_paste(&mut self) -> Result<bool> {
        // Only paste when input is focused
        if self.state.focus == FocusTarget::Input {
            match arboard::Clipboard::new() {
                Ok(mut clipboard) => {
                    if let Ok(text) = clipboard.get_text() {
                        // Insert text into input
                        self.state.input.insert_str(&text);
                        tracing::debug!("Pasted {} chars from clipboard", text.len());
                    }
                }
                Err(e) => {
                    tracing::warn!("Clipboard unavailable: {}", e);
                }
            }
        }
        Ok(true)
    }

    /// Handle select all action - select all in input.
    pub(crate) fn handle_select_all(&mut self) -> Result<bool> {
        if self.state.focus == FocusTarget::Input {
            self.state.input.select_all();
            tracing::debug!("Selected all text in input");
        }
        Ok(true)
    }
}
