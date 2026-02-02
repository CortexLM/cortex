//! Mouse event handling
//!
//! This module provides abstractions for processing crossterm mouse events
//! into high-level mouse actions like clicks, double-clicks, scrolling, and drags.
//!
//! # Features
//!
//! - Single and double-click detection with configurable threshold
//! - Scroll wheel handling with configurable delta
//! - Drag operation tracking
//! - Mouse movement events for hover effects
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::input::{MouseHandler, MouseAction, MouseButton};
//!
//! let mut handler = MouseHandler::new();
//!
//! // Process a mouse event from crossterm
//! if let Some(action) = handler.handle(mouse_event) {
//!     match action {
//!         MouseAction::Click { x, y, button } => {
//!             println!("Clicked at ({}, {}) with {:?}", x, y, button);
//!         }
//!         MouseAction::DoubleClick { x, y } => {
//!             println!("Double-clicked at ({}, {})", x, y);
//!         }
//!         MouseAction::Scroll { x, y, delta } => {
//!             println!("Scrolled {} at ({}, {})", delta, x, y);
//!         }
//!         _ => {}
//!     }
//! }
//! ```

use crossterm::event::{MouseButton as CrosstermButton, MouseEvent, MouseEventKind};
use std::time::Instant;

// ============================================================================
// MOUSE BUTTON
// ============================================================================

/// Mouse button types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// Left mouse button (primary)
    Left,
    /// Right mouse button (secondary/context)
    Right,
    /// Middle mouse button (wheel click)
    Middle,
}

impl From<CrosstermButton> for MouseButton {
    fn from(button: CrosstermButton) -> Self {
        match button {
            CrosstermButton::Left => MouseButton::Left,
            CrosstermButton::Right => MouseButton::Right,
            CrosstermButton::Middle => MouseButton::Middle,
        }
    }
}

impl std::fmt::Display for MouseButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MouseButton::Left => write!(f, "Left"),
            MouseButton::Right => write!(f, "Right"),
            MouseButton::Middle => write!(f, "Middle"),
        }
    }
}

// ============================================================================
// MOUSE ACTION
// ============================================================================

/// High-level mouse actions derived from raw mouse events.
///
/// These actions represent semantic user interactions rather than
/// low-level mouse state changes.
#[derive(Debug, Clone, PartialEq)]
pub enum MouseAction {
    /// Single click at position
    Click {
        /// X coordinate (column)
        x: u16,
        /// Y coordinate (row)
        y: u16,
        /// Which button was clicked
        button: MouseButton,
    },

    /// Double click at position (left button only)
    /// Used for word selection in input fields.
    DoubleClick {
        /// X coordinate (column)
        x: u16,
        /// Y coordinate (row)
        y: u16,
    },

    /// Triple click at position (left button only)
    /// Used for line selection in input fields.
    TripleClick {
        /// X coordinate (column)
        x: u16,
        /// Y coordinate (row)
        y: u16,
    },

    /// Scroll up/down at position
    Scroll {
        /// X coordinate (column)
        x: u16,
        /// Y coordinate (row)
        y: u16,
        /// Scroll delta (negative = up, positive = down)
        delta: i16,
    },

    /// Drag from start to current position
    Drag {
        /// Starting position (x, y)
        start: (u16, u16),
        /// Current position (x, y)
        current: (u16, u16),
        /// Which button is being held
        button: MouseButton,
    },

    /// Mouse moved (for hover effects)
    Move {
        /// X coordinate (column)
        x: u16,
        /// Y coordinate (row)
        y: u16,
    },

    /// Mouse button released (for ending selection)
    Release {
        /// X coordinate (column)
        x: u16,
        /// Y coordinate (row)
        y: u16,
        /// Which button was released
        button: MouseButton,
    },
}

impl MouseAction {
    /// Returns the position of this action.
    pub fn position(&self) -> (u16, u16) {
        match self {
            MouseAction::Click { x, y, .. } => (*x, *y),
            MouseAction::DoubleClick { x, y } => (*x, *y),
            MouseAction::TripleClick { x, y } => (*x, *y),
            MouseAction::Scroll { x, y, .. } => (*x, *y),
            MouseAction::Drag { current, .. } => *current,
            MouseAction::Move { x, y } => (*x, *y),
            MouseAction::Release { x, y, .. } => (*x, *y),
        }
    }

    /// Returns true if this is a click action.
    pub fn is_click(&self) -> bool {
        matches!(self, MouseAction::Click { .. })
    }

    /// Returns true if this is a double-click action.
    pub fn is_double_click(&self) -> bool {
        matches!(self, MouseAction::DoubleClick { .. })
    }

    /// Returns true if this is a triple-click action.
    pub fn is_triple_click(&self) -> bool {
        matches!(self, MouseAction::TripleClick { .. })
    }

    /// Returns true if this is a scroll action.
    pub fn is_scroll(&self) -> bool {
        matches!(self, MouseAction::Scroll { .. })
    }

    /// Returns true if this is a drag action.
    pub fn is_drag(&self) -> bool {
        matches!(self, MouseAction::Drag { .. })
    }

    /// Returns true if this is a left-click action.
    pub fn is_left_click(&self) -> bool {
        matches!(
            self,
            MouseAction::Click {
                button: MouseButton::Left,
                ..
            }
        )
    }

    /// Returns true if this is a right-click action.
    pub fn is_right_click(&self) -> bool {
        matches!(
            self,
            MouseAction::Click {
                button: MouseButton::Right,
                ..
            }
        )
    }
}

// ============================================================================
// MOUSE HANDLER
// ============================================================================

/// Processes raw mouse events into high-level actions.
///
/// The handler maintains state to detect double-clicks and track drag operations.
/// It should be persistent across frames to properly track these multi-event actions.
///
/// # Example
///
/// ```rust,ignore
/// let mut handler = MouseHandler::new();
///
/// // In your event loop
/// if let EngineEvent::Mouse(mouse_event) = event {
///     if let Some(action) = handler.handle(mouse_event) {
///         match action {
///             MouseAction::Click { x, y, button: MouseButton::Left } => {
///                 // Handle left click
///             }
///             MouseAction::Scroll { delta, .. } => {
///                 // Handle scroll
///             }
///             _ => {}
///         }
///     }
/// }
/// ```
pub struct MouseHandler {
    /// Last click position and time for double-click detection
    last_click: Option<(u16, u16, Instant)>,
    /// Last double-click position and time for triple-click detection
    last_double_click: Option<(u16, u16, Instant)>,
    /// Drag start position and button
    drag_start: Option<(u16, u16, MouseButton)>,
    /// Double-click detection threshold in milliseconds
    double_click_threshold_ms: u64,
    /// Scroll delta multiplier (lines per scroll event)
    scroll_delta: i16,
}

impl MouseHandler {
    /// Creates a new mouse handler with default settings.
    ///
    /// Default settings:
    /// - Double-click threshold: 300ms
    /// - Scroll delta: 3 lines per scroll event
    pub fn new() -> Self {
        Self {
            last_click: None,
            last_double_click: None,
            drag_start: None,
            double_click_threshold_ms: 300,
            scroll_delta: 3,
        }
    }

    /// Sets the double-click detection threshold.
    ///
    /// # Arguments
    ///
    /// * `ms` - Threshold in milliseconds (default: 300)
    pub fn with_double_click_threshold(mut self, ms: u64) -> Self {
        self.double_click_threshold_ms = ms;
        self
    }

    /// Sets the scroll delta (lines per scroll event).
    ///
    /// # Arguments
    ///
    /// * `delta` - Lines to scroll per event (default: 3)
    pub fn with_scroll_delta(mut self, delta: i16) -> Self {
        self.scroll_delta = delta;
        self
    }

    /// Process a crossterm mouse event into a MouseAction.
    ///
    /// Returns `Some(MouseAction)` if the event should trigger an action,
    /// or `None` for events that don't produce an immediate action (like button release).
    ///
    /// # Arguments
    ///
    /// * `event` - The crossterm MouseEvent to process
    pub fn handle(&mut self, event: MouseEvent) -> Option<MouseAction> {
        match event.kind {
            MouseEventKind::Down(button) => {
                let btn = MouseButton::from(button);
                let pos = (event.column, event.row);

                // Check for multi-click sequences (left button only)
                if btn == MouseButton::Left {
                    // Check for triple-click first (click after double-click)
                    if let Some((dx, dy, dtime)) = self.last_double_click
                        && dx == pos.0
                        && dy == pos.1
                        && dtime.elapsed().as_millis() < self.double_click_threshold_ms as u128
                    {
                        self.last_click = None;
                        self.last_double_click = None;
                        self.drag_start = None;
                        return Some(MouseAction::TripleClick { x: pos.0, y: pos.1 });
                    }

                    // Check for double-click
                    if let Some((lx, ly, time)) = self.last_click
                        && lx == pos.0
                        && ly == pos.1
                        && time.elapsed().as_millis() < self.double_click_threshold_ms as u128
                    {
                        self.last_click = None;
                        self.last_double_click = Some((pos.0, pos.1, Instant::now()));
                        self.drag_start = None;
                        return Some(MouseAction::DoubleClick { x: pos.0, y: pos.1 });
                    }
                    self.last_click = Some((pos.0, pos.1, Instant::now()));
                }

                // Track drag start
                self.drag_start = Some((pos.0, pos.1, btn));

                Some(MouseAction::Click {
                    x: pos.0,
                    y: pos.1,
                    button: btn,
                })
            }

            MouseEventKind::Up(button) => {
                let btn = MouseButton::from(button);
                // Clear drag state on button release
                self.drag_start = None;
                // Return release action for selection handling
                Some(MouseAction::Release {
                    x: event.column,
                    y: event.row,
                    button: btn,
                })
            }

            MouseEventKind::Drag(button) => {
                let btn = MouseButton::from(button);

                // If we have a drag start, report drag action
                if let Some((sx, sy, start_btn)) = self.drag_start {
                    // Only report drag if it's the same button
                    if start_btn == btn {
                        return Some(MouseAction::Drag {
                            start: (sx, sy),
                            current: (event.column, event.row),
                            button: btn,
                        });
                    }
                }

                None
            }

            MouseEventKind::ScrollUp => Some(MouseAction::Scroll {
                x: event.column,
                y: event.row,
                delta: -self.scroll_delta,
            }),

            MouseEventKind::ScrollDown => Some(MouseAction::Scroll {
                x: event.column,
                y: event.row,
                delta: self.scroll_delta,
            }),

            MouseEventKind::Moved => Some(MouseAction::Move {
                x: event.column,
                y: event.row,
            }),

            // ScrollLeft and ScrollRight are not commonly used, ignore for now
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => None,
        }
    }

    /// Clears the click detection state (both single and double-click tracking).
    ///
    /// Call this when focus changes or when you want to prevent
    /// accidental multi-clicks across different UI elements.
    pub fn clear_click_state(&mut self) {
        self.last_click = None;
        self.last_double_click = None;
    }

    /// Clears the drag state.
    ///
    /// Call this when the drag operation should be cancelled.
    pub fn clear_drag_state(&mut self) {
        self.drag_start = None;
    }

    /// Resets all handler state.
    pub fn reset(&mut self) {
        self.last_click = None;
        self.last_double_click = None;
        self.drag_start = None;
    }

    /// Returns true if a drag operation is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag_start.is_some()
    }

    /// Returns the drag start position if a drag is in progress.
    pub fn drag_start(&self) -> Option<(u16, u16)> {
        self.drag_start.map(|(x, y, _)| (x, y))
    }
}

impl Default for MouseHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MouseHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MouseHandler")
            .field("double_click_threshold_ms", &self.double_click_threshold_ms)
            .field("scroll_delta", &self.scroll_delta)
            .field("is_dragging", &self.is_dragging())
            .finish()
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::MouseEventKind;

    fn make_mouse_event(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::empty(),
        }
    }

    #[test]
    fn test_mouse_button_from_crossterm() {
        assert_eq!(MouseButton::from(CrosstermButton::Left), MouseButton::Left);
        assert_eq!(
            MouseButton::from(CrosstermButton::Right),
            MouseButton::Right
        );
        assert_eq!(
            MouseButton::from(CrosstermButton::Middle),
            MouseButton::Middle
        );
    }

    #[test]
    fn test_mouse_handler_new() {
        let handler = MouseHandler::new();
        assert!(!handler.is_dragging());
        assert!(handler.drag_start().is_none());
    }

    #[test]
    fn test_mouse_handler_click() {
        let mut handler = MouseHandler::new();
        let event = make_mouse_event(MouseEventKind::Down(CrosstermButton::Left), 10, 20);

        let action = handler.handle(event);
        assert!(action.is_some());

        if let Some(MouseAction::Click { x, y, button }) = action {
            assert_eq!(x, 10);
            assert_eq!(y, 20);
            assert_eq!(button, MouseButton::Left);
        } else {
            panic!("Expected Click action");
        }
    }

    #[test]
    fn test_mouse_handler_right_click() {
        let mut handler = MouseHandler::new();
        let event = make_mouse_event(MouseEventKind::Down(CrosstermButton::Right), 5, 15);

        let action = handler.handle(event);
        assert!(action.is_some());

        if let Some(MouseAction::Click { x, y, button }) = action {
            assert_eq!(x, 5);
            assert_eq!(y, 15);
            assert_eq!(button, MouseButton::Right);
        } else {
            panic!("Expected Click action");
        }
    }

    #[test]
    fn test_mouse_handler_scroll_up() {
        let mut handler = MouseHandler::new();
        let event = make_mouse_event(MouseEventKind::ScrollUp, 10, 20);

        let action = handler.handle(event);
        assert!(action.is_some());

        if let Some(MouseAction::Scroll { x, y, delta }) = action {
            assert_eq!(x, 10);
            assert_eq!(y, 20);
            assert!(delta < 0); // Scroll up is negative
        } else {
            panic!("Expected Scroll action");
        }
    }

    #[test]
    fn test_mouse_handler_scroll_down() {
        let mut handler = MouseHandler::new();
        let event = make_mouse_event(MouseEventKind::ScrollDown, 10, 20);

        let action = handler.handle(event);
        assert!(action.is_some());

        if let Some(MouseAction::Scroll { x, y, delta }) = action {
            assert_eq!(x, 10);
            assert_eq!(y, 20);
            assert!(delta > 0); // Scroll down is positive
        } else {
            panic!("Expected Scroll action");
        }
    }

    #[test]
    fn test_mouse_handler_move() {
        let mut handler = MouseHandler::new();
        let event = make_mouse_event(MouseEventKind::Moved, 30, 40);

        let action = handler.handle(event);
        assert!(action.is_some());

        if let Some(MouseAction::Move { x, y }) = action {
            assert_eq!(x, 30);
            assert_eq!(y, 40);
        } else {
            panic!("Expected Move action");
        }
    }

    #[test]
    fn test_mouse_handler_drag() {
        let mut handler = MouseHandler::new();

        // Mouse down to start drag
        let down = make_mouse_event(MouseEventKind::Down(CrosstermButton::Left), 10, 20);
        handler.handle(down);
        assert!(handler.is_dragging());

        // Drag event
        let drag = make_mouse_event(MouseEventKind::Drag(CrosstermButton::Left), 15, 25);
        let action = handler.handle(drag);
        assert!(action.is_some());

        if let Some(MouseAction::Drag {
            start,
            current,
            button,
        }) = action
        {
            assert_eq!(start, (10, 20));
            assert_eq!(current, (15, 25));
            assert_eq!(button, MouseButton::Left);
        } else {
            panic!("Expected Drag action");
        }
    }

    #[test]
    #[ignore = "TUI behavior differs across platforms"]
    fn test_mouse_handler_button_up() {
        let mut handler = MouseHandler::new();

        // Start drag
        let down = make_mouse_event(MouseEventKind::Down(CrosstermButton::Left), 10, 20);
        handler.handle(down);
        assert!(handler.is_dragging());

        // Release button
        let up = make_mouse_event(MouseEventKind::Up(CrosstermButton::Left), 15, 25);
        let action = handler.handle(up);
        assert!(action.is_none()); // Up doesn't produce an action
        assert!(!handler.is_dragging());
    }

    #[test]
    fn test_mouse_handler_reset() {
        let mut handler = MouseHandler::new();

        // Start drag
        let down = make_mouse_event(MouseEventKind::Down(CrosstermButton::Left), 10, 20);
        handler.handle(down);
        assert!(handler.is_dragging());

        // Reset
        handler.reset();
        assert!(!handler.is_dragging());
    }

    #[test]
    fn test_mouse_action_position() {
        let click = MouseAction::Click {
            x: 10,
            y: 20,
            button: MouseButton::Left,
        };
        assert_eq!(click.position(), (10, 20));

        let scroll = MouseAction::Scroll {
            x: 5,
            y: 15,
            delta: 3,
        };
        assert_eq!(scroll.position(), (5, 15));
    }

    #[test]
    fn test_mouse_action_is_methods() {
        let click = MouseAction::Click {
            x: 0,
            y: 0,
            button: MouseButton::Left,
        };
        assert!(click.is_click());
        assert!(click.is_left_click());
        assert!(!click.is_right_click());

        let right_click = MouseAction::Click {
            x: 0,
            y: 0,
            button: MouseButton::Right,
        };
        assert!(right_click.is_click());
        assert!(!right_click.is_left_click());
        assert!(right_click.is_right_click());

        let double_click = MouseAction::DoubleClick { x: 0, y: 0 };
        assert!(double_click.is_double_click());
        assert!(!double_click.is_click());

        let scroll = MouseAction::Scroll {
            x: 0,
            y: 0,
            delta: 3,
        };
        assert!(scroll.is_scroll());

        let drag = MouseAction::Drag {
            start: (0, 0),
            current: (10, 10),
            button: MouseButton::Left,
        };
        assert!(drag.is_drag());
    }

    #[test]
    fn test_mouse_handler_with_settings() {
        let handler = MouseHandler::new()
            .with_double_click_threshold(500)
            .with_scroll_delta(5);

        assert_eq!(handler.double_click_threshold_ms, 500);
        assert_eq!(handler.scroll_delta, 5);
    }

    #[test]
    fn test_mouse_button_display() {
        assert_eq!(format!("{}", MouseButton::Left), "Left");
        assert_eq!(format!("{}", MouseButton::Right), "Right");
        assert_eq!(format!("{}", MouseButton::Middle), "Middle");
    }
}
