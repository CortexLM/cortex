//! Core event types for terminal input handling.
//!
//! This module defines the main `Event` enum which represents all possible
//! input events that can occur in a terminal application.

use crate::keyboard::KeyEvent;
use crate::mouse::MouseEvent;
use std::fmt;

/// Represents any input event that can occur in the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A keyboard event (key press, release, or repeat).
    Key(KeyEvent),
    /// A mouse event (click, move, scroll, drag).
    Mouse(MouseEvent),
    /// The terminal was resized to the given width and height.
    Resize(u16, u16),
    /// Text was pasted (via bracketed paste mode).
    Paste(String),
    /// The terminal gained or lost focus.
    Focus(bool),
}

impl Event {
    /// Returns true if this is a key event.
    #[must_use]
    pub fn is_key(&self) -> bool {
        matches!(self, Event::Key(_))
    }

    /// Returns true if this is a mouse event.
    #[must_use]
    pub fn is_mouse(&self) -> bool {
        matches!(self, Event::Mouse(_))
    }

    /// Returns true if this is a resize event.
    #[must_use]
    pub fn is_resize(&self) -> bool {
        matches!(self, Event::Resize(_, _))
    }

    /// Returns true if this is a paste event.
    #[must_use]
    pub fn is_paste(&self) -> bool {
        matches!(self, Event::Paste(_))
    }

    /// Returns true if this is a focus event.
    #[must_use]
    pub fn is_focus(&self) -> bool {
        matches!(self, Event::Focus(_))
    }

    /// Returns the key event if this is a `Key` variant.
    #[must_use]
    pub fn as_key(&self) -> Option<&KeyEvent> {
        match self {
            Event::Key(key) => Some(key),
            _ => None,
        }
    }

    /// Returns the mouse event if this is a `Mouse` variant.
    #[must_use]
    pub fn as_mouse(&self) -> Option<&MouseEvent> {
        match self {
            Event::Mouse(mouse) => Some(mouse),
            _ => None,
        }
    }

    /// Returns the resize dimensions if this is a `Resize` variant.
    #[must_use]
    pub fn as_resize(&self) -> Option<(u16, u16)> {
        match self {
            Event::Resize(w, h) => Some((*w, *h)),
            _ => None,
        }
    }

    /// Returns the pasted text if this is a `Paste` variant.
    #[must_use]
    pub fn as_paste(&self) -> Option<&str> {
        match self {
            Event::Paste(text) => Some(text),
            _ => None,
        }
    }

    /// Returns the focus state if this is a `Focus` variant.
    #[must_use]
    pub fn as_focus(&self) -> Option<bool> {
        match self {
            Event::Focus(focused) => Some(*focused),
            _ => None,
        }
    }

    /// Consumes the event and returns the key event if this is a `Key` variant.
    #[must_use]
    pub fn into_key(self) -> Option<KeyEvent> {
        match self {
            Event::Key(key) => Some(key),
            _ => None,
        }
    }

    /// Consumes the event and returns the mouse event if this is a `Mouse` variant.
    #[must_use]
    pub fn into_mouse(self) -> Option<MouseEvent> {
        match self {
            Event::Mouse(mouse) => Some(mouse),
            _ => None,
        }
    }

    /// Consumes the event and returns the pasted text if this is a `Paste` variant.
    #[must_use]
    pub fn into_paste(self) -> Option<String> {
        match self {
            Event::Paste(text) => Some(text),
            _ => None,
        }
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::Key(key) => write!(f, "Key({key})"),
            Event::Mouse(mouse) => write!(f, "Mouse({mouse})"),
            Event::Resize(w, h) => write!(f, "Resize({w}x{h})"),
            Event::Paste(text) => {
                let preview = if text.len() > 20 {
                    format!("{}...", &text[..20])
                } else {
                    text.clone()
                };
                write!(f, "Paste({preview:?})")
            }
            Event::Focus(focused) => {
                if *focused {
                    write!(f, "Focus(gained)")
                } else {
                    write!(f, "Focus(lost)")
                }
            }
        }
    }
}

impl From<KeyEvent> for Event {
    fn from(event: KeyEvent) -> Self {
        Event::Key(event)
    }
}

impl From<MouseEvent> for Event {
    fn from(event: MouseEvent) -> Self {
        Event::Mouse(event)
    }
}

impl From<crossterm::event::Event> for Event {
    fn from(event: crossterm::event::Event) -> Self {
        match event {
            crossterm::event::Event::Key(key) => Event::Key(key.into()),
            crossterm::event::Event::Mouse(mouse) => Event::Mouse(mouse.into()),
            crossterm::event::Event::Resize(w, h) => Event::Resize(w, h),
            crossterm::event::Event::Paste(text) => Event::Paste(text),
            crossterm::event::Event::FocusGained => Event::Focus(true),
            crossterm::event::Event::FocusLost => Event::Focus(false),
        }
    }
}

/// Trait for types that can be propagated through an event system.
///
/// This trait provides methods to control event propagation and default behavior,
/// similar to DOM events in web browsers.
pub trait PropagatingEvent {
    /// Stops the event from propagating to parent elements.
    fn stop_propagation(&mut self);

    /// Returns true if propagation has been stopped.
    fn is_propagation_stopped(&self) -> bool;

    /// Prevents the default action for this event.
    fn prevent_default(&mut self);

    /// Returns true if the default action has been prevented.
    fn is_default_prevented(&self) -> bool;
}

/// A wrapper that adds propagation control to any event type.
#[derive(Debug, Clone)]
pub struct PropagatedEvent<T> {
    /// The inner event.
    pub event: T,
    /// Whether propagation has been stopped.
    propagation_stopped: bool,
    /// Whether the default action has been prevented.
    default_prevented: bool,
}

impl<T> PropagatedEvent<T> {
    /// Creates a new propagated event wrapper.
    #[must_use]
    pub fn new(event: T) -> Self {
        Self {
            event,
            propagation_stopped: false,
            default_prevented: false,
        }
    }

    /// Consumes the wrapper and returns the inner event.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.event
    }

    /// Returns a reference to the inner event.
    #[must_use]
    pub fn inner(&self) -> &T {
        &self.event
    }

    /// Returns a mutable reference to the inner event.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.event
    }
}

impl<T> PropagatingEvent for PropagatedEvent<T> {
    fn stop_propagation(&mut self) {
        self.propagation_stopped = true;
    }

    fn is_propagation_stopped(&self) -> bool {
        self.propagation_stopped
    }

    fn prevent_default(&mut self) {
        self.default_prevented = true;
    }

    fn is_default_prevented(&self) -> bool {
        self.default_prevented
    }
}

impl<T> std::ops::Deref for PropagatedEvent<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}

impl<T> std::ops::DerefMut for PropagatedEvent<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.event
    }
}

impl<T> From<T> for PropagatedEvent<T> {
    fn from(event: T) -> Self {
        Self::new(event)
    }
}

/// Type alias for a propagatable key event.
pub type PropagatedKeyEvent = PropagatedEvent<KeyEvent>;

/// Type alias for a propagatable mouse event.
pub type PropagatedMouseEvent = PropagatedEvent<MouseEvent>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::{KeyCode, KeyModifiers};
    use crate::mouse::{MouseButton, MouseEventKind};

    #[test]
    fn test_event_predicates() {
        let key_event = Event::Key(KeyEvent::char('a'));
        assert!(key_event.is_key());
        assert!(!key_event.is_mouse());

        let mouse_event = Event::Mouse(MouseEvent::down(MouseButton::Left, 0, 0));
        assert!(mouse_event.is_mouse());
        assert!(!mouse_event.is_key());

        let resize = Event::Resize(80, 24);
        assert!(resize.is_resize());

        let paste = Event::Paste("hello".to_string());
        assert!(paste.is_paste());

        let focus = Event::Focus(true);
        assert!(focus.is_focus());
    }

    #[test]
    fn test_event_accessors() {
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        let event = Event::Key(key.clone());
        assert_eq!(event.as_key(), Some(&key));
        assert!(event.as_mouse().is_none());

        let mouse = MouseEvent::new(MouseEventKind::Moved, 10, 20, KeyModifiers::NONE);
        let event = Event::Mouse(mouse.clone());
        assert_eq!(event.as_mouse(), Some(&mouse));

        let event = Event::Resize(120, 40);
        assert_eq!(event.as_resize(), Some((120, 40)));

        let event = Event::Paste("test".to_string());
        assert_eq!(event.as_paste(), Some("test"));

        let event = Event::Focus(false);
        assert_eq!(event.as_focus(), Some(false));
    }

    #[test]
    fn test_event_into_conversions() {
        let key = KeyEvent::char('z');
        let event = Event::Key(key.clone());
        assert_eq!(event.into_key(), Some(key));

        let mouse = MouseEvent::moved(5, 5);
        let event = Event::Mouse(mouse.clone());
        assert_eq!(event.into_mouse(), Some(mouse));

        let event = Event::Paste("data".to_string());
        assert_eq!(event.into_paste(), Some("data".to_string()));
    }

    #[test]
    fn test_event_display() {
        let event = Event::Resize(80, 24);
        assert_eq!(format!("{event}"), "Resize(80x24)");

        let event = Event::Focus(true);
        assert_eq!(format!("{event}"), "Focus(gained)");

        let event = Event::Focus(false);
        assert_eq!(format!("{event}"), "Focus(lost)");
    }

    #[test]
    fn test_propagated_event() {
        let key = KeyEvent::char('a');
        let mut propagated = PropagatedEvent::new(key);

        assert!(!propagated.is_propagation_stopped());
        assert!(!propagated.is_default_prevented());

        propagated.stop_propagation();
        assert!(propagated.is_propagation_stopped());

        propagated.prevent_default();
        assert!(propagated.is_default_prevented());
    }

    #[test]
    fn test_propagated_event_deref() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let propagated = PropagatedEvent::new(key);

        // Should be able to access KeyEvent methods through Deref
        assert_eq!(propagated.code, KeyCode::Enter);
        assert!(!propagated.ctrl());
    }
}
