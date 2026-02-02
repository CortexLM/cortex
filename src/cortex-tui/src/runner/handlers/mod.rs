//! Action handlers that connect KeyActions to application logic.
//!
//! This module provides the implementation for each KeyAction, separated from
//! the event loop for better organization and testability.
//!
//! ## Architecture
//!
//! ```text
//! KeyEvent -> ActionMapper -> KeyAction -> ActionHandler -> AppState mutation
//! ```
//!
//! The `ActionHandler` receives a `KeyAction` and executes the corresponding
//! logic against the application state, optionally interacting with the
//! session bridge for backend communication.
//!
//! ## Example
//!
//! ```rust,ignore
//! let mut handler = ActionHandler::new(&mut state, session.as_ref(), &mut stream);
//!
//! match handler.handle(KeyAction::Submit).await {
//!     Ok(true) => println!("Action consumed"),
//!     Ok(false) => println!("Action not handled"),
//!     Err(e) => eprintln!("Error: {}", e),
//! }
//! ```

mod approval;
mod command;
mod context;
mod core;
mod dispatch;
mod edit;
mod focus;
mod input;
mod model;
mod operation_mode;
mod scroll;
mod session;
mod tool;
mod view;

#[cfg(test)]
mod tests;

use crate::app::AppState;
use crate::bridge::{SessionBridge, StreamController};

// ============================================================================
// ACTION HANDLER
// ============================================================================

/// Handles KeyAction execution against application state.
///
/// This struct separates action handling logic from the event loop for better
/// testability and organization. It holds mutable references to the application
/// state and stream controller, and an optional reference to the session bridge
/// for backend communication.
///
/// # Lifetime
///
/// The handler borrows all its dependencies for the duration of action handling,
/// ensuring safe concurrent access patterns.
pub struct ActionHandler<'a> {
    /// Mutable reference to the application state
    pub(crate) state: &'a mut AppState,
    /// Optional reference to the session bridge for backend communication
    pub(crate) session: Option<&'a SessionBridge>,
    /// Mutable reference to the stream controller for streaming state
    pub(crate) stream: &'a mut StreamController,
}

impl<'a> ActionHandler<'a> {
    /// Creates a new action handler with the given dependencies.
    ///
    /// # Arguments
    ///
    /// * `state` - Mutable reference to the application state
    /// * `session` - Optional reference to the session bridge
    /// * `stream` - Mutable reference to the stream controller
    pub fn new(
        state: &'a mut AppState,
        session: Option<&'a SessionBridge>,
        stream: &'a mut StreamController,
    ) -> Self {
        Self {
            state,
            session,
            stream,
        }
    }
}
