//! Cortex Core Widgets
//!
//! Reusable UI components built on ratatui with the Cortex visual identity.
//!
//! ## Available Widgets
//!
//! - [`Brain`](brain::Brain) - Animated ASCII brain logo with pulse effect
//! - [`Chat`](chat) - Message display with streaming support
//! - [`Input`](input::CortexInput) - Text input with history
//! - [`Spinner`](spinner::SpinnerWidget) - Loading indicator widget
//! - [`ToolIndicator`](spinner::ToolIndicator) - Tool execution indicator
//! - [`StreamingIndicator`](spinner::StreamingIndicator) - Streaming response indicator
//! - [`ApprovalIndicator`](spinner::ApprovalIndicator) - Approval waiting indicator
//! - [`ModeIndicator`](mode_indicator::ModeIndicator) - Operation mode indicator (Build/Plan/Spec)

pub mod brain;
pub mod chat;
pub mod input;
pub mod mode_indicator;
pub mod spinner;

// Re-exports
pub use brain::Brain;
pub use chat::{ChatWidget, Message, MessageRole};
pub use input::CortexInput;
pub use mode_indicator::{CompactModeIndicator, DisplayMode, ModeIndicator};
pub use spinner::{
    ApprovalIndicator, ProgressSpinner, SpinnerWidget, StatusSpinner, StreamingIndicator,
    ThinkingIndicator, ToolIndicator,
};
