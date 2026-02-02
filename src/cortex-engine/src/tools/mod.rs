//! Tool routing and execution.
//!
//! Manages available tools and routes tool calls to handlers.
//!
//! # Unified Executor
//!
//! The [`UnifiedToolExecutor`] provides a single entry point for all tool execution,
//! including special tools like Task (subagent spawning) and Batch (parallel execution).
//! This is the recommended way to execute tools from cortex-tui.
//!
//! ```rust,ignore
//! use cortex_engine::tools::{UnifiedToolExecutor, ExecutorConfig};
//!
//! let config = ExecutorConfig::from_env()?;
//! let executor = UnifiedToolExecutor::new(config)?;
//!
//! // Execute any tool - Task and Batch are handled automatically
//! let result = executor.execute("Read", args, context).await?;
//! ```
//!
//! # Tool Artifacts
//!
//! When tools return large outputs (>32KB by default), the content is automatically
//! truncated and the full output is saved to an artifact file. The agent can use
//! the Read tool to access the full content if needed.
//!
//! See [`artifacts`] module for configuration and usage.

pub mod artifacts;
pub mod context;
pub mod handlers;
pub mod registry;
pub mod router;
pub mod spec;
pub mod unified_executor;

#[cfg(test)]
mod tests;

pub use artifacts::{
    ARTIFACTS_SUBDIR, ArtifactConfig, ArtifactResult, DEFAULT_TRUNCATE_LINES,
    DEFAULT_TRUNCATE_THRESHOLD, cleanup_old_artifacts, cleanup_session_artifacts, process_output,
    process_tool_result,
};
pub use context::ToolContext;
pub use handlers::*;
pub use registry::{PluginTool, ToolRegistry};
pub use router::ToolRouter;
pub use spec::{ToolCall, ToolDefinition, ToolHandler, ToolResult};
pub use unified_executor::{ExecutorConfig, UnifiedToolExecutor};
