//! Loading spinner widgets with various styles.
//!
//! Provides spinner widgets for displaying loading states, progress indicators,
//! and AI thinking animations. All spinners integrate with the Cortex animation
//! system for smooth 120 FPS rendering.
//!
//! ## Available Widgets
//!
//! - [`SpinnerWidget`] - Basic spinner with optional label
//! - [`StatusSpinner`] - Spinner with status text and elapsed time
//! - [`ProgressSpinner`] - Spinner with progress indication (current/total)
//! - [`ThinkingIndicator`] - Pulsing spinner for AI thinking state (with "Thinking..." label)
//! - [`ToolIndicator`] - Spinner with tool name and elapsed time
//! - [`StreamingIndicator`] - Wave animation with token count
//! - [`ApprovalIndicator`] - Pulsing spinner with "Awaiting approval..." label

mod approval;
mod base;
mod progress;
mod status;
mod streaming;
mod thinking;
mod tool;

pub use approval::ApprovalIndicator;
pub use base::SpinnerWidget;
pub use progress::ProgressSpinner;
pub use status::StatusSpinner;
pub use streaming::StreamingIndicator;
pub use thinking::ThinkingIndicator;
pub use tool::ToolIndicator;
