//! Cortex TUI Framework: A high-performance terminal UI framework for Rust
//!
//! > **DEPRECATED**: This crate is deprecated. The Cortex CLI has migrated to
//! > [`cortex-tui`](../cortex_tui) which uses `ratatui` + `crossterm` directly for
//! > a simpler, more maintainable architecture. This crate will be removed in a
//! > future release.
//!
//! This crate provides a complete terminal UI framework with:
//! - Flexbox-based layout system
//! - Double-buffered rendering at 60 FPS
//! - Full Unicode and grapheme cluster support
//! - Syntax highlighting via tree-sitter
//! - Comprehensive input handling (keyboard, mouse)
//!
//! # Example
//!
//! ```ignore
//! use cortex_tui_framework::prelude::*;
//! use cortex_tui_framework::terminal::CrosstermBackend;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let backend = CrosstermBackend::new(std::io::stdout())?;
//!     let mut app = Application::new(backend)?;
//!     
//!     let text = TextWidget::builder()
//!         .text("Hello, Cortex TUI!")
//!         .build();
//!     
//!     let root = BoxWidget::builder()
//!         .direction(FlexDirection::Column)
//!         .child(text)
//!         .build();
//!     
//!     app.run()?;
//!     Ok(())
//! }
//! ```

pub use cortex_tui_buffer as buffer;
pub use cortex_tui_core as core;
pub use cortex_tui_input as input;
pub use cortex_tui_layout as layout;
pub use cortex_tui_syntax as syntax;
pub use cortex_tui_terminal as terminal;
pub use cortex_tui_text as text;
pub use cortex_tui_widgets as widgets;

pub mod prelude {
    pub use cortex_tui_core::{Color, Point, Rect, Size, Style, TextAttributes};
    pub use cortex_tui_input::{Event, KeyCode, KeyModifiers, MouseButton, MouseEvent};
    pub use cortex_tui_layout::{
        AlignContent, AlignItems, FlexDirection, FlexWrap, JustifyContent,
    };
    pub use cortex_tui_terminal::Application;
    pub use cortex_tui_widgets::{BoxWidget, Input, ScrollBox, TextWidget, Widget};
}
