//! Bridge module for cortex-core integration.
//!
//! This module provides bridge types that wrap cortex-core APIs to provide
//! clean async interfaces for cortex-tui.
//!
//! ## Architecture
//!
//! The bridge pattern decouples the TUI layer from the core business logic:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        cortex-tui                               │
//! │  ┌─────────────────────────────────────────────────────────┐   │
//! │  │                      Bridge Layer                        │   │
//! │  │  ┌─────────────────┐  ┌───────────────────┐             │   │
//! │  │  │  SessionBridge  │  │SubmissionBuilder  │             │   │
//! │  │  └────────┬────────┘  └─────────┬─────────┘             │   │
//! │  │  ┌─────────────────┐  ┌───────────────────┐             │   │
//! │  │  │  EventAdapter   │  │  StreamController │             │   │
//! │  │  └────────┬────────┘  └─────────┬─────────┘             │   │
//! │  └───────────┼─────────────────────┼────────────────────────┘   │
//! └──────────────┼─────────────────────┼────────────────────────────┘
//!                │                     │
//! ┌──────────────▼─────────────────────▼────────────────────────────┐
//! │                        cortex-core                              │
//! │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
//! │  │     Session     │  │   ToolRouter    │  │      MCP        │  │
//! │  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Modules
//!
//! - [`session_bridge`] - Bridge to cortex-core's Session API
//! - [`submission_builder`] - Fluent builder for protocol Submission messages
//! - [`streaming`] - State machine for managing streaming response state
//! - [`event_adapter`] - Converts cortex_protocol events to cortex-tui AppEvents

pub mod event_adapter;
pub mod session_bridge;
pub mod streaming;
pub mod submission_builder;

// Re-exports for convenience
pub use event_adapter::{
    adapt_event, adapt_events, create_approval_state, create_patch_approval_state,
    is_high_priority_event, is_mcp_event, is_message_event, is_session_event, is_streaming_event,
    is_tool_event,
};
pub use session_bridge::SessionBridge;
pub use streaming::{StreamController, StreamState};
pub use submission_builder::{SubmissionBuilder, SubmissionSender};
