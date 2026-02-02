//! Progress tracking module for real-time task progress display.
//!
//! This module provides the event system and collector for tracking the progress
//! of agent tasks, tool calls, and todo list updates.
//!
//! ## Components
//!
//! - [`events`] - Progress event types and emitter/subscriber
//! - [`ProgressEvent`] - Events emitted during task execution
//! - [`ProgressEmitter`] - Sends progress events
//! - [`ProgressSubscriber`] - Receives progress events
//! - [`TodoItem`] - Todo list item with status
//! - [`TodoStatus`] - Status of a todo item (Pending, InProgress, Completed)

pub mod events;

// Re-exports for convenience
pub use events::{
    DEFAULT_PROGRESS_CHANNEL_SIZE, ProgressEmitter, ProgressEvent, ProgressSubscriber, TaskResult,
    TodoItem, TodoStatus, ToolResult,
};
