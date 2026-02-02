//! Minimalist Session View
//!
//! A terminal-style chat interface for conversations.
//! This view provides a clean, minimal UI with:
//! - Chat history as simple terminal scrollback
//! - Status indicator with shimmer animation
//! - Simple input line with prompt
//! - Contextual key hints at the bottom

mod layout;
mod rendering;
mod text_utils;
mod view;

#[cfg(test)]
mod tests;

/// Application version
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

// Re-export main types for backwards compatibility
pub use view::{ChatMessage, MinimalSessionView};
