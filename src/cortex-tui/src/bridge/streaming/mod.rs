//! Stream controller state machine for managing streaming response state.
//!
//! This module provides a state machine that manages the lifecycle of streaming
//! AI responses, similar to cortex-tui's StreamController but integrated with
//! cortex-tui's animation system.
//!
//! # Overview
//!
//! The streaming system handles:
//! - State transitions (idle → processing → streaming → complete)
//! - Text buffering with optional newline-gated display
//! - Integration with [`Typewriter`] animation for smooth text reveal
//! - Time tracking for performance metrics (time-to-first-token, etc.)
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::bridge::streaming::{StreamController, StreamState};
//!
//! let mut controller = StreamController::with_typewriter(60.0);
//!
//! // User sends a message
//! controller.start_processing();
//! assert!(matches!(controller.state(), StreamState::Processing));
//!
//! // First token arrives
//! controller.append_text("Hello");
//! assert!(controller.state().is_active());
//!
//! // More tokens stream in
//! controller.append_text(", world!\n");
//!
//! // Stream completes
//! controller.complete();
//! assert!(controller.is_complete());
//!
//! // Access metrics
//! if let Some(ttft) = controller.time_to_first_token() {
//!     println!("Time to first token: {:?}", ttft);
//! }
//! ```

mod controller;
mod state;

#[cfg(test)]
mod tests;

// Re-export all public types for backwards compatibility
pub use controller::StreamController;
pub use state::StreamState;
