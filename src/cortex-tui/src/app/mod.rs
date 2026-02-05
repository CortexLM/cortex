//! Application state module for cortex-tui.
//!
//! This module contains all the core application state types and management logic.

mod approval;
mod autocomplete;
mod methods;
mod session;
mod state;
mod streaming;
mod subagent;
mod types;

// Re-export all public types
pub use approval::{ApprovalState, PendingToolResult};
pub use autocomplete::{AutocompleteItem, AutocompleteState};
pub use session::{ActiveModal, SessionSummary};
pub use state::{AppState, MainAgentTodoItem, MainAgentTodoStatus};
pub use streaming::StreamingState;
pub use subagent::{
    SubagentDisplayStatus, SubagentTaskDisplay, SubagentTodoItem, SubagentTodoStatus,
};
pub use types::{AppView, ApprovalMode, AutocompleteTrigger, FocusTarget, OperationMode};
