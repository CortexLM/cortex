//! Terminal backend for Cortex TUI.
//!
//! This crate provides the terminal abstraction layer for Cortex TUI, including:
//! - [`TerminalBackend`] trait for terminal operations
//! - [`CrosstermBackend`] implementation using crossterm
//! - [`Renderer`] for efficient double-buffered rendering
//! - [`Application`] for managing the main event loop
//! - [`Capabilities`] for terminal capability detection
//!
//! # Example
//!
//! ```no_run
//! use cortex_tui_terminal::{Application, CrosstermBackend, Style};
//!
//! fn main() -> cortex_tui_core::Result<()> {
//!     let backend = CrosstermBackend::new()?;
//!     let mut app = Application::new(backend)?;
//!     
//!     app.run(|buffer, _dt| {
//!         // Render your UI to the buffer
//!         buffer.draw_str(0, 0, "Hello, Cortex TUI!", Style::default());
//!         true // Return false to exit
//!     })?;
//!     
//!     Ok(())
//! }
//! ```

mod application;
mod backend;
mod capabilities;
mod renderer;

pub use application::{AppState, Application, ApplicationBuilder};
pub use backend::{CrosstermBackend, CursorStyle, TerminalBackend};
pub use capabilities::{Capabilities, ColorMode, UnicodeMode};
pub use renderer::{FramePacer, Renderer};

/// Re-export core types for convenience.
pub use cortex_tui_core::{Color, Error, Point, Rect, Result, Size, Style, TextAttributes};

/// Re-export buffer types for convenience.
pub use cortex_tui_buffer::{Buffer, BufferDiff, Cell, CellChange, DiffOptions, DoubleBuffer};

/// Re-export input types for convenience.
pub use cortex_tui_input::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
