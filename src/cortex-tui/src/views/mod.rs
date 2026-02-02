//! Cortex TUI Views
//!
//! This module contains the main view components for the Cortex TUI.
//!
//! ## Available Views
//!
//! - [`MinimalSessionView`](minimal_session::MinimalSessionView) - Minimalist terminal-style chat
//! - [`ApprovalView`](approval::ApprovalView) - Tool approval modal
//! - [`QuestionPromptView`](question_prompt::QuestionPromptView) - Interactive question prompt
//! - [`TasksView`](tasks::TasksView) - Background tasks manager
//! - [`ForgeView`](forge::ForgeView) - Forge validation dashboard
//! - [`tool_call`] - Tool call display types

pub mod approval;
pub mod forge;
pub mod minimal_session;
pub mod question_prompt;
pub mod tasks;
pub mod tool_call;

// Re-exports
pub use approval::ApprovalView;
pub use forge::ForgeView;
pub use minimal_session::MinimalSessionView;
pub use question_prompt::{QuestionClickZones, QuestionHit, QuestionPromptView};
pub use tasks::TasksView;
