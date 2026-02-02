//! Help browser widget for displaying documentation.
//!
//! Provides a modal help browser with sidebar navigation and searchable content.
//! Displays keyboard shortcuts, commands, model info, and other documentation.
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_tui::widgets::{HelpBrowser, HelpBrowserState};
//!
//! let mut state = HelpBrowserState::new();
//!
//! // Navigate with keyboard
//! state.select_next();
//! state.toggle_focus();
//! state.scroll_down();
//!
//! // Render
//! let widget = HelpBrowser::new(&state);
//! widget.render(area, buf);
//! ```

mod content;
mod render;
mod state;
#[cfg(test)]
mod tests;
mod utils;

// Re-export all public types for backwards compatibility
pub use content::{HelpContent, HelpSection, get_help_sections};
pub use render::HelpBrowser;
pub use state::{HelpBrowserState, HelpFocus};
pub use utils::wrap_text;
