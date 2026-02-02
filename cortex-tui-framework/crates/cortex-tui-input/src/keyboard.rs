//! Keyboard input types and handling.
//!
//! This module provides types for representing keyboard events, including key codes,
//! modifiers, and the complete key event structure.

use bitflags::bitflags;
use std::fmt;

/// Represents the type of key event (press, release, or repeat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum KeyEventKind {
    /// Key was pressed down.
    #[default]
    Press,
    /// Key is being held down and repeating.
    Repeat,
    /// Key was released.
    Release,
}

impl fmt::Display for KeyEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyEventKind::Press => write!(f, "press"),
            KeyEventKind::Repeat => write!(f, "repeat"),
            KeyEventKind::Release => write!(f, "release"),
        }
    }
}

impl From<crossterm::event::KeyEventKind> for KeyEventKind {
    fn from(kind: crossterm::event::KeyEventKind) -> Self {
        match kind {
            crossterm::event::KeyEventKind::Press => KeyEventKind::Press,
            crossterm::event::KeyEventKind::Repeat => KeyEventKind::Repeat,
            crossterm::event::KeyEventKind::Release => KeyEventKind::Release,
        }
    }
}

/// Represents a key on the keyboard.
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
    /// Shift+Tab (backtab).
    BackTab,
    /// Delete key.
    Delete,
    /// Insert key.
    Insert,
    /// Function key F1-F24.
    F(u8),
    /// A regular character key.
    Char(char),
    /// Null character (Ctrl+Space or Ctrl+@).
    Null,
    /// Escape key.
    Esc,
    /// Caps Lock key.
    CapsLock,
    /// Scroll Lock key.
    ScrollLock,
    /// Num Lock key.
    NumLock,
    /// Print Screen key.
    PrintScreen,
    /// Pause key.
    Pause,
    /// Menu key.
    Menu,
    /// Keypad Begin (5 without NumLock).
    KeypadBegin,
}

impl KeyCode {
    /// Returns the normalized name for this key code.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            KeyCode::Backspace => "backspace".to_string(),
            KeyCode::Enter => "enter".to_string(),
            KeyCode::Left => "left".to_string(),
            KeyCode::Right => "right".to_string(),
            KeyCode::Up => "up".to_string(),
            KeyCode::Down => "down".to_string(),
            KeyCode::Home => "home".to_string(),
            KeyCode::End => "end".to_string(),
            KeyCode::PageUp => "pageup".to_string(),
            KeyCode::PageDown => "pagedown".to_string(),
            KeyCode::Tab => "tab".to_string(),
            KeyCode::BackTab => "backtab".to_string(),
            KeyCode::Delete => "delete".to_string(),
            KeyCode::Insert => "insert".to_string(),
            KeyCode::F(n) => format!("f{n}"),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Null => "null".to_string(),
            KeyCode::Esc => "escape".to_string(),
            KeyCode::CapsLock => "capslock".to_string(),
            KeyCode::ScrollLock => "scrolllock".to_string(),
            KeyCode::NumLock => "numlock".to_string(),
            KeyCode::PrintScreen => "printscreen".to_string(),
            KeyCode::Pause => "pause".to_string(),
            KeyCode::Menu => "menu".to_string(),
            KeyCode::KeypadBegin => "keypadbegin".to_string(),
        }
    }

    /// Returns true if this key code represents a digit (0-9).
    #[must_use]
    pub fn is_digit(&self) -> bool {
        matches!(self, KeyCode::Char('0'..='9'))
    }

    /// Returns true if this key code represents an alphabetic character.
    #[must_use]
    pub fn is_alphabetic(&self) -> bool {
        matches!(self, KeyCode::Char(c) if c.is_alphabetic())
    }

    /// Returns true if this key code is a function key (F1-F24).
    #[must_use]
    pub fn is_function_key(&self) -> bool {
        matches!(self, KeyCode::F(_))
    }

    /// Returns true if this key code is an arrow key.
    #[must_use]
    pub fn is_arrow_key(&self) -> bool {
        matches!(
            self,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
        )
    }

    /// Returns true if this key code is a navigation key.
    #[must_use]
    pub fn is_navigation_key(&self) -> bool {
        matches!(
            self,
            KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
        )
    }
}

impl fmt::Display for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl From<crossterm::event::KeyCode> for KeyCode {
    fn from(code: crossterm::event::KeyCode) -> Self {
        use crossterm::event::KeyCode as CT;
        match code {
            CT::Backspace => KeyCode::Backspace,
            CT::Enter => KeyCode::Enter,
            CT::Left => KeyCode::Left,
            CT::Right => KeyCode::Right,
            CT::Up => KeyCode::Up,
            CT::Down => KeyCode::Down,
            CT::Home => KeyCode::Home,
            CT::End => KeyCode::End,
            CT::PageUp => KeyCode::PageUp,
            CT::PageDown => KeyCode::PageDown,
            CT::Tab => KeyCode::Tab,
            CT::BackTab => KeyCode::BackTab,
            CT::Delete => KeyCode::Delete,
            CT::Insert => KeyCode::Insert,
            CT::F(n) => KeyCode::F(n),
            CT::Char(c) => KeyCode::Char(c),
            CT::Null => KeyCode::Null,
            CT::Esc => KeyCode::Esc,
            CT::CapsLock => KeyCode::CapsLock,
            CT::ScrollLock => KeyCode::ScrollLock,
            CT::NumLock => KeyCode::NumLock,
            CT::PrintScreen => KeyCode::PrintScreen,
            CT::Pause => KeyCode::Pause,
            CT::Menu => KeyCode::Menu,
            CT::KeypadBegin => KeyCode::KeypadBegin,
            CT::Media(_) | CT::Modifier(_) => KeyCode::Null,
        }
    }
}

bitflags! {
    /// Keyboard modifier flags.
    ///
    /// Multiple modifiers can be combined using bitwise OR.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct KeyModifiers: u16 {
        /// No modifiers pressed.
        const NONE = 0b0000_0000_0000_0000;
        /// Shift modifier.
        const SHIFT = 0b0000_0000_0000_0001;
        /// Control modifier.
        const CONTROL = 0b0000_0000_0000_0010;
        /// Alt/Option modifier.
        const ALT = 0b0000_0000_0000_0100;
        /// Super/Windows/Command modifier.
        const SUPER = 0b0000_0000_0000_1000;
        /// Hyper modifier (rare, mostly Linux).
        const HYPER = 0b0000_0000_0001_0000;
        /// Meta modifier (rare, mostly Linux).
        const META = 0b0000_0000_0010_0000;
    }
}

impl fmt::Display for KeyModifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl");
        }
        if self.contains(KeyModifiers::ALT) {
            parts.push("Alt");
        }
        if self.contains(KeyModifiers::SHIFT) {
            parts.push("Shift");
        }
        if self.contains(KeyModifiers::SUPER) {
            parts.push("Super");
        }
        if self.contains(KeyModifiers::HYPER) {
            parts.push("Hyper");
        }
        if self.contains(KeyModifiers::META) {
            parts.push("Meta");
        }
        if parts.is_empty() {
            write!(f, "None")
        } else {
            write!(f, "{}", parts.join("+"))
        }
    }
}

impl From<crossterm::event::KeyModifiers> for KeyModifiers {
    fn from(mods: crossterm::event::KeyModifiers) -> Self {
        let mut result = KeyModifiers::NONE;
        if mods.contains(crossterm::event::KeyModifiers::SHIFT) {
            result |= KeyModifiers::SHIFT;
        }
        if mods.contains(crossterm::event::KeyModifiers::CONTROL) {
            result |= KeyModifiers::CONTROL;
        }
        if mods.contains(crossterm::event::KeyModifiers::ALT) {
            result |= KeyModifiers::ALT;
        }
        if mods.contains(crossterm::event::KeyModifiers::SUPER) {
            result |= KeyModifiers::SUPER;
        }
        if mods.contains(crossterm::event::KeyModifiers::HYPER) {
            result |= KeyModifiers::HYPER;
        }
        if mods.contains(crossterm::event::KeyModifiers::META) {
            result |= KeyModifiers::META;
        }
        result
    }
}

bitflags! {
    /// Additional key event state from the Kitty keyboard protocol.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct KeyEventState: u8 {
        /// No additional state.
        const NONE = 0b0000_0000;
        /// Key was triggered by the keypad.
        const KEYPAD = 0b0000_0001;
        /// Caps Lock was active during the event.
        const CAPS_LOCK = 0b0000_0010;
        /// Num Lock was active during the event.
        const NUM_LOCK = 0b0000_0100;
    }
}

impl From<crossterm::event::KeyEventState> for KeyEventState {
    fn from(state: crossterm::event::KeyEventState) -> Self {
        let mut result = KeyEventState::NONE;
        if state.contains(crossterm::event::KeyEventState::KEYPAD) {
            result |= KeyEventState::KEYPAD;
        }
        if state.contains(crossterm::event::KeyEventState::CAPS_LOCK) {
            result |= KeyEventState::CAPS_LOCK;
        }
        if state.contains(crossterm::event::KeyEventState::NUM_LOCK) {
            result |= KeyEventState::NUM_LOCK;
        }
        result
    }
}

/// A complete keyboard event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    /// The key code that was pressed/released.
    pub code: KeyCode,
    /// Active modifiers during this event.
    pub modifiers: KeyModifiers,
    /// The kind of event (press, repeat, release).
    pub kind: KeyEventKind,
    /// The state of the keyboard (Kitty protocol only).
    pub state: KeyEventState,
}

impl KeyEvent {
    /// Creates a new key event with default kind (Press) and state.
    #[must_use]
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    /// Creates a new key event with the specified kind.
    #[must_use]
    pub fn with_kind(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Self {
        Self {
            code,
            modifiers,
            kind,
            state: KeyEventState::empty(),
        }
    }

    /// Creates a key event for a simple character press with no modifiers.
    #[must_use]
    pub fn char(c: char) -> Self {
        Self::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Returns true if the Control modifier is pressed.
    #[must_use]
    pub fn ctrl(&self) -> bool {
        self.modifiers.contains(KeyModifiers::CONTROL)
    }

    /// Returns true if the Alt modifier is pressed.
    #[must_use]
    pub fn alt(&self) -> bool {
        self.modifiers.contains(KeyModifiers::ALT)
    }

    /// Returns true if the Shift modifier is pressed.
    #[must_use]
    pub fn shift(&self) -> bool {
        self.modifiers.contains(KeyModifiers::SHIFT)
    }

    /// Returns true if the Super modifier is pressed.
    #[must_use]
    pub fn super_key(&self) -> bool {
        self.modifiers.contains(KeyModifiers::SUPER)
    }

    /// Returns true if this is a key press event.
    #[must_use]
    pub fn is_press(&self) -> bool {
        self.kind == KeyEventKind::Press
    }

    /// Returns true if this is a key release event.
    #[must_use]
    pub fn is_release(&self) -> bool {
        self.kind == KeyEventKind::Release
    }

    /// Returns true if this is a key repeat event.
    #[must_use]
    pub fn is_repeat(&self) -> bool {
        self.kind == KeyEventKind::Repeat
    }

    /// Checks if this key event matches a specific pattern.
    #[must_use]
    pub fn matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.code == code && self.modifiers == modifiers
    }

    /// Checks if this key event matches a character shortcut.
    /// This method normalizes the character case for better international keyboard support.
    /// For example, Ctrl+C will match regardless of whether shift is also pressed,
    /// and handles keyboard layouts where the character might require shift.
    #[must_use]
    pub fn matches_char(&self, c: char, modifiers: KeyModifiers) -> bool {
        // Check that the required modifiers (excluding shift) are present
        let required_mods = modifiers - KeyModifiers::SHIFT;
        let actual_mods = self.modifiers - KeyModifiers::SHIFT;

        if required_mods != actual_mods {
            return false;
        }

        // Match the character in a case-insensitive way for letters
        match self.code {
            KeyCode::Char(key_char) => {
                let target_lower = c.to_lowercase().next().unwrap_or(c);
                let key_lower = key_char.to_lowercase().next().unwrap_or(key_char);
                target_lower == key_lower
            }
            _ => false,
        }
    }

    /// Check if this is a specific control+char shortcut (e.g., Ctrl+C, Ctrl+Z).
    /// This is layout-aware and handles international keyboards correctly.
    #[must_use]
    pub fn is_ctrl_char(&self, c: char) -> bool {
        self.ctrl() && self.matches_char(c, KeyModifiers::CONTROL)
    }

    /// Check if this is a specific alt+char shortcut.
    /// This is layout-aware and handles international keyboards correctly.
    #[must_use]
    pub fn is_alt_char(&self, c: char) -> bool {
        self.alt() && self.matches_char(c, KeyModifiers::ALT)
    }

    /// Returns a descriptive string for this key combination.
    #[must_use]
    pub fn to_shortcut_string(&self) -> String {
        let mut parts = Vec::new();

        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("Alt".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("Shift".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SUPER) {
            parts.push("Super".to_string());
        }

        parts.push(self.code.name());
        parts.join("+")
    }
}

impl Default for KeyEvent {
    fn default() -> Self {
        Self {
            code: KeyCode::Null,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }
}

impl fmt::Display for KeyEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_shortcut_string())
    }
}

impl From<crossterm::event::KeyEvent> for KeyEvent {
    fn from(event: crossterm::event::KeyEvent) -> Self {
        Self {
            code: event.code.into(),
            modifiers: event.modifiers.into(),
            kind: event.kind.into(),
            state: event.state.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_code_name() {
        assert_eq!(KeyCode::Enter.name(), "enter");
        assert_eq!(KeyCode::Char('a').name(), "a");
        assert_eq!(KeyCode::F(5).name(), "f5");
        assert_eq!(KeyCode::Esc.name(), "escape");
    }

    #[test]
    fn test_key_code_predicates() {
        assert!(KeyCode::Char('5').is_digit());
        assert!(!KeyCode::Char('a').is_digit());
        assert!(KeyCode::Char('Z').is_alphabetic());
        assert!(KeyCode::F(12).is_function_key());
        assert!(KeyCode::Up.is_arrow_key());
        assert!(KeyCode::Home.is_navigation_key());
    }

    #[test]
    fn test_key_modifiers_display() {
        assert_eq!(KeyModifiers::CONTROL.to_string(), "Ctrl");
        assert_eq!(
            (KeyModifiers::CONTROL | KeyModifiers::SHIFT).to_string(),
            "Ctrl+Shift"
        );
        assert_eq!(KeyModifiers::NONE.to_string(), "None");
    }

    #[test]
    fn test_key_event_matches() {
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(event.matches(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!event.matches(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(!event.matches(KeyCode::Char('x'), KeyModifiers::CONTROL));
    }

    #[test]
    fn test_key_event_shortcut_string() {
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(event.to_shortcut_string(), "Ctrl+s");

        let event2 = KeyEvent::new(
            KeyCode::Char('S'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(event2.to_shortcut_string(), "Ctrl+Shift+S");
    }

    #[test]
    fn test_key_event_predicates() {
        let press =
            KeyEvent::with_kind(KeyCode::Char('a'), KeyModifiers::NONE, KeyEventKind::Press);
        let release = KeyEvent::with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        let repeat =
            KeyEvent::with_kind(KeyCode::Char('a'), KeyModifiers::NONE, KeyEventKind::Repeat);

        assert!(press.is_press());
        assert!(release.is_release());
        assert!(repeat.is_repeat());
    }

    #[test]
    fn test_matches_char_case_insensitive() {
        // Ctrl+C should match both 'c' and 'C'
        let ctrl_c_lower = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let ctrl_c_upper = KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );

        assert!(ctrl_c_lower.matches_char('c', KeyModifiers::CONTROL));
        assert!(ctrl_c_lower.matches_char('C', KeyModifiers::CONTROL));
        assert!(ctrl_c_upper.matches_char('c', KeyModifiers::CONTROL));
        assert!(ctrl_c_upper.matches_char('C', KeyModifiers::CONTROL));
    }

    #[test]
    fn test_is_ctrl_char() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(ctrl_c.is_ctrl_char('c'));
        assert!(ctrl_c.is_ctrl_char('C'));
        assert!(!ctrl_c.is_ctrl_char('z'));

        let ctrl_z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert!(ctrl_z.is_ctrl_char('z'));
        assert!(!ctrl_z.is_ctrl_char('c'));
    }

    #[test]
    fn test_is_alt_char() {
        let alt_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        assert!(alt_x.is_alt_char('x'));
        assert!(alt_x.is_alt_char('X'));
        assert!(!alt_x.is_alt_char('y'));
    }
}
