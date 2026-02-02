//! Tool execution module - connects to cortex-core tools.
//!
//! This module is organized into submodules:
//! - `types`: Core types (ToolResult, ToolDefinition)
//! - `executor`: Main executor that dispatches tool calls
//! - `filesystem`: File operations (read, write, edit, list, patch)
//! - `search`: Search operations (grep, glob)
//! - `web`: Web operations (fetch URL, web search)
//! - `shell`: Shell command execution
//! - `planning`: Planning tools (todos, plans, questions)
//! - `definitions`: Tool definitions for API
//! - `security`: Security helpers for command validation

mod definitions;
mod executor;
mod filesystem;
mod planning;
mod search;
mod security;
mod shell;
mod types;
mod web;

// Re-export public API
pub use definitions::get_tool_definitions;
pub use executor::ToolExecutor;
pub use types::{ToolDefinition, ToolResult};
