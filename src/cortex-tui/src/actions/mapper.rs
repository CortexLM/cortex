//! ActionMapper - Maps keys to actions.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use uuid::Uuid;

use super::{ActionContext, KeyAction, KeyBinding};

/// Maps key events to actions based on the current context.
///
/// The mapper holds all key bindings and provides efficient lookup
/// of actions for given key events and contexts.
#[derive(Debug)]
pub struct ActionMapper {
    /// All registered key bindings.
    bindings: Vec<KeyBinding>,
}

impl ActionMapper {
    /// Create a new empty action mapper.
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// Create an action mapper with default bindings.
    pub fn default_bindings() -> Self {
        let mut mapper = Self::new();

        // === Global bindings ===
        mapper.add_bindings(vec![
            // Quit (only Ctrl+Q, not Ctrl+C which is now Copy)
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
                KeyAction::Quit,
                "Quit application",
            ),
            // Copy (Ctrl+C) - standard copy like most editors
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                KeyAction::Copy,
                "Copy selection",
            ),
            // Help
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
                KeyAction::Help,
                "Show help",
            ),
            KeyBinding::global(
                KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
                KeyAction::Help,
                "Show help",
            ),
            // Cancel
            KeyBinding::global(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                KeyAction::Cancel,
                "Cancel/close",
            ),
            // Focus navigation
            KeyBinding::global(
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                KeyAction::FocusNext,
                "Focus next element",
            ),
            KeyBinding::global(
                KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
                KeyAction::CyclePermissionMode,
                "Cycle permission mode",
            ),
            // Sidebar toggle (Ctrl+B only)
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
                KeyAction::ToggleSidebar,
                "Toggle sidebar",
            ),
            // Sessions card (Ctrl+S)
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                KeyAction::OpenSessions,
                "Open sessions",
            ),
            // New session
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
                KeyAction::NewSession,
                "New session",
            ),
            // Copy (Ctrl+Shift+C) - like terminal
            KeyBinding::global(
                KeyEvent::new(
                    KeyCode::Char('C'),
                    KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
                ),
                KeyAction::Copy,
                "Copy selection",
            ),
            // Paste (Ctrl+Shift+V) - like terminal
            KeyBinding::global(
                KeyEvent::new(
                    KeyCode::Char('V'),
                    KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
                ),
                KeyAction::Paste,
                "Paste from clipboard",
            ),
            // Model switching
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL),
                KeyAction::SwitchModel,
                "Switch model",
            ),
            // Focus shortcuts
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL),
                KeyAction::FocusInput,
                "Focus input",
            ),
            // === Card shortcuts (new minimalist UI) ===
            // Command palette
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
                KeyAction::OpenCommandPalette,
                "Open command palette",
            ),
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
                KeyAction::OpenCommandPalette,
                "Open command palette",
            ),
            // Sessions card
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
                KeyAction::OpenSessions,
                "Open sessions",
            ),
            // MCP servers card
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
                KeyAction::OpenMcp,
                "Open MCP servers",
            ),
            // View transcript
            KeyBinding::global(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
                KeyAction::ViewTranscript,
                "View transcript",
            ),
            // Provider switching (Ctrl+Shift+P) - REMOVED
            // Provider is now always "cortex", no need for provider picker
        ]);

        // === Input context bindings ===
        mapper.add_bindings(vec![
            // Submit
            KeyBinding::input(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                KeyAction::Submit,
                "Submit message",
            ),
            // New line (Shift+Enter)
            KeyBinding::input(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
                KeyAction::NewLine,
                "Insert new line",
            ),
            // History navigation
            KeyBinding::input(
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                KeyAction::HistoryPrev,
                "Previous in history",
            ),
            KeyBinding::input(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                KeyAction::HistoryNext,
                "Next in history",
            ),
            // Clear input
            KeyBinding::input(
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
                KeyAction::Clear,
                "Clear input",
            ),
            KeyBinding::input(
                KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
                KeyAction::Clear,
                "Clear input",
            ),
            // Paste
            KeyBinding::input(
                KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
                KeyAction::Paste,
                "Paste from clipboard",
            ),
            // Select all
            KeyBinding::input(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
                KeyAction::SelectAll,
                "Select all",
            ),
        ]);

        // === Chat context bindings ===
        mapper.add_bindings(vec![
            // Vim-style scrolling
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                KeyAction::ScrollDown,
                "Scroll down",
            ),
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                KeyAction::ScrollUp,
                "Scroll up",
            ),
            // Arrow key scrolling
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                KeyAction::ScrollDown,
                "Scroll down",
            ),
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                KeyAction::ScrollUp,
                "Scroll up",
            ),
            // Jump to top/bottom
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
                KeyAction::ScrollToTop,
                "Scroll to top",
            ),
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
                KeyAction::ScrollToBottom,
                "Scroll to bottom",
            ),
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
                KeyAction::ScrollToTop,
                "Scroll to top",
            ),
            KeyBinding::chat(
                KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
                KeyAction::ScrollToBottom,
                "Scroll to bottom",
            ),
            // Page up/down
            KeyBinding::chat(
                KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
                KeyAction::ScrollPageUp,
                "Page up",
            ),
            KeyBinding::chat(
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                KeyAction::ScrollPageDown,
                "Page down",
            ),
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
                KeyAction::ScrollPageUp,
                "Half page up",
            ),
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                KeyAction::ScrollPageDown,
                "Half page down",
            ),
            // Copy (vim yank)
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
                KeyAction::Copy,
                "Copy selected (yank)",
            ),
            // Paste (vim style)
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
                KeyAction::Paste,
                "Paste (vim)",
            ),
            // Cycle permission mode
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                KeyAction::CyclePermissionMode,
                "Cycle permission mode",
            ),
            // Toggle tool details
            KeyBinding::chat(
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
                KeyAction::ToggleToolDetails,
                "Toggle tool details",
            ),
        ]);

        // === Sidebar context bindings ===
        mapper.add_bindings(vec![
            // Load session
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                KeyAction::LoadSession(Uuid::nil()),
                "Load selected session",
            ),
            // Delete session
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
                KeyAction::DeleteSession,
                "Delete session",
            ),
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
                KeyAction::DeleteSession,
                "Delete session",
            ),
            // Rename session
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
                KeyAction::RenameSession,
                "Rename session",
            ),
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
                KeyAction::RenameSession,
                "Rename session",
            ),
            // Export session
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
                KeyAction::ExportSession,
                "Export session",
            ),
            // Navigation in sidebar
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                KeyAction::ScrollDown,
                "Move down",
            ),
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                KeyAction::ScrollUp,
                "Move up",
            ),
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                KeyAction::ScrollDown,
                "Move down",
            ),
            KeyBinding::sidebar(
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                KeyAction::ScrollUp,
                "Move up",
            ),
        ]);

        // === Approval context bindings ===
        mapper.add_bindings(vec![
            // Approve
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
                KeyAction::Approve,
                "Approve",
            ),
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                KeyAction::Approve,
                "Approve",
            ),
            // Reject
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
                KeyAction::Reject,
                "Reject",
            ),
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                KeyAction::Reject,
                "Reject",
            ),
            // Approve for session (allow this tool for the session)
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
                KeyAction::ApproveSession,
                "Approve for session",
            ),
            // Approve always (add to always-allowed list)
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                KeyAction::ApproveAlways,
                "Always allow",
            ),
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
                KeyAction::ApproveAll,
                "Approve all",
            ),
            // Reject all
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
                KeyAction::RejectAll,
                "Reject all",
            ),
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
                KeyAction::RejectAll,
                "Reject all",
            ),
            // View diff
            KeyBinding::approval(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
                KeyAction::ViewDiff,
                "View diff",
            ),
        ]);

        // === Help context bindings ===
        mapper.add_bindings(vec![
            // Close help
            KeyBinding::help(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                KeyAction::Cancel,
                "Close help",
            ),
            KeyBinding::help(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                KeyAction::Cancel,
                "Close help",
            ),
            KeyBinding::help(
                KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
                KeyAction::Cancel,
                "Close help",
            ),
            // Scroll in help
            KeyBinding::help(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                KeyAction::ScrollDown,
                "Scroll down",
            ),
            KeyBinding::help(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                KeyAction::ScrollUp,
                "Scroll up",
            ),
            KeyBinding::help(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                KeyAction::ScrollDown,
                "Scroll down",
            ),
            KeyBinding::help(
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                KeyAction::ScrollUp,
                "Scroll up",
            ),
        ]);

        mapper
    }

    /// Add a single binding.
    pub fn add_binding(&mut self, binding: KeyBinding) {
        self.bindings.push(binding);
    }

    /// Add multiple bindings.
    pub fn add_bindings(&mut self, bindings: Vec<KeyBinding>) {
        self.bindings.extend(bindings);
    }

    /// Get the action for a key event in the given context.
    ///
    /// Context-specific bindings take precedence over global bindings.
    pub fn get_action(&self, key: KeyEvent, context: ActionContext) -> KeyAction {
        // First, look for context-specific binding
        if context != ActionContext::Global {
            for binding in &self.bindings {
                if binding.context == context && keys_match(&binding.key, &key) {
                    return binding.action.clone();
                }
            }
        }

        // Then, look for global binding
        for binding in &self.bindings {
            if binding.context == ActionContext::Global && keys_match(&binding.key, &key) {
                return binding.action.clone();
            }
        }

        KeyAction::None
    }

    /// Get all bindings for a specific context.
    pub fn bindings_for_context(&self, context: ActionContext) -> Vec<&KeyBinding> {
        self.bindings
            .iter()
            .filter(|b| b.context == context)
            .collect()
    }

    /// Get all unique bindings.
    pub fn all_bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    /// Get bindings grouped by context for help display.
    pub fn bindings_by_context(&self) -> Vec<(ActionContext, Vec<&KeyBinding>)> {
        ActionContext::all()
            .iter()
            .map(|&ctx| (ctx, self.bindings_for_context(ctx)))
            .filter(|(_, bindings)| !bindings.is_empty())
            .collect()
    }
}

impl Default for ActionMapper {
    fn default() -> Self {
        Self::default_bindings()
    }
}

/// Check if two key events match (ignoring key state).
fn keys_match(a: &KeyEvent, b: &KeyEvent) -> bool {
    a.code == b.code && a.modifiers == b.modifiers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_mapper_default() {
        let mapper = ActionMapper::default_bindings();

        // Test global bindings - Ctrl+C is Copy (not Quit)
        let copy_key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            mapper.get_action(copy_key, ActionContext::Global),
            KeyAction::Copy
        );

        // Ctrl+Q is Quit
        let quit_key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(
            mapper.get_action(quit_key, ActionContext::Global),
            KeyAction::Quit
        );

        // Test context-specific bindings
        let submit_key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            mapper.get_action(submit_key, ActionContext::Input),
            KeyAction::Submit
        );

        // Test global fallback
        let help_key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        assert_eq!(
            mapper.get_action(help_key, ActionContext::Chat),
            KeyAction::Help
        );
    }

    #[test]
    fn test_action_mapper_context_override() {
        let mapper = ActionMapper::default_bindings();

        // 'y' in chat context should be Copy
        let y_key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        assert_eq!(
            mapper.get_action(y_key, ActionContext::Chat),
            KeyAction::Copy
        );

        // 'y' in approval context should be Approve
        assert_eq!(
            mapper.get_action(y_key, ActionContext::Approval),
            KeyAction::Approve
        );
    }

    #[test]
    fn test_bindings_for_context() {
        let mapper = ActionMapper::default_bindings();

        let global_bindings = mapper.bindings_for_context(ActionContext::Global);
        assert!(!global_bindings.is_empty());

        let input_bindings = mapper.bindings_for_context(ActionContext::Input);
        assert!(!input_bindings.is_empty());
    }
}
