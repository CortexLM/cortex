//! Tool router.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::context::ToolContext;
use super::handlers::batch::{BatchToolExecutor, BatchToolHandler, batch_tool_definition};
use super::handlers::*;
use super::registry::ToolRegistry;
use super::spec::{ToolDefinition, ToolHandler, ToolResult};
use crate::error::{CortexError, Result};

/// Routes tool calls to appropriate handlers.
pub struct ToolRouter {
    registry: ToolRegistry,
    handlers: HashMap<String, Box<dyn ToolHandler>>,
}

/// Arc-wrapped router that can be used as a BatchToolExecutor.
/// This allows the Batch tool to execute other tools through the router.
pub struct RouterExecutor {
    handlers: HashMap<String, Box<dyn ToolHandler>>,
}

impl RouterExecutor {
    /// Create a new RouterExecutor with a copy of the handlers.
    fn new(handlers: &HashMap<String, Box<dyn ToolHandler>>) -> Self {
        // We need to clone the handlers map, but Box<dyn ToolHandler> isn't Clone.
        // Instead, we'll create a new set of handlers.
        let mut new_handlers: HashMap<String, Box<dyn ToolHandler>> = HashMap::new();

        // Re-create all the handlers
        new_handlers.insert("Execute".to_string(), Box::new(LocalShellHandler::new()));
        new_handlers.insert("Read".to_string(), Box::new(ReadFileHandler::new()));
        new_handlers.insert("Create".to_string(), Box::new(WriteFileHandler::new()));
        new_handlers.insert("Tree".to_string(), Box::new(TreeHandler::new()));
        new_handlers.insert("LS".to_string(), Box::new(TreeHandler::new()));
        new_handlers.insert(
            "SearchFiles".to_string(),
            Box::new(SearchFilesHandler::new()),
        );
        new_handlers.insert(
            "ApplyPatch".to_string(),
            Box::new(crate::agent::tools::PatchTool::new()),
        );
        new_handlers.insert("WebSearch".to_string(), Box::new(WebSearchHandler::new()));
        new_handlers.insert("Patch".to_string(), Box::new(PatchHandler::new()));
        new_handlers.insert(
            "MultiEdit".to_string(),
            Box::new(crate::agent::tools::MultiEditTool::new()),
        );
        new_handlers.insert("Grep".to_string(), Box::new(GrepHandler::new()));
        new_handlers.insert("Glob".to_string(), Box::new(GlobHandler::new()));
        new_handlers.insert(
            "FetchUrl".to_string(),
            Box::new(crate::agent::tools::WebFetchTool::new()),
        );
        new_handlers.insert(
            "WebFetch".to_string(),
            Box::new(crate::agent::tools::WebFetchTool::new()),
        );
        new_handlers.insert("TodoWrite".to_string(), Box::new(TodoWriteHandler::new()));
        new_handlers.insert("TodoRead".to_string(), Box::new(TodoReadHandler::new()));
        new_handlers.insert("Plan".to_string(), Box::new(PlanHandler::new()));
        new_handlers.insert("Propose".to_string(), Box::new(ProposeHandler::new()));
        new_handlers.insert("Questions".to_string(), Box::new(QuestionsHandler::new()));
        new_handlers.insert(
            "LspHover".to_string(),
            Box::new(crate::agent::tools::LspHoverTool::new()),
        );
        new_handlers.insert(
            "LspDiagnostics".to_string(),
            Box::new(crate::agent::tools::LspDiagnosticsTool::new_handler()),
        );

        // Also include any custom handlers that were registered
        for name in handlers.keys() {
            if !new_handlers.contains_key(name) {
                // For custom handlers we can't recreate, we skip them in batch
                // This is a limitation, but necessary for safety
                tracing::debug!("Custom handler '{}' not available in batch execution", name);
            }
        }

        Self {
            handlers: new_handlers,
        }
    }
}

#[async_trait]
impl BatchToolExecutor for RouterExecutor {
    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<ToolResult> {
        let handler = self
            .handlers
            .get(name)
            .ok_or_else(|| CortexError::UnknownTool {
                name: name.to_string(),
            })?;

        handler.execute(arguments, context).await
    }

    fn has_tool(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }
}

impl ToolRouter {
    /// Create a new tool router with default tools.
    pub fn new() -> Self {
        let mut registry = ToolRegistry::new();
        let mut handlers: HashMap<String, Box<dyn ToolHandler>> = HashMap::new();

        // Register default handlers with standardized names matching system prompt
        handlers.insert("Execute".to_string(), Box::new(LocalShellHandler::new()));
        handlers.insert("Read".to_string(), Box::new(ReadFileHandler::new()));
        handlers.insert("Create".to_string(), Box::new(WriteFileHandler::new()));
        handlers.insert("Tree".to_string(), Box::new(TreeHandler::new()));
        handlers.insert("LS".to_string(), Box::new(TreeHandler::new()));
        handlers.insert(
            "SearchFiles".to_string(),
            Box::new(SearchFilesHandler::new()),
        );
        handlers.insert(
            "ApplyPatch".to_string(),
            Box::new(crate::agent::tools::PatchTool::new()),
        );
        handlers.insert("WebSearch".to_string(), Box::new(WebSearchHandler::new()));

        // Additional handlers
        handlers.insert("Patch".to_string(), Box::new(PatchHandler::new()));
        handlers.insert(
            "MultiEdit".to_string(),
            Box::new(crate::agent::tools::MultiEditTool::new()),
        );
        handlers.insert("Grep".to_string(), Box::new(GrepHandler::new()));
        handlers.insert("Glob".to_string(), Box::new(GlobHandler::new()));
        handlers.insert(
            "FetchUrl".to_string(),
            Box::new(crate::agent::tools::WebFetchTool::new()),
        );
        handlers.insert(
            "WebFetch".to_string(),
            Box::new(crate::agent::tools::WebFetchTool::new()),
        );
        handlers.insert("TodoWrite".to_string(), Box::new(TodoWriteHandler::new()));
        handlers.insert("TodoRead".to_string(), Box::new(TodoReadHandler::new()));
        handlers.insert("Plan".to_string(), Box::new(PlanHandler::new()));
        handlers.insert("Propose".to_string(), Box::new(ProposeHandler::new()));
        handlers.insert("Questions".to_string(), Box::new(QuestionsHandler::new()));
        handlers.insert(
            "LspHover".to_string(),
            Box::new(crate::agent::tools::LspHoverTool::new()),
        );
        handlers.insert(
            "LspDiagnostics".to_string(),
            Box::new(crate::agent::tools::LspDiagnosticsTool::new_handler()),
        );

        // Create the Batch tool handler with a RouterExecutor
        let router_executor = Arc::new(RouterExecutor::new(&handlers));
        let batch_handler = BatchToolHandler::new(router_executor);
        handlers.insert("Batch".to_string(), Box::new(batch_handler));

        // Register batch tool definition
        registry.register(batch_tool_definition());

        Self { registry, handlers }
    }

    /// Execute a tool.
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<ToolResult> {
        let handler = self
            .handlers
            .get(tool_name)
            .ok_or_else(|| CortexError::UnknownTool {
                name: tool_name.to_string(),
            })?;

        handler.execute(arguments, context).await
    }

    /// Get tool definitions for the model.
    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.registry.all().into_iter().cloned().collect()
    }

    /// Check if a tool is available.
    pub fn has_tool(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Register a custom tool handler.
    pub fn register_handler(&mut self, handler: Box<dyn ToolHandler>) {
        self.handlers.insert(handler.name().to_string(), handler);
    }

    /// Register a tool with both its definition and handler.
    pub fn register(&mut self, definition: ToolDefinition, handler: Box<dyn ToolHandler>) {
        self.registry.register(definition);
        self.handlers.insert(handler.name().to_string(), handler);
    }

    /// Set the LSP integration.
    pub fn set_lsp(&mut self, lsp: std::sync::Arc<crate::integrations::LspIntegration>) {
        self.registry.set_lsp(lsp);
    }
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_read_file() {
        let router = ToolRouter::new();
        let context = ToolContext::new(PathBuf::from("."));

        let result = router
            .execute(
                "Read",
                serde_json::json!({ "path": "Cargo.toml" }),
                &context,
            )
            .await;

        // This test assumes we're in the cortex-core directory
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let router = ToolRouter::new();
        let context = ToolContext::new(PathBuf::from("."));

        let result = router
            .execute("unknown_tool", serde_json::json!({}), &context)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_batch_tool_exists() {
        let router = ToolRouter::new();
        assert!(router.has_tool("Batch"));
    }

    #[tokio::test]
    async fn test_batch_tool_execution() {
        let router = ToolRouter::new();
        let context = ToolContext::new(PathBuf::from("."));

        // Test batch execution with multiple LS operations
        // Note: LS handler may fail if directory doesn't exist in test context,
        // but batch should still complete and report results
        let result = router
            .execute(
                "Batch",
                serde_json::json!({
                    "calls": [
                        {"tool": "LS", "arguments": {"directory_path": "."}},
                        {"tool": "LS", "arguments": {"directory_path": "."}}
                    ]
                }),
                &context,
            )
            .await;

        assert!(result.is_ok());
        let tool_result = result.unwrap();
        // Batch completed - check output contains results summary
        assert!(
            tool_result.output.contains("Results:")
                || tool_result.output.contains("tools executed"),
            "Expected batch results summary, got: {}",
            tool_result.output
        );
    }

    #[tokio::test]
    async fn test_batch_prevents_recursion() {
        let router = ToolRouter::new();
        let context = ToolContext::new(PathBuf::from("."));

        // Try to call Batch within Batch - should fail validation with Result::Err
        // because recursive/disallowed tools trigger a validation error
        let result = router
            .execute(
                "Batch",
                serde_json::json!({
                    "calls": [
                        {"tool": "Batch", "arguments": {"calls": []}}
                    ]
                }),
                &context,
            )
            .await;

        // Validation errors return Result::Err, not Result::Ok with error ToolResult
        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        assert!(
            error_msg.contains("cannot be called within a batch")
                || error_msg.contains("Recursive"),
            "Expected recursion error message, got: {}",
            error_msg
        );
    }
}
