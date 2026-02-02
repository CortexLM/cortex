//! Event and action system for cortex-core.
//!
//! This module provides the core event handling infrastructure including:
//! - [`Action`] - core actions the engine can dispatch
//! - [`InputAction`] - text input specific actions
//! - [`EventBus`] - manages event distribution via channels
//! - [`KeyMapper`] - trait for mapping key events to actions
//! - [`DefaultKeyMapper`] - standard key bindings implementation

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

/// Actions for text input handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// A character was typed
    Char(char),
    /// Backspace key - delete character before cursor
    Backspace,
    /// Delete key - delete character at cursor
    Delete,
    /// Enter/Return key
    Enter,
    /// Tab key
    Tab,
    /// Left arrow - move cursor left
    Left,
    /// Right arrow - move cursor right
    Right,
    /// Home key - move cursor to start
    Home,
    /// End key - move cursor to end
    End,
    /// Up arrow in input - previous history item
    HistoryPrev,
    /// Down arrow in input - next history item
    HistoryNext,
    /// Clear the input
    Clear,
    /// Paste text from clipboard
    Paste(String),
}

/// Core actions the engine can dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    // Navigation
    /// Quit the application
    Quit,
    /// Focus the next widget
    FocusNext,
    /// Focus the previous widget
    FocusPrev,

    // Input
    /// Text input action
    Input(InputAction),

    // Scrolling
    /// Scroll up by the specified number of lines
    ScrollUp(u16),
    /// Scroll down by the specified number of lines
    ScrollDown(u16),
    /// Scroll to the top
    ScrollToTop,
    /// Scroll to the bottom
    ScrollToBottom,
    /// Page up
    PageUp,
    /// Page down
    PageDown,

    // Selection
    /// Select the current item
    Select,
    /// Cancel the current operation
    Cancel,

    // View
    /// Toggle the sidebar visibility
    ToggleSidebar,
    /// Toggle help display
    ToggleHelp,

    // Animation
    /// Animation tick for frame updates
    Tick,

    // Custom app action (for cortex-tui to extend)
    /// Custom action for application-specific behavior
    Custom(String),

    // No-op
    /// No operation
    None,
}

impl Default for Action {
    fn default() -> Self {
        Self::None
    }
}

/// Manages event distribution via async channels.
///
/// The `EventBus` provides a simple publish-subscribe mechanism for
/// dispatching actions throughout the application.
///
/// # Example
///
/// ```ignore
/// let mut bus = EventBus::new(100);
/// let sender = bus.sender();
///
/// // Send an action from another task
/// sender.send(Action::Quit).await.unwrap();
///
/// // Receive the action
/// if let Some(action) = bus.recv().await {
///     // Handle action
/// }
/// ```
pub struct EventBus {
    action_tx: mpsc::Sender<Action>,
    action_rx: mpsc::Receiver<Action>,
}

impl EventBus {
    /// Create a new event bus with the specified channel capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - The maximum number of actions that can be buffered
    pub fn new(capacity: usize) -> Self {
        let (action_tx, action_rx) = mpsc::channel(capacity);
        Self {
            action_tx,
            action_rx,
        }
    }

    /// Get a clone of the sender for dispatching actions.
    ///
    /// This can be cloned and shared across tasks to allow
    /// multiple producers to send actions.
    pub fn sender(&self) -> mpsc::Sender<Action> {
        self.action_tx.clone()
    }

    /// Asynchronously receive the next action.
    ///
    /// Returns `None` if all senders have been dropped.
    pub async fn recv(&mut self) -> Option<Action> {
        self.action_rx.recv().await
    }

    /// Try to receive an action without blocking.
    ///
    /// Returns `None` if no action is available or if all senders have been dropped.
    pub fn try_recv(&mut self) -> Option<Action> {
        self.action_rx.try_recv().ok()
    }
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("capacity", &"<channel>")
            .finish()
    }
}

/// Trait for mapping key events to actions.
///
/// Implement this trait to create custom key bindings for your application.
///
/// # Example
///
/// ```ignore
/// struct VimKeyMapper;
///
/// impl KeyMapper for VimKeyMapper {
///     fn map_key(&self, key: KeyEvent) -> Action {
///         match key.code {
///             KeyCode::Char('h') => Action::ScrollUp(1),
///             KeyCode::Char('l') => Action::ScrollDown(1),
///             _ => Action::None,
///         }
///     }
/// }
/// ```
pub trait KeyMapper {
    /// Map a key event to an action.
    ///
    /// Returns [`Action::None`] if the key should not trigger any action.
    fn map_key(&self, key: KeyEvent) -> Action;
}

/// Default key mapper with standard bindings.
///
/// # Key Bindings
///
/// | Key | Action |
/// |-----|--------|
/// | `q`, `Ctrl+c` | Quit |
/// | `Tab` | Focus Next |
/// | `Shift+Tab` | Focus Previous |
/// | `Up`, `k` | Scroll Up |
/// | `Down`, `j` | Scroll Down |
/// | `PageUp` | Page Up |
/// | `PageDown` | Page Down |
/// | `Home` | Scroll to Top |
/// | `End` | Scroll to Bottom |
/// | `Enter` | Select |
/// | `Esc` | Cancel |
/// | `?` | Toggle Help |
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultKeyMapper;

impl DefaultKeyMapper {
    /// Create a new default key mapper.
    pub fn new() -> Self {
        Self
    }
}

impl KeyMapper for DefaultKeyMapper {
    fn map_key(&self, key: KeyEvent) -> Action {
        // Check for Ctrl+c first
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if let KeyCode::Char('c') = key.code {
                return Action::Quit;
            }
        }

        // Check for Shift+Tab
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            if let KeyCode::BackTab = key.code {
                return Action::FocusPrev;
            }
        }

        // Standard key mappings
        match key.code {
            // Quit
            KeyCode::Char('q') => Action::Quit,

            // Focus navigation
            KeyCode::Tab => Action::FocusNext,

            // Scrolling - arrow keys and vim-style
            KeyCode::Up | KeyCode::Char('k') => Action::ScrollUp(1),
            KeyCode::Down | KeyCode::Char('j') => Action::ScrollDown(1),

            // Page navigation
            KeyCode::PageUp => Action::PageUp,
            KeyCode::PageDown => Action::PageDown,

            // Jump to boundaries
            KeyCode::Home => Action::ScrollToTop,
            KeyCode::End => Action::ScrollToBottom,

            // Selection
            KeyCode::Enter => Action::Select,
            KeyCode::Esc => Action::Cancel,

            // Help
            KeyCode::Char('?') => Action::ToggleHelp,

            // No mapping
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key_event(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_event_with_modifiers(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new_with_kind(code, modifiers, KeyEventKind::Press)
    }

    #[test]
    fn test_default_mapper_quit() {
        let mapper = DefaultKeyMapper::new();

        // q quits
        assert_eq!(mapper.map_key(key_event(KeyCode::Char('q'))), Action::Quit);

        // Ctrl+c quits
        assert_eq!(
            mapper.map_key(key_event_with_modifiers(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL
            )),
            Action::Quit
        );
    }

    #[test]
    fn test_default_mapper_focus() {
        let mapper = DefaultKeyMapper::new();

        // Tab focuses next
        assert_eq!(mapper.map_key(key_event(KeyCode::Tab)), Action::FocusNext);

        // Shift+Tab focuses previous
        assert_eq!(
            mapper.map_key(key_event_with_modifiers(
                KeyCode::BackTab,
                KeyModifiers::SHIFT
            )),
            Action::FocusPrev
        );
    }

    #[test]
    fn test_default_mapper_scrolling() {
        let mapper = DefaultKeyMapper::new();

        // Arrow keys
        assert_eq!(mapper.map_key(key_event(KeyCode::Up)), Action::ScrollUp(1));
        assert_eq!(
            mapper.map_key(key_event(KeyCode::Down)),
            Action::ScrollDown(1)
        );

        // Vim-style
        assert_eq!(
            mapper.map_key(key_event(KeyCode::Char('k'))),
            Action::ScrollUp(1)
        );
        assert_eq!(
            mapper.map_key(key_event(KeyCode::Char('j'))),
            Action::ScrollDown(1)
        );

        // Page navigation
        assert_eq!(mapper.map_key(key_event(KeyCode::PageUp)), Action::PageUp);
        assert_eq!(
            mapper.map_key(key_event(KeyCode::PageDown)),
            Action::PageDown
        );

        // Boundaries
        assert_eq!(
            mapper.map_key(key_event(KeyCode::Home)),
            Action::ScrollToTop
        );
        assert_eq!(
            mapper.map_key(key_event(KeyCode::End)),
            Action::ScrollToBottom
        );
    }

    #[test]
    fn test_default_mapper_selection() {
        let mapper = DefaultKeyMapper::new();

        assert_eq!(mapper.map_key(key_event(KeyCode::Enter)), Action::Select);
        assert_eq!(mapper.map_key(key_event(KeyCode::Esc)), Action::Cancel);
    }

    #[test]
    fn test_default_mapper_help() {
        let mapper = DefaultKeyMapper::new();

        assert_eq!(
            mapper.map_key(key_event(KeyCode::Char('?'))),
            Action::ToggleHelp
        );
    }

    #[test]
    fn test_default_mapper_unmapped() {
        let mapper = DefaultKeyMapper::new();

        // Unmapped keys return None
        assert_eq!(mapper.map_key(key_event(KeyCode::Char('x'))), Action::None);
        assert_eq!(mapper.map_key(key_event(KeyCode::F(1))), Action::None);
    }

    #[test]
    fn test_action_default() {
        assert_eq!(Action::default(), Action::None);
    }

    #[test]
    fn test_input_action_equality() {
        assert_eq!(InputAction::Char('a'), InputAction::Char('a'));
        assert_ne!(InputAction::Char('a'), InputAction::Char('b'));
        assert_eq!(
            InputAction::Paste("test".to_string()),
            InputAction::Paste("test".to_string())
        );
    }

    #[test]
    fn test_action_equality() {
        assert_eq!(Action::ScrollUp(5), Action::ScrollUp(5));
        assert_ne!(Action::ScrollUp(5), Action::ScrollUp(10));
        assert_eq!(
            Action::Custom("test".to_string()),
            Action::Custom("test".to_string())
        );
        assert_eq!(
            Action::Input(InputAction::Char('a')),
            Action::Input(InputAction::Char('a'))
        );
    }

    #[tokio::test]
    async fn test_event_bus_send_recv() {
        let mut bus = EventBus::new(10);
        let sender = bus.sender();

        sender.send(Action::Quit).await.unwrap();
        sender.send(Action::Select).await.unwrap();

        assert_eq!(bus.recv().await, Some(Action::Quit));
        assert_eq!(bus.recv().await, Some(Action::Select));
    }

    #[tokio::test]
    async fn test_event_bus_try_recv() {
        let mut bus = EventBus::new(10);
        let sender = bus.sender();

        // Empty bus returns None
        assert_eq!(bus.try_recv(), None);

        sender.send(Action::Tick).await.unwrap();

        // Now it should return the action
        assert_eq!(bus.try_recv(), Some(Action::Tick));

        // Empty again
        assert_eq!(bus.try_recv(), None);
    }

    #[tokio::test]
    async fn test_event_bus_multiple_senders() {
        let mut bus = EventBus::new(10);
        let sender1 = bus.sender();
        let sender2 = bus.sender();

        sender1.send(Action::FocusNext).await.unwrap();
        sender2.send(Action::FocusPrev).await.unwrap();

        let action1 = bus.recv().await.unwrap();
        let action2 = bus.recv().await.unwrap();

        // Both actions should be received (order depends on scheduling)
        assert!(action1 == Action::FocusNext || action1 == Action::FocusPrev);
        assert!(action2 == Action::FocusNext || action2 == Action::FocusPrev);
        assert_ne!(action1, action2);
    }

    #[test]
    fn test_event_bus_debug() {
        let bus = EventBus::new(10);
        let debug_str = format!("{:?}", bus);
        assert!(debug_str.contains("EventBus"));
    }
}
