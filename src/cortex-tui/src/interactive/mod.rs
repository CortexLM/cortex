//! Interactive input system for command selection.
//!
//! This module provides an interactive selection interface that appears
//! in the input area, allowing users to navigate and select options
//! with arrow keys and Enter, keeping the conversation clean.
//!
//! ## Architecture
//!
//! The system consists of:
//! - `InputMode`: Enum distinguishing normal text input from interactive selection
//! - `InteractiveState`: State for the interactive selector (items, selection, search)
//! - `InteractiveWidget`: Ratatui widget for rendering the selector
//! - `handlers`: Key event handling for navigation and selection
//! - `builders`: Functions to create interactive states for various commands
//!
//! ## Usage
//!
//! ```rust,ignore
//! use cortex_tui::interactive::{InputMode, builders};
//!
//! // When user types /provider without args
//! let interactive = builders::build_provider_selector(Some("cortex"));
//! app_state.input_mode = InputMode::Interactive(interactive);
//!
//! // In the render loop, check input_mode and render appropriately
//! match &app_state.input_mode {
//!     InputMode::Normal => render_input_widget(),
//!     InputMode::Interactive(state) => {
//!         let widget = InteractiveWidget::new(state);
//!         widget.render(area, buf);
//!     }
//! }
//!
//! // Handle keys in interactive mode
//! if let InputMode::Interactive(ref mut state) = app_state.input_mode {
//!     match handlers::handle_interactive_key(state, key) {
//!         InteractiveResult::Selected { action, item_id, .. } => {
//!             // Execute the action
//!             app_state.input_mode = InputMode::Normal;
//!         }
//!         InteractiveResult::Cancelled => {
//!             app_state.input_mode = InputMode::Normal;
//!         }
//!         InteractiveResult::Continue => {}
//!     }
//! }
//! ```

pub mod builders;
pub mod handlers;
pub mod renderer;
pub mod state;

pub use handlers::handle_interactive_key;
pub use renderer::InteractiveWidget;
pub use state::{
    InlineFormField, InlineFormState, InputMode, InteractiveAction, InteractiveItem,
    InteractiveResult, InteractiveState,
};
