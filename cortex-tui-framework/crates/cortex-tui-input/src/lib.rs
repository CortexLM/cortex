#![allow(clippy::iter_without_into_iter, dead_code)]
//! # `Cortex TUI` Input
//!
//! Input handling for terminal UI applications.
//!
//! This crate provides a complete input handling system for terminal applications,
//! including keyboard events, mouse events, focus management, and event reading.
//!
//! ## Features
//!
//! - **Keyboard Input**: Full keyboard event support including modifiers, function keys,
//!   and special keys. Compatible with the Kitty keyboard protocol for advanced features.
//!
//! - **Mouse Input**: Mouse button clicks, movement, drag operations, and scrolling.
//!   Supports both SGR and X10 mouse protocols.
//!
//! - **Focus Management**: A focus tree system for managing which UI element has focus,
//!   with tab navigation and focus scoping for modals.
//!
//! - **Event Reader**: High-level API for reading terminal events with support for
//!   blocking and non-blocking modes.
//!
//! ## Quick Start
//!
//! ```no_run
//! use cortex_tui_input::{Event, InputReader, InputReaderConfig};
//! use std::time::Duration;
//!
//! // Create and initialize the input reader
//! let mut reader = InputReader::new(InputReaderConfig::default());
//! reader.init().expect("Failed to initialize input reader");
//!
//! // Event loop
//! loop {
//!     if let Some(event) = reader.poll(Duration::from_millis(100)).expect("Poll failed") {
//!         match event {
//!             Event::Key(key) => {
//!                 println!("Key pressed: {}", key);
//!                 // Exit on Ctrl+C
//!                 if key.ctrl() && key.code == cortex_tui_input::KeyCode::Char('c') {
//!                     break;
//!                 }
//!             }
//!             Event::Mouse(mouse) => println!("Mouse event: {}", mouse),
//!             Event::Resize(w, h) => println!("Terminal resized to {}x{}", w, h),
//!             Event::Paste(text) => println!("Pasted: {}", text),
//!             Event::Focus(focused) => println!("Focus: {}", focused),
//!         }
//!     }
//! }
//!
//! // Cleanup (also happens automatically on drop)
//! reader.cleanup().expect("Failed to cleanup");
//! ```
//!
//! ## Handling Key Events
//!
//! ```no_run
//! use cortex_tui_input::{KeyEvent, KeyCode, KeyModifiers};
//!
//! fn handle_key(event: &KeyEvent) {
//!     // Check for specific key combinations
//!     if event.matches(KeyCode::Char('s'), KeyModifiers::CONTROL) {
//!         println!("Save command!");
//!     }
//!
//!     // Check modifiers individually
//!     if event.ctrl() && event.shift() {
//!         println!("Ctrl+Shift combination");
//!     }
//!
//!     // Get a descriptive string for the key
//!     println!("Shortcut: {}", event.to_shortcut_string());
//! }
//! ```
//!
//! ## Focus Management
//!
//! ```
//! use cortex_tui_input::focus::{FocusManager, FocusConfig};
//!
//! let mut focus = FocusManager::new();
//!
//! // Register focusable elements
//! let button1 = focus.generate_id();
//! let button2 = focus.generate_id();
//! focus.register(button1, None, true, 0);
//! focus.register(button2, None, true, 0);
//!
//! // Navigate with Tab
//! focus.focus_next(); // Focus button1
//! focus.focus_next(); // Focus button2
//! focus.focus_previous(); // Back to button1
//! ```

#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::match_same_arms)]

pub mod event;
pub mod focus;
pub mod keyboard;
pub mod mouse;
pub mod reader;

// Re-export main types at crate root for convenience
pub use event::{
    Event, PropagatedEvent, PropagatedKeyEvent, PropagatedMouseEvent, PropagatingEvent,
};
pub use focus::{
    FocusConfig, FocusDirection, FocusEvent, FocusId, FocusManager, Focusable, GridFocusNavigator,
};
pub use keyboard::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
pub use mouse::{MouseButton, MouseEvent, MouseEventKind, MouseState, ScrollDirection};
pub use reader::{
    InputError, InputReader, InputReaderConfig, InputResult, MouseCaptureGuard, RawModeGuard,
};

/// Prelude module for convenient imports.
///
/// ```
/// use cortex_tui_input::prelude::*;
/// ```
pub mod prelude {
    pub use crate::event::{Event, PropagatedEvent, PropagatingEvent};
    pub use crate::focus::{FocusDirection, FocusEvent, FocusId, FocusManager, Focusable};
    pub use crate::keyboard::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    pub use crate::mouse::{MouseButton, MouseEvent, MouseEventKind, MouseState, ScrollDirection};
    pub use crate::reader::{InputReader, InputReaderConfig, InputResult};
}
