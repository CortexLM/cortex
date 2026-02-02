//! Tool handler traits and implementations.

use anyhow::Result;
use cortex_mcp_types::{CallToolResult, Tool};
use serde_json::Value;

/// Trait for implementing custom tool handlers.
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    /// Get the tool definition.
    fn tool(&self) -> Tool;

    /// Execute the tool with given arguments.
    async fn execute(&self, arguments: Value) -> Result<CallToolResult>;
}

/// A simple function-based tool handler.
pub struct FnToolHandler<F>
where
    F: Fn(Value) -> Result<CallToolResult> + Send + Sync,
{
    tool: Tool,
    handler: F,
}

impl<F> FnToolHandler<F>
where
    F: Fn(Value) -> Result<CallToolResult> + Send + Sync,
{
    /// Create a new function-based tool handler.
    pub fn new(tool: Tool, handler: F) -> Self {
        Self { tool, handler }
    }
}

#[async_trait::async_trait]
impl<F> ToolHandler for FnToolHandler<F>
where
    F: Fn(Value) -> Result<CallToolResult> + Send + Sync,
{
    fn tool(&self) -> Tool {
        self.tool.clone()
    }

    async fn execute(&self, arguments: Value) -> Result<CallToolResult> {
        (self.handler)(arguments)
    }
}

/// An async function-based tool handler.
pub struct AsyncToolHandler<F, Fut>
where
    F: Fn(Value) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<CallToolResult>> + Send,
{
    tool: Tool,
    handler: F,
}

impl<F, Fut> AsyncToolHandler<F, Fut>
where
    F: Fn(Value) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<CallToolResult>> + Send,
{
    /// Create a new async tool handler.
    pub fn new(tool: Tool, handler: F) -> Self {
        Self { tool, handler }
    }
}

#[async_trait::async_trait]
impl<F, Fut> ToolHandler for AsyncToolHandler<F, Fut>
where
    F: Fn(Value) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<CallToolResult>> + Send,
{
    fn tool(&self) -> Tool {
        self.tool.clone()
    }

    async fn execute(&self, arguments: Value) -> Result<CallToolResult> {
        (self.handler)(arguments).await
    }
}

/// A no-op tool handler that returns an error.
pub(crate) struct NoOpToolHandler {
    pub(crate) tool: Tool,
}

#[async_trait::async_trait]
impl ToolHandler for NoOpToolHandler {
    fn tool(&self) -> Tool {
        self.tool.clone()
    }

    async fn execute(&self, _arguments: Value) -> Result<CallToolResult> {
        Ok(CallToolResult::error(format!(
            "Tool '{}' has no implementation",
            self.tool.name
        )))
    }
}
