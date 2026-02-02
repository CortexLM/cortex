//! Mouse input types and handling.
//!
//! This module provides types for representing mouse events, including button presses,
//! movement, scrolling, and drag operations.

use crate::keyboard::KeyModifiers;
use std::fmt;

/// Represents a mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MouseButton {
    /// Left mouse button (primary).
    #[default]
    Left,
    /// Right mouse button (secondary).
    Right,
    /// Middle mouse button (scroll wheel click).
    Middle,
}

impl MouseButton {
    /// Converts a button number to a `MouseButton`.
    #[must_use]
    pub fn from_number(n: u8) -> Self {
        match n {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::Left,
        }
    }

    /// Converts this button to its numeric representation.
    #[must_use]
    pub fn to_number(self) -> u8 {
        match self {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        }
    }
}

impl fmt::Display for MouseButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MouseButton::Left => write!(f, "left"),
            MouseButton::Right => write!(f, "right"),
            MouseButton::Middle => write!(f, "middle"),
        }
    }
}

impl From<crossterm::event::MouseButton> for MouseButton {
    fn from(btn: crossterm::event::MouseButton) -> Self {
        match btn {
            crossterm::event::MouseButton::Left => MouseButton::Left,
            crossterm::event::MouseButton::Right => MouseButton::Right,
            crossterm::event::MouseButton::Middle => MouseButton::Middle,
        }
    }
}

/// The kind of mouse event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseEventKind {
    /// A button was pressed down.
    Down(MouseButton),
    /// A button was released.
    Up(MouseButton),
    /// The mouse was dragged while a button was held.
    Drag(MouseButton),
    /// The mouse was moved without any buttons pressed.
    Moved,
    /// The scroll wheel was scrolled down.
    ScrollDown,
    /// The scroll wheel was scrolled up.
    ScrollUp,
    /// The scroll wheel was scrolled left (horizontal scroll).
    ScrollLeft,
    /// The scroll wheel was scrolled right (horizontal scroll).
    ScrollRight,
}

impl MouseEventKind {
    /// Returns true if this is a button down event.
    #[must_use]
    pub fn is_down(&self) -> bool {
        matches!(self, MouseEventKind::Down(_))
    }

    /// Returns true if this is a button up event.
    #[must_use]
    pub fn is_up(&self) -> bool {
        matches!(self, MouseEventKind::Up(_))
    }

    /// Returns true if this is a drag event.
    #[must_use]
    pub fn is_drag(&self) -> bool {
        matches!(self, MouseEventKind::Drag(_))
    }

    /// Returns true if this is a move event (no buttons pressed).
    #[must_use]
    pub fn is_move(&self) -> bool {
        matches!(self, MouseEventKind::Moved)
    }

    /// Returns true if this is a scroll event.
    #[must_use]
    pub fn is_scroll(&self) -> bool {
        matches!(
            self,
            MouseEventKind::ScrollDown
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        )
    }

    /// Returns the button associated with this event, if any.
    #[must_use]
    pub fn button(&self) -> Option<MouseButton> {
        match self {
            MouseEventKind::Down(btn) | MouseEventKind::Up(btn) | MouseEventKind::Drag(btn) => {
                Some(*btn)
            }
            _ => None,
        }
    }
}

impl fmt::Display for MouseEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MouseEventKind::Down(btn) => write!(f, "down({btn})"),
            MouseEventKind::Up(btn) => write!(f, "up({btn})"),
            MouseEventKind::Drag(btn) => write!(f, "drag({btn})"),
            MouseEventKind::Moved => write!(f, "moved"),
            MouseEventKind::ScrollDown => write!(f, "scroll_down"),
            MouseEventKind::ScrollUp => write!(f, "scroll_up"),
            MouseEventKind::ScrollLeft => write!(f, "scroll_left"),
            MouseEventKind::ScrollRight => write!(f, "scroll_right"),
        }
    }
}

impl From<crossterm::event::MouseEventKind> for MouseEventKind {
    fn from(kind: crossterm::event::MouseEventKind) -> Self {
        match kind {
            crossterm::event::MouseEventKind::Down(btn) => MouseEventKind::Down(btn.into()),
            crossterm::event::MouseEventKind::Up(btn) => MouseEventKind::Up(btn.into()),
            crossterm::event::MouseEventKind::Drag(btn) => MouseEventKind::Drag(btn.into()),
            crossterm::event::MouseEventKind::Moved => MouseEventKind::Moved,
            crossterm::event::MouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
            crossterm::event::MouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
            crossterm::event::MouseEventKind::ScrollLeft => MouseEventKind::ScrollLeft,
            crossterm::event::MouseEventKind::ScrollRight => MouseEventKind::ScrollRight,
        }
    }
}

/// A complete mouse event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MouseEvent {
    /// The kind of mouse event.
    pub kind: MouseEventKind,
    /// The column (x coordinate) where the event occurred.
    pub column: u16,
    /// The row (y coordinate) where the event occurred.
    pub row: u16,
    /// Active keyboard modifiers during this event.
    pub modifiers: KeyModifiers,
}

impl MouseEvent {
    /// Creates a new mouse event.
    #[must_use]
    pub fn new(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> Self {
        Self {
            kind,
            column,
            row,
            modifiers,
        }
    }

    /// Creates a simple button down event.
    #[must_use]
    pub fn down(button: MouseButton, column: u16, row: u16) -> Self {
        Self::new(
            MouseEventKind::Down(button),
            column,
            row,
            KeyModifiers::NONE,
        )
    }

    /// Creates a simple button up event.
    #[must_use]
    pub fn up(button: MouseButton, column: u16, row: u16) -> Self {
        Self::new(MouseEventKind::Up(button), column, row, KeyModifiers::NONE)
    }

    /// Creates a simple move event.
    #[must_use]
    pub fn moved(column: u16, row: u16) -> Self {
        Self::new(MouseEventKind::Moved, column, row, KeyModifiers::NONE)
    }

    /// Returns the position as a tuple (column, row).
    #[must_use]
    pub fn position(&self) -> (u16, u16) {
        (self.column, self.row)
    }

    /// Returns true if the Control modifier was held.
    #[must_use]
    pub fn ctrl(&self) -> bool {
        self.modifiers.contains(KeyModifiers::CONTROL)
    }

    /// Returns true if the Alt modifier was held.
    #[must_use]
    pub fn alt(&self) -> bool {
        self.modifiers.contains(KeyModifiers::ALT)
    }

    /// Returns true if the Shift modifier was held.
    #[must_use]
    pub fn shift(&self) -> bool {
        self.modifiers.contains(KeyModifiers::SHIFT)
    }

    /// Checks if this event occurred within the given rectangular bounds.
    #[must_use]
    pub fn is_within(&self, x: u16, y: u16, width: u16, height: u16) -> bool {
        self.column >= x
            && self.column < x.saturating_add(width)
            && self.row >= y
            && self.row < y.saturating_add(height)
    }
}

impl Default for MouseEvent {
    fn default() -> Self {
        Self {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }
}

impl fmt::Display for MouseEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@({},{})", self.kind, self.column, self.row)
    }
}

impl From<crossterm::event::MouseEvent> for MouseEvent {
    fn from(event: crossterm::event::MouseEvent) -> Self {
        Self {
            kind: event.kind.into(),
            column: event.column,
            row: event.row,
            modifiers: event.modifiers.into(),
        }
    }
}

/// Tracks mouse button state for drag detection.
#[derive(Debug, Clone, Default)]
pub struct MouseState {
    /// Currently pressed buttons.
    pressed_buttons: [bool; 3],
    /// Last known mouse position.
    last_position: Option<(u16, u16)>,
    /// Position where the last button press occurred.
    press_position: Option<(u16, u16)>,
    /// Button that initiated the current drag.
    drag_button: Option<MouseButton>,
}

impl MouseState {
    /// Creates a new mouse state tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates state based on a mouse event and returns additional derived events if any.
    pub fn update(&mut self, event: &MouseEvent) -> Option<MouseEvent> {
        let position = event.position();

        match event.kind {
            MouseEventKind::Down(btn) => {
                let idx = btn.to_number() as usize;
                if idx < 3 {
                    self.pressed_buttons[idx] = true;
                }
                self.press_position = Some(position);
                self.last_position = Some(position);
                None
            }
            MouseEventKind::Up(btn) => {
                let idx = btn.to_number() as usize;
                if idx < 3 {
                    self.pressed_buttons[idx] = false;
                }
                self.last_position = Some(position);

                if self.drag_button == Some(btn) {
                    self.drag_button = None;
                    self.press_position = None;
                }
                None
            }
            MouseEventKind::Moved => {
                self.last_position = Some(position);

                if let Some(btn) = self.any_button_pressed() {
                    self.drag_button = Some(btn);
                    Some(MouseEvent::new(
                        MouseEventKind::Drag(btn),
                        event.column,
                        event.row,
                        event.modifiers,
                    ))
                } else {
                    None
                }
            }
            MouseEventKind::Drag(_) => {
                self.last_position = Some(position);
                None
            }
            _ => {
                self.last_position = Some(position);
                None
            }
        }
    }

    /// Returns the first currently pressed button, if any.
    #[must_use]
    pub fn any_button_pressed(&self) -> Option<MouseButton> {
        if self.pressed_buttons[0] {
            Some(MouseButton::Left)
        } else if self.pressed_buttons[1] {
            Some(MouseButton::Middle)
        } else if self.pressed_buttons[2] {
            Some(MouseButton::Right)
        } else {
            None
        }
    }

    /// Returns true if the specified button is currently pressed.
    #[must_use]
    pub fn is_button_pressed(&self, button: MouseButton) -> bool {
        let idx = button.to_number() as usize;
        idx < 3 && self.pressed_buttons[idx]
    }

    /// Returns true if any button is currently pressed.
    #[must_use]
    pub fn is_any_button_pressed(&self) -> bool {
        self.pressed_buttons.iter().any(|&pressed| pressed)
    }

    /// Returns true if a drag operation is in progress.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.drag_button.is_some()
    }

    /// Returns the button being used for the current drag, if any.
    #[must_use]
    pub fn drag_button(&self) -> Option<MouseButton> {
        self.drag_button
    }

    /// Returns the last known mouse position.
    #[must_use]
    pub fn last_position(&self) -> Option<(u16, u16)> {
        self.last_position
    }

    /// Returns the position where the current press/drag started.
    #[must_use]
    pub fn press_position(&self) -> Option<(u16, u16)> {
        self.press_position
    }

    /// Calculates the drag distance from the initial press position.
    #[must_use]
    pub fn drag_distance(&self) -> Option<(i32, i32)> {
        match (self.press_position, self.last_position) {
            (Some((px, py)), Some((lx, ly))) => {
                Some((i32::from(lx) - i32::from(px), i32::from(ly) - i32::from(py)))
            }
            _ => None,
        }
    }

    /// Resets the mouse state.
    pub fn reset(&mut self) {
        self.pressed_buttons = [false; 3];
        self.last_position = None;
        self.press_position = None;
        self.drag_button = None;
    }
}

/// Direction for scroll events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScrollDirection {
    /// Scrolling up.
    Up,
    /// Scrolling down.
    Down,
    /// Scrolling left.
    Left,
    /// Scrolling right.
    Right,
}

impl ScrollDirection {
    /// Returns the delta for this scroll direction.
    #[must_use]
    pub fn delta(self) -> (i8, i8) {
        match self {
            ScrollDirection::Up => (0, -1),
            ScrollDirection::Down => (0, 1),
            ScrollDirection::Left => (-1, 0),
            ScrollDirection::Right => (1, 0),
        }
    }

    /// Returns true if this is a vertical scroll direction.
    #[must_use]
    pub fn is_vertical(self) -> bool {
        matches!(self, ScrollDirection::Up | ScrollDirection::Down)
    }

    /// Returns true if this is a horizontal scroll direction.
    #[must_use]
    pub fn is_horizontal(self) -> bool {
        matches!(self, ScrollDirection::Left | ScrollDirection::Right)
    }
}

impl From<MouseEventKind> for Option<ScrollDirection> {
    fn from(kind: MouseEventKind) -> Self {
        match kind {
            MouseEventKind::ScrollUp => Some(ScrollDirection::Up),
            MouseEventKind::ScrollDown => Some(ScrollDirection::Down),
            MouseEventKind::ScrollLeft => Some(ScrollDirection::Left),
            MouseEventKind::ScrollRight => Some(ScrollDirection::Right),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mouse_button_conversion() {
        assert_eq!(MouseButton::from_number(0), MouseButton::Left);
        assert_eq!(MouseButton::from_number(1), MouseButton::Middle);
        assert_eq!(MouseButton::from_number(2), MouseButton::Right);

        assert_eq!(MouseButton::Left.to_number(), 0);
        assert_eq!(MouseButton::Middle.to_number(), 1);
        assert_eq!(MouseButton::Right.to_number(), 2);
    }

    #[test]
    fn test_mouse_event_kind_predicates() {
        assert!(MouseEventKind::Down(MouseButton::Left).is_down());
        assert!(MouseEventKind::Up(MouseButton::Left).is_up());
        assert!(MouseEventKind::Drag(MouseButton::Left).is_drag());
        assert!(MouseEventKind::Moved.is_move());
        assert!(MouseEventKind::ScrollDown.is_scroll());
    }

    #[test]
    fn test_mouse_event_is_within() {
        let event = MouseEvent::new(MouseEventKind::Moved, 10, 10, KeyModifiers::NONE);

        assert!(event.is_within(5, 5, 10, 10));
        assert!(event.is_within(10, 10, 1, 1));
        assert!(!event.is_within(0, 0, 5, 5));
        assert!(!event.is_within(15, 15, 5, 5));
    }

    #[test]
    fn test_mouse_state_tracking() {
        let mut state = MouseState::new();

        let down = MouseEvent::down(MouseButton::Left, 10, 10);
        state.update(&down);
        assert!(state.is_button_pressed(MouseButton::Left));
        assert!(state.is_any_button_pressed());
        assert_eq!(state.press_position(), Some((10, 10)));

        let move_event = MouseEvent::moved(15, 15);
        let drag = state.update(&move_event);
        assert!(drag.is_some());
        assert!(state.is_dragging());
        assert_eq!(state.drag_button(), Some(MouseButton::Left));

        let up = MouseEvent::up(MouseButton::Left, 20, 20);
        state.update(&up);
        assert!(!state.is_button_pressed(MouseButton::Left));
        assert!(!state.is_dragging());
    }

    #[test]
    fn test_scroll_direction() {
        assert_eq!(ScrollDirection::Up.delta(), (0, -1));
        assert_eq!(ScrollDirection::Down.delta(), (0, 1));
        assert_eq!(ScrollDirection::Left.delta(), (-1, 0));
        assert_eq!(ScrollDirection::Right.delta(), (1, 0));

        assert!(ScrollDirection::Up.is_vertical());
        assert!(ScrollDirection::Left.is_horizontal());
    }
}
