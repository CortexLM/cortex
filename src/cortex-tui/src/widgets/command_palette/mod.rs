//! Command Palette Widget
//!
//! A VS Code-style command palette (Ctrl+P) for quick access to commands,
//! files, and sessions with fuzzy search.
//!
//! ## Usage
//!
//! ```ignore
//! use cortex_tui::widgets::{CommandPalette, CommandPaletteState};
//!
//! let mut state = CommandPaletteState::new();
//! state.load_commands(&registry);
//!
//! // In render loop:
//! let widget = CommandPalette::new(&state);
//! frame.render_widget(widget, area);
//! ```

mod fuzzy;
mod state;
mod types;
mod widget;

// Re-export all public types for backwards compatibility
pub use state::CommandPaletteState;
pub use types::{PaletteItem, RecentType};
pub use widget::CommandPalette;
