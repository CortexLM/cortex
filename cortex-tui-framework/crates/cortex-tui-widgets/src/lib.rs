//! Cortex TUI Widgets Library
//!
//! This crate provides the core widgets for building terminal user interfaces
//! with Cortex TUI. It includes fundamental building blocks like containers and
//! text displays, along with the `Widget` trait that all widgets implement.

// Allow complex types and manual div_ceil in this crate
#![allow(clippy::type_complexity)]
#![allow(clippy::manual_div_ceil)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::box_default)]
#![allow(dead_code)]
#![allow(unused_mut)]
//!
//! # Overview
//!
//! The widget system is built around several core concepts:
//!
//! - **Widget trait**: The core abstraction that all UI components implement.
//!   Widgets handle layout, rendering, and event handling.
//!
//! - **Buffer**: A 2D grid of cells representing terminal content. Widgets
//!   render themselves to a buffer.
//!
//! - **Layout**: Flexbox-style layout system for positioning widgets.
//!
//! - **Events**: Input events (keyboard, mouse) that widgets can respond to.
//!
//! # Core Widgets
//!
//! - [`BoxWidget`]: A container widget with optional border and background.
//!   Use it to group and organize other widgets.
//!
//! - [`TextWidget`]: Displays styled text with alignment, wrapping, and
//!   truncation support.
//!
//! # Example
//!
//! ```ignore
//! use cortex_tui_widgets::prelude::*;
//!
//! // Create a bordered container with some text
//! let container = BoxWidget::builder()
//!     .border_rounded()
//!     .padding(1)
//!     .title("Hello")
//!     .child(TextWidget::builder()
//!         .text("Welcome to Cortex TUI!")
//!         .center()
//!         .bold()
//!         .build())
//!     .build();
//! ```
//!
//! # Module Structure
//!
//! - [`types`]: Core types like `Rect`, `RGBA`, `Style`, `Cell`
//! - [`buffer`]: Terminal buffer for rendering
//! - [`event`]: Event types and handling
//! - [`layout`]: Layout system types
//! - [`widget`]: The `Widget` trait and related types
//! - [`border`]: Border styles and rendering
//! - [`box_widget`]: Box container widget
//! - [`text_widget`]: Text display widget

// Core modules
pub mod buffer;
pub mod event;
pub mod layout;
pub mod types;

// Widget system
pub mod border;
pub mod widget;

// Widgets
pub mod box_widget;
pub mod text_widget;

// Text editing
pub mod cursor;
pub mod input;
pub mod textarea;

// Scrolling system
pub mod scrollbar;
pub mod scrollbox;
pub mod viewport;

// Re-exports for convenience
pub use border::{
    draw_border, inner_rect, BorderBuilder, BorderChars, BorderSides, BorderStyle,
    DrawBorderParams, TitleAlignment,
};
pub use box_widget::{
    bordered_box, double_box, rounded_box, single_box, BoxWidget, BoxWidgetBuilder,
};
pub use buffer::{Buffer, BufferExt, ClipGuard};
pub use event::{
    Event, EventResult, FocusEvent, KeyCode, KeyEvent, Modifiers, MouseButton, MouseEvent,
    MouseEventKind, PasteEvent, ResizeEvent,
};
pub use layout::{
    AlignItems, AlignSelf, Dimension, EdgeDimensions, FlexDirection, FlexWrap, JustifyContent,
    LayoutStyle, Overflow, PositionType,
};
pub use text_widget::{
    bold_text, centered_text, text, TextAlign, TextSpan, TextWidget, TextWidgetBuilder, Truncation,
};
pub use types::{Cell, Color, Edges, Rect, Style, RGBA};
pub use widget::{
    boxed, BoxedWidget, EmptyWidget, Widget, WidgetChildrenExt, WidgetExt, WidgetId, WidgetRef,
};

// Cursor and text editing re-exports
pub use cursor::{
    byte_to_grapheme_offset, grapheme_to_byte_offset, CursorMove, CursorPosition, LineCursor,
    Selection, TextCursor,
};
pub use input::{
    Input, InputBuilder, InputChange, InputKey, InputModifiers, InputStyle, VisibleText,
};
pub use textarea::{
    CursorInfo, TextArea, TextAreaBuilder, TextAreaChange, TextAreaKey, TextAreaModifiers,
    TextAreaStyle, VisibleLine, VisibleLines, WrapMode as TextAreaWrapMode,
};

// Scrolling system re-exports
pub use scrollbar::{
    Orientation, Scrollbar, ScrollbarBuilder, ScrollbarCell, ScrollbarMetrics, ScrollbarStyle,
    ScrollbarVisibility,
};
pub use scrollbox::{
    ScrollAccelConfig, ScrollBox, ScrollBoxBuilder, ScrollBoxConfig, ScrollDirection, ScrollUnit,
    StickyPosition, StickyState,
};
pub use viewport::{Rect as ViewportRect, ScrollOffset, Viewport, ViewportBuilder};

/// Prelude module for convenient imports.
///
/// Use `use cortex_tui_widgets::prelude::*;` to import commonly used types.
pub mod prelude {
    // Types
    pub use crate::types::{Cell, Color, Edges, Rect, Style, RGBA};

    // Buffer
    pub use crate::buffer::{Buffer, BufferExt, ClipGuard};

    // Events
    pub use crate::event::{
        Event, EventResult, KeyCode, KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    // Layout
    pub use crate::layout::{
        AlignItems, Dimension, FlexDirection, JustifyContent, LayoutStyle, Overflow,
    };

    // Widget trait and types
    pub use crate::widget::{
        boxed, BoxedWidget, Widget, WidgetChildrenExt, WidgetExt, WidgetId, WidgetRef,
    };

    // Border
    pub use crate::border::{BorderBuilder, BorderChars, BorderSides, BorderStyle, TitleAlignment};

    // Widgets
    pub use crate::box_widget::{
        bordered_box, double_box, rounded_box, single_box, BoxWidget, BoxWidgetBuilder,
    };
    pub use crate::text_widget::{
        bold_text, centered_text, text, TextAlign, TextSpan, TextWidget, TextWidgetBuilder,
        Truncation, WrapMode,
    };
}

#[cfg(test)]
mod tests {
    use super::prelude::*;

    #[test]
    fn test_prelude_imports() {
        // Verify that prelude imports work
        let _rect = Rect::new(0, 0, 10, 10);
        let _style = Style::new();
        let _buffer = Buffer::new(10, 10);
        let _event_result = EventResult::Ignored;
        let _layout = LayoutStyle::new();
        let _border_style = BorderStyle::Single;
        let _box_widget = BoxWidget::new();
        let _text_widget = TextWidget::new();
    }

    #[test]
    fn test_widget_composition() {
        // Test building a composed widget tree
        let container = BoxWidget::builder()
            .border_rounded()
            .padding(1)
            .title("Test Container")
            .column()
            .child(TextWidget::builder().text("Header").center().bold().build())
            .child(
                TextWidget::builder()
                    .text("Content goes here")
                    .wrap_word()
                    .build(),
            )
            .build();

        assert_eq!(container.type_name(), "Box");
        assert_eq!(container.children().len(), 2);
        assert_eq!(container.border_style(), BorderStyle::Rounded);
        assert_eq!(container.title(), Some("Test Container"));
    }

    #[test]
    fn test_render_composed_widget() {
        let container = BoxWidget::builder().border_single().build();

        let mut buffer = Buffer::new(20, 5);
        container.render(&mut buffer, Rect::new(0, 0, 20, 5));

        // Verify border was rendered
        assert_eq!(buffer.get(0, 0).unwrap().character, '┌');
        assert_eq!(buffer.get(19, 0).unwrap().character, '┐');
        assert_eq!(buffer.get(0, 4).unwrap().character, '└');
        assert_eq!(buffer.get(19, 4).unwrap().character, '┘');
    }

    #[test]
    fn test_text_in_box() {
        let text_widget = TextWidget::with_text("Hello");
        let mut container = BoxWidget::new();
        container.add_child(Box::new(text_widget));

        assert_eq!(container.children().len(), 1);
    }

    #[test]
    fn test_event_handling() {
        let mut widget = BoxWidget::new();
        let event = Event::Key(KeyEvent::char('a'));

        // Box doesn't handle events by default
        let result = widget.handle_event(&event);
        assert!(result.is_ignored());
    }

    #[test]
    fn test_downcasting() {
        let widget: BoxedWidget = Box::new(BoxWidget::new());

        // Downcast to concrete type
        assert!(widget.downcast_ref::<BoxWidget>().is_some());
        assert!(widget.downcast_ref::<TextWidget>().is_none());
    }
}
