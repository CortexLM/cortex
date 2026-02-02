//! Chat message display widget with streaming support.
//!
//! Provides widgets for rendering chat conversations with support for
//! multiple message roles, streaming text with typewriter animation,
//! and basic markdown-lite rendering.
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_engine::widgets::{ChatWidget, Message, MessageRole};
//! use cortex_engine::animation::Typewriter;
//!
//! let messages = vec![
//!     Message::user("Hello!"),
//!     Message::assistant("Hi there! How can I help?").streaming(),
//! ];
//!
//! let typewriter = Typewriter::new("Hi there! How can I help?".to_string(), 60.0);
//! let chat = ChatWidget::new(&messages)
//!     .typewriter(&typewriter)
//!     .show_timestamps(true);
//! ```

mod message_cell;
mod parsing;
mod types;
mod widget;
mod wrapping;

// Re-export public types and widgets
pub use message_cell::MessageCell;
pub use types::{Message, MessageRole, StyledSegment};
pub use widget::{ChatWidget, extract_selected_text};

// Re-export utility functions for internal use
pub use parsing::parse_markdown_lite;
pub use wrapping::{split_at_char_boundary, wrap_text};
