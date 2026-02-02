//! Core Component trait and types.
//!
//! All interactive TUI components should implement the `Component` trait,
//! which provides a consistent interface for rendering, input handling,
//! and accessibility features.

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// Result of handling a key event in a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentResult<T = ()> {
    /// Component handled the event, continue displaying
    Handled,
    /// Component did not handle the event, propagate to parent
    NotHandled,
    /// Component completed with a value
    Done(T),
    /// Component was cancelled (e.g., Escape pressed)
    Cancelled,
}

impl<T> ComponentResult<T> {
    /// Returns true if the component handled the event.
    pub fn is_handled(&self) -> bool {
        matches!(self, ComponentResult::Handled | ComponentResult::Done(_))
    }

    /// Returns true if the component is done (completed or cancelled).
    pub fn is_done(&self) -> bool {
        matches!(self, ComponentResult::Done(_) | ComponentResult::Cancelled)
    }

    /// Maps the inner value if Done.
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> ComponentResult<U> {
        match self {
            ComponentResult::Handled => ComponentResult::Handled,
            ComponentResult::NotHandled => ComponentResult::NotHandled,
            ComponentResult::Done(v) => ComponentResult::Done(f(v)),
            ComponentResult::Cancelled => ComponentResult::Cancelled,
        }
    }
}

/// Focus state of a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusState {
    /// Component is not focused
    #[default]
    Unfocused,
    /// Component has focus
    Focused,
    /// Component has focus and is in edit/input mode
    Editing,
}

impl FocusState {
    /// Returns true if the component has any form of focus.
    pub fn has_focus(&self) -> bool {
        !matches!(self, FocusState::Unfocused)
    }

    /// Returns true if the component is in editing mode.
    pub fn is_editing(&self) -> bool {
        matches!(self, FocusState::Editing)
    }
}

/// Core trait for all TUI components.
///
/// This trait defines the standard interface that all interactive components
/// must implement, ensuring consistent behavior across the TUI.
///
/// # Example Implementation
///
/// ```rust,ignore
/// use cortex_tui_components::component::{Component, ComponentResult, FocusState};
/// use crossterm::event::{KeyCode, KeyEvent};
/// use ratatui::{buffer::Buffer, layout::Rect};
///
/// struct MyButton {
///     label: String,
///     focused: bool,
/// }
///
/// impl Component for MyButton {
///     type Output = ();
///
///     fn render(&self, area: Rect, buf: &mut Buffer) {
///         // Render button with appropriate styling based on focus
///     }
///
///     fn handle_key(&mut self, key: KeyEvent) -> ComponentResult<Self::Output> {
///         match key.code {
///             KeyCode::Enter => ComponentResult::Done(()),
///             KeyCode::Esc => ComponentResult::Cancelled,
///             _ => ComponentResult::NotHandled,
///         }
///     }
///
///     fn focus_state(&self) -> FocusState {
///         if self.focused { FocusState::Focused } else { FocusState::Unfocused }
///     }
///
///     fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
///         vec![("Enter", "Activate"), ("Esc", "Cancel")]
///     }
/// }
/// ```
pub trait Component {
    /// The type of value this component produces when completed.
    type Output;

    /// Render the component to the buffer.
    ///
    /// # Arguments
    /// * `area` - The area to render into
    /// * `buf` - The buffer to render to
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Handle a key event.
    ///
    /// # Arguments
    /// * `key` - The key event to handle
    ///
    /// # Returns
    /// A `ComponentResult` indicating what happened.
    fn handle_key(&mut self, key: KeyEvent) -> ComponentResult<Self::Output>;

    /// Returns the current focus state of the component.
    fn focus_state(&self) -> FocusState;

    /// Set the focus state of the component.
    fn set_focus(&mut self, focused: bool);

    /// Returns key hints to display for this component.
    ///
    /// Each tuple is (key_label, description).
    /// Example: `[("↑↓", "Navigate"), ("Enter", "Select")]`
    fn key_hints(&self) -> Vec<(&'static str, &'static str)>;

    /// Handle pasted text. Returns true if handled.
    ///
    /// Default implementation does nothing.
    fn handle_paste(&mut self, _text: &str) -> bool {
        false
    }

    /// Called when the component is about to be shown.
    ///
    /// Use this to initialize state or start animations.
    fn on_show(&mut self) {}

    /// Called when the component is about to be hidden.
    ///
    /// Use this to clean up state or stop animations.
    fn on_hide(&mut self) {}

    /// Returns the desired size of the component.
    ///
    /// Returns (min_width, min_height, max_width, max_height).
    /// Use `u16::MAX` for unbounded dimensions.
    fn desired_size(&self) -> (u16, u16, u16, u16) {
        (0, 0, u16::MAX, u16::MAX)
    }

    /// Returns true if the component can be focused.
    ///
    /// Components that are display-only should return false.
    fn can_focus(&self) -> bool {
        true
    }
}

// Note: We cannot implement Widget for &C where C: Component due to orphan rules.
// Instead, each component should implement Widget directly or provide a .widget() method.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_result_is_handled() {
        assert!(ComponentResult::<()>::Handled.is_handled());
        assert!(ComponentResult::Done(42).is_handled());
        assert!(!ComponentResult::<()>::NotHandled.is_handled());
        assert!(!ComponentResult::<()>::Cancelled.is_handled());
    }

    #[test]
    fn test_component_result_is_done() {
        assert!(!ComponentResult::<()>::Handled.is_done());
        assert!(ComponentResult::Done(42).is_done());
        assert!(!ComponentResult::<()>::NotHandled.is_done());
        assert!(ComponentResult::<()>::Cancelled.is_done());
    }

    #[test]
    fn test_component_result_map() {
        let result: ComponentResult<i32> = ComponentResult::Done(42);
        let mapped = result.map(|x| x * 2);
        assert_eq!(mapped, ComponentResult::Done(84));

        let handled: ComponentResult<i32> = ComponentResult::Handled;
        let mapped_handled = handled.map(|x| x * 2);
        assert_eq!(mapped_handled, ComponentResult::Handled);
    }

    #[test]
    fn test_focus_state() {
        assert!(!FocusState::Unfocused.has_focus());
        assert!(FocusState::Focused.has_focus());
        assert!(FocusState::Editing.has_focus());

        assert!(!FocusState::Unfocused.is_editing());
        assert!(!FocusState::Focused.is_editing());
        assert!(FocusState::Editing.is_editing());
    }
}
