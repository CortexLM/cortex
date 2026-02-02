//! Event types for input handling.
//!
//! This module defines events and event handling results used by widgets.

use std::fmt;

/// Keyboard key codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// Backspace key.
    Backspace,
    /// Enter/Return key.
    Enter,
    /// Left arrow key.
    Left,
    /// Right arrow key.
    Right,
    /// Up arrow key.
    Up,
    /// Down arrow key.
    Down,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page Up key.
    PageUp,
    /// Page Down key.
    PageDown,
    /// Tab key.
    Tab,
    /// Back Tab (Shift+Tab).
    BackTab,
    /// Delete key.
    Delete,
    /// Insert key.
    Insert,
    /// Escape key.
    Esc,
    /// Function key (F1-F24).
    F(u8),
    /// A character key.
    Char(char),
    /// Null key (Ctrl+Space on some terminals).
    Null,
}

impl fmt::Display for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backspace => write!(f, "Backspace"),
            Self::Enter => write!(f, "Enter"),
            Self::Left => write!(f, "Left"),
            Self::Right => write!(f, "Right"),
            Self::Up => write!(f, "Up"),
            Self::Down => write!(f, "Down"),
            Self::Home => write!(f, "Home"),
            Self::End => write!(f, "End"),
            Self::PageUp => write!(f, "PageUp"),
            Self::PageDown => write!(f, "PageDown"),
            Self::Tab => write!(f, "Tab"),
            Self::BackTab => write!(f, "BackTab"),
            Self::Delete => write!(f, "Delete"),
            Self::Insert => write!(f, "Insert"),
            Self::Esc => write!(f, "Esc"),
            Self::F(n) => write!(f, "F{}", n),
            Self::Char(c) => write!(f, "{}", c),
            Self::Null => write!(f, "Null"),
        }
    }
}

/// Keyboard modifier flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Shift key is pressed.
    pub shift: bool,
    /// Control key is pressed.
    pub ctrl: bool,
    /// Alt key is pressed.
    pub alt: bool,
    /// Meta/Super/Windows key is pressed.
    pub meta: bool,
}

impl Modifiers {
    /// No modifiers.
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    };

    /// Shift modifier only.
    pub const SHIFT: Self = Self {
        shift: true,
        ctrl: false,
        alt: false,
        meta: false,
    };

    /// Control modifier only.
    pub const CTRL: Self = Self {
        shift: false,
        ctrl: true,
        alt: false,
        meta: false,
    };

    /// Alt modifier only.
    pub const ALT: Self = Self {
        shift: false,
        ctrl: false,
        alt: true,
        meta: false,
    };

    /// Returns true if no modifiers are pressed.
    pub const fn is_empty(&self) -> bool {
        !self.shift && !self.ctrl && !self.alt && !self.meta
    }
}

impl fmt::Display for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        let mut write_mod = |s: &str| -> fmt::Result {
            if !first {
                write!(f, "+")?;
            }
            first = false;
            write!(f, "{}", s)
        };

        if self.ctrl {
            write_mod("Ctrl")?;
        }
        if self.alt {
            write_mod("Alt")?;
        }
        if self.shift {
            write_mod("Shift")?;
        }
        if self.meta {
            write_mod("Meta")?;
        }

        Ok(())
    }
}

/// A keyboard event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// The key code.
    pub code: KeyCode,
    /// The modifier keys.
    pub modifiers: Modifiers,
}

impl KeyEvent {
    /// Creates a new key event.
    pub const fn new(code: KeyCode, modifiers: Modifiers) -> Self {
        Self { code, modifiers }
    }

    /// Creates a key event with no modifiers.
    pub const fn plain(code: KeyCode) -> Self {
        Self::new(code, Modifiers::NONE)
    }

    /// Creates a key event for a character.
    pub const fn char(c: char) -> Self {
        Self::plain(KeyCode::Char(c))
    }

    /// Returns true if this is a character key.
    pub const fn is_char(&self) -> bool {
        matches!(self.code, KeyCode::Char(_))
    }

    /// Returns the character if this is a character key.
    pub const fn get_char(&self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        }
    }
}

impl fmt::Display for KeyEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.modifiers.is_empty() {
            write!(f, "{}+{}", self.modifiers, self.code)
        } else {
            write!(f, "{}", self.code)
        }
    }
}

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// Left mouse button.
    Left,
    /// Right mouse button.
    Right,
    /// Middle mouse button.
    Middle,
    /// Additional buttons (numbered).
    Other(u8),
}

/// Mouse event kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    /// Mouse button pressed.
    Down(MouseButton),
    /// Mouse button released.
    Up(MouseButton),
    /// Mouse moved (with button held).
    Drag(MouseButton),
    /// Mouse moved (no button held).
    Moved,
    /// Scroll up.
    ScrollUp,
    /// Scroll down.
    ScrollDown,
    /// Scroll left.
    ScrollLeft,
    /// Scroll right.
    ScrollRight,
}

/// A mouse event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    /// The kind of mouse event.
    pub kind: MouseEventKind,
    /// X coordinate (column).
    pub x: u16,
    /// Y coordinate (row).
    pub y: u16,
    /// Modifier keys held during the event.
    pub modifiers: Modifiers,
}

impl MouseEvent {
    /// Creates a new mouse event.
    pub const fn new(kind: MouseEventKind, x: u16, y: u16, modifiers: Modifiers) -> Self {
        Self {
            kind,
            x,
            y,
            modifiers,
        }
    }
}

/// Resize event containing new terminal dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeEvent {
    /// New width in columns.
    pub width: u16,
    /// New height in rows.
    pub height: u16,
}

impl ResizeEvent {
    /// Creates a new resize event.
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

/// Focus event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusEvent {
    /// Widget gained focus.
    Gained,
    /// Widget lost focus.
    Lost,
}

/// Paste event containing pasted text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteEvent {
    /// The pasted text.
    pub text: String,
}

impl PasteEvent {
    /// Creates a new paste event.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// All possible event types.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Keyboard event.
    Key(KeyEvent),
    /// Mouse event.
    Mouse(MouseEvent),
    /// Terminal resize event.
    Resize(ResizeEvent),
    /// Focus event.
    Focus(FocusEvent),
    /// Paste event.
    Paste(PasteEvent),
    /// Tick event for animations (deltatime in seconds).
    Tick(f32),
}

impl Event {
    /// Returns true if this is a key event.
    pub const fn is_key(&self) -> bool {
        matches!(self, Self::Key(_))
    }

    /// Returns the key event if this is a key event.
    pub const fn as_key(&self) -> Option<&KeyEvent> {
        match self {
            Self::Key(e) => Some(e),
            _ => None,
        }
    }

    /// Returns true if this is a mouse event.
    pub const fn is_mouse(&self) -> bool {
        matches!(self, Self::Mouse(_))
    }

    /// Returns the mouse event if this is a mouse event.
    pub const fn as_mouse(&self) -> Option<&MouseEvent> {
        match self {
            Self::Mouse(e) => Some(e),
            _ => None,
        }
    }
}

/// Result of event handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    /// Event was handled, should not propagate further.
    Handled,
    /// Event was not handled, should propagate to parent.
    Ignored,
}

impl EventResult {
    /// Returns true if the event was handled.
    pub const fn is_handled(&self) -> bool {
        matches!(self, Self::Handled)
    }

    /// Returns true if the event was ignored.
    pub const fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

impl From<bool> for EventResult {
    fn from(handled: bool) -> Self {
        if handled {
            Self::Handled
        } else {
            Self::Ignored
        }
    }
}

/// Combines multiple event results (handled if any handled).
impl std::ops::BitOr for EventResult {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        if self.is_handled() || rhs.is_handled() {
            Self::Handled
        } else {
            Self::Ignored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_event_display() {
        let event = KeyEvent::new(KeyCode::Char('a'), Modifiers::CTRL);
        assert_eq!(format!("{}", event), "Ctrl+a");

        let event = KeyEvent::plain(KeyCode::Enter);
        assert_eq!(format!("{}", event), "Enter");
    }

    #[test]
    fn test_event_result_or() {
        assert_eq!(
            EventResult::Handled | EventResult::Ignored,
            EventResult::Handled
        );
        assert_eq!(
            EventResult::Ignored | EventResult::Ignored,
            EventResult::Ignored
        );
        assert_eq!(
            EventResult::Handled | EventResult::Handled,
            EventResult::Handled
        );
    }

    #[test]
    fn test_event_as_key() {
        let event = Event::Key(KeyEvent::char('x'));
        assert!(event.is_key());
        assert_eq!(event.as_key().unwrap().get_char(), Some('x'));
    }
}
