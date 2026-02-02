//! Core widget trait and related types.
//!
//! This module defines the `Widget` trait that all widgets implement,
//! along with helper types for widget management.

use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::buffer::Buffer;
use crate::event::{Event, EventResult};
use crate::layout::LayoutStyle;
use crate::types::Rect;

/// Unique identifier for widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WidgetId(u64);

impl WidgetId {
    /// Generates a new unique widget ID.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Returns the raw ID value.
    pub const fn raw(&self) -> u64 {
        self.0
    }
}

impl Default for WidgetId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WidgetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Widget({})", self.0)
    }
}

/// A reference to a child widget, either owned or borrowed.
pub type WidgetRef = Box<dyn Widget>;

/// The core widget trait that all widgets must implement.
///
/// Widgets are the building blocks of the UI. Each widget handles:
/// - Layout: Defining its size and positioning constraints
/// - Rendering: Drawing itself to a buffer
/// - Events: Responding to user input
/// - Children: Managing child widgets (for containers)
pub trait Widget: Any + Send {
    /// Returns the widget's unique identifier.
    fn id(&self) -> WidgetId;

    /// Returns a static string identifying the widget type.
    fn type_name(&self) -> &'static str;

    /// Returns the layout style for this widget.
    fn layout(&self) -> &LayoutStyle;

    /// Returns a mutable reference to the layout style.
    fn layout_mut(&mut self) -> &mut LayoutStyle;

    /// Renders the widget to the buffer within the given rectangle.
    ///
    /// The `rect` parameter defines the area allocated to this widget
    /// by the layout system. The widget should render itself within
    /// these bounds.
    fn render(&self, buffer: &mut Buffer, rect: Rect);

    /// Handles an event and returns whether it was consumed.
    ///
    /// Return `EventResult::Handled` if the event was consumed and
    /// should not propagate to other widgets.
    fn handle_event(&mut self, event: &Event) -> EventResult;

    /// Returns a slice of this widget's children.
    ///
    /// For leaf widgets (no children), return an empty slice.
    fn children(&self) -> &[WidgetRef];

    /// Returns a mutable slice of this widget's children.
    fn children_mut(&mut self) -> &mut [WidgetRef];

    /// Returns whether this widget can receive focus.
    fn is_focusable(&self) -> bool {
        false
    }

    /// Called when the widget receives focus.
    fn on_focus(&mut self) {}

    /// Called when the widget loses focus.
    fn on_blur(&mut self) {}

    /// Returns whether this widget is visible.
    fn is_visible(&self) -> bool {
        true
    }

    /// Casts this widget to `Any` for downcasting.
    fn as_any(&self) -> &dyn Any;

    /// Casts this widget to `Any` for mutable downcasting.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Measures the widget's preferred size given available space.
    ///
    /// This is used by the layout system to determine the widget's
    /// intrinsic size when `width` or `height` is set to `Auto`.
    fn measure(&self, available_width: f32, available_height: f32) -> (f32, f32) {
        let _ = (available_width, available_height);
        (0.0, 0.0)
    }

    /// Called when the widget is mounted to the tree.
    fn on_mount(&mut self) {}

    /// Called when the widget is unmounted from the tree.
    fn on_unmount(&mut self) {}
}

/// Helper trait for downcasting widgets.
pub trait WidgetExt: Widget {
    /// Attempts to downcast this widget to a concrete type.
    fn downcast_ref<T: Widget + 'static>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }

    /// Attempts to downcast this widget to a concrete mutable type.
    fn downcast_mut<T: Widget + 'static>(&mut self) -> Option<&mut T> {
        self.as_any_mut().downcast_mut::<T>()
    }
}

impl<W: Widget + ?Sized> WidgetExt for W {}

/// A boxed widget that can contain any widget type.
pub type BoxedWidget = Box<dyn Widget>;

/// Creates a boxed widget from any widget type.
pub fn boxed<W: Widget + 'static>(widget: W) -> BoxedWidget {
    Box::new(widget)
}

/// Iterator over child widgets.
pub struct ChildIter<'a> {
    inner: std::slice::Iter<'a, WidgetRef>,
}

impl<'a> Iterator for ChildIter<'a> {
    type Item = &'a dyn Widget;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|w| w.as_ref())
    }
}

/// Mutable iterator over child widgets.
pub struct ChildIterMut<'a> {
    inner: std::slice::IterMut<'a, WidgetRef>,
}

impl<'a> Iterator for ChildIterMut<'a> {
    type Item = &'a mut dyn Widget;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|w| w.as_mut())
    }
}

/// Extension methods for working with widget children.
pub trait WidgetChildrenExt: Widget {
    /// Returns an iterator over child widgets.
    fn iter_children(&self) -> ChildIter<'_> {
        ChildIter {
            inner: self.children().iter(),
        }
    }

    /// Returns a mutable iterator over child widgets.
    fn iter_children_mut(&mut self) -> ChildIterMut<'_> {
        ChildIterMut {
            inner: self.children_mut().iter_mut(),
        }
    }

    /// Returns the number of children.
    fn child_count(&self) -> usize {
        self.children().len()
    }

    /// Returns whether this widget has any children.
    fn has_children(&self) -> bool {
        !self.children().is_empty()
    }
}

impl<W: Widget + ?Sized> WidgetChildrenExt for W {}

/// A no-op widget that renders nothing.
/// Useful as a placeholder or for testing.
#[derive(Debug)]
pub struct EmptyWidget {
    id: WidgetId,
    layout: LayoutStyle,
}

impl EmptyWidget {
    /// Creates a new empty widget.
    pub fn new() -> Self {
        Self {
            id: WidgetId::new(),
            layout: LayoutStyle::default(),
        }
    }
}

impl Default for EmptyWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for EmptyWidget {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn type_name(&self) -> &'static str {
        "Empty"
    }

    fn layout(&self) -> &LayoutStyle {
        &self.layout
    }

    fn layout_mut(&mut self) -> &mut LayoutStyle {
        &mut self.layout
    }

    fn render(&self, _buffer: &mut Buffer, _rect: Rect) {
        // Nothing to render
    }

    fn handle_event(&mut self, _event: &Event) -> EventResult {
        EventResult::Ignored
    }

    fn children(&self) -> &[WidgetRef] {
        &[]
    }

    fn children_mut(&mut self) -> &mut [WidgetRef] {
        &mut []
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_widget_id_uniqueness() {
        let id1 = WidgetId::new();
        let id2 = WidgetId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_empty_widget() {
        let widget = EmptyWidget::new();
        assert_eq!(widget.type_name(), "Empty");
        assert!(!widget.has_children());
        assert!(!widget.is_focusable());
    }

    #[test]
    fn test_widget_downcast() {
        let widget: BoxedWidget = Box::new(EmptyWidget::new());
        assert!(widget.downcast_ref::<EmptyWidget>().is_some());
    }
}
