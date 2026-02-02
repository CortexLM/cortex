#![allow(clippy::type_complexity)]
//! Cortex MCP Server - Model Context Protocol server implementation.
//!
//! This crate provides a complete MCP server implementation that can:
//! - Register and serve tools, resources, and prompts
//! - Handle tool invocations with proper error handling
//! - Support stdio and HTTP transports
//! - Integrate with cortex-core for tool execution
//!
//! # Example
//! ```rust,no_run
//! use cortex_mcp_server::{McpServer, McpServerBuilder, ToolHandler};
//! use cortex_mcp_types::{CallToolResult, Tool, ToolInputSchema};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let server = McpServerBuilder::new("my-server", "1.0.0")
//!         .tool(Tool::new("hello", "Say hello"))
//!         .build()?;
//!     
//!     // Run with stdio transport
//!     server.run_stdio().await
//! }
//! ```

// ============================================================================
// Module declarations
// ============================================================================

mod builder;
mod handlers;
mod providers;
mod server;

// ============================================================================
// Re-exports for backwards compatibility
// ============================================================================

// Handler types
pub use handlers::{AsyncToolHandler, FnToolHandler, ToolHandler};

// Provider types
pub use providers::{
    PromptProvider, ResourceProvider, StaticPromptProvider, StaticResourceProvider,
};

// Server types
pub use server::{McpServer, ServerState};

// Builder types
pub use builder::McpServerBuilder;

// Re-export cortex_mcp_types for convenience
pub use cortex_mcp_types;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_mcp_types::{
        CallToolResult, InitializeParams, JsonRpcRequest, ListToolsResult, PropertySchema, Tool,
        ToolInputSchema, methods,
    };
    use handlers::NoOpToolHandler;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_server_creation() {
        let server = McpServerBuilder::new("test-server", "1.0.0")
            .with_tools_capability()
            .build()
            .unwrap();

        assert_eq!(server.info().name, "test-server");
        assert_eq!(server.info().version, "1.0.0");
        assert!(server.capabilities().tools.is_some());
    }

    #[tokio::test]
    async fn test_tool_registration() {
        let server = McpServerBuilder::new("test-server", "1.0.0")
            .build()
            .unwrap();

        let tool = Tool::new("test-tool", "A test tool");
        let handler = Arc::new(NoOpToolHandler { tool });

        server.register_tool(handler).await;

        let tools = server.tools().await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test-tool");
    }

    #[tokio::test]
    async fn test_initialize_request() {
        let server = McpServerBuilder::new("test-server", "1.0.0")
            .with_tools_capability()
            .build()
            .unwrap();

        let request = JsonRpcRequest::new(1, methods::INITIALIZE)
            .with_params(serde_json::to_value(InitializeParams::default()).unwrap());

        let response = server.handle_request(request).await;
        assert!(response.is_success());

        let result: cortex_mcp_types::InitializeResult =
            serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(result.server_info.name, "test-server");
    }

    #[tokio::test]
    async fn test_list_tools_request() {
        let tool = Tool::new("echo", "Echo input")
            .with_schema(ToolInputSchema::object().property("message", PropertySchema::string()));

        let server = McpServerBuilder::new("test-server", "1.0.0")
            .tool(tool)
            .build()
            .unwrap();

        // Wait for tool registration
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let request = JsonRpcRequest::new(1, methods::TOOLS_LIST);
        let response = server.handle_request(request).await;
        assert!(response.is_success());

        let result: ListToolsResult = serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "echo");
    }

    #[tokio::test]
    async fn test_call_tool_request() {
        let tool = Tool::new("echo", "Echo input");
        let server = McpServerBuilder::new("test-server", "1.0.0")
            .tool_fn(tool, |args| {
                let message = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no message");
                Ok(CallToolResult::text(message))
            })
            .build()
            .unwrap();

        // Wait for tool registration
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let request = JsonRpcRequest::new(1, methods::TOOLS_CALL).with_params(json!({
            "name": "echo",
            "arguments": { "message": "Hello, World!" }
        }));

        let response = server.handle_request(request).await;
        assert!(response.is_success());

        let result: CallToolResult = serde_json::from_value(response.result.unwrap()).unwrap();
        assert!(!result.is_error());
        assert_eq!(result.content[0].as_text(), Some("Hello, World!"));
    }

    #[tokio::test]
    async fn test_unknown_method() {
        let server = McpServerBuilder::new("test-server", "1.0.0")
            .build()
            .unwrap();

        let request = JsonRpcRequest::new(1, "unknown/method");
        let response = server.handle_request(request).await;

        assert!(response.is_error());
        let error = response.error.unwrap();
        assert_eq!(error.code, cortex_mcp_types::ErrorCode::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_ping() {
        let server = McpServerBuilder::new("test-server", "1.0.0")
            .build()
            .unwrap();

        let request = JsonRpcRequest::new(1, methods::PING);
        let response = server.handle_request(request).await;

        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_set_log_level() {
        let server = McpServerBuilder::new("test-server", "1.0.0")
            .with_logging_capability()
            .build()
            .unwrap();

        assert_eq!(server.log_level().await, cortex_mcp_types::LogLevel::Info);

        let request = JsonRpcRequest::new(1, methods::LOGGING_SET_LEVEL)
            .with_params(json!({ "level": "debug" }));

        let response = server.handle_request(request).await;
        assert!(response.is_success());

        assert_eq!(server.log_level().await, cortex_mcp_types::LogLevel::Debug);
    }

    #[tokio::test]
    async fn test_static_resource_provider() {
        use cortex_mcp_types::Resource;

        let mut provider = StaticResourceProvider::new();
        provider.add_text(
            Resource::new("file:///test.txt", "Test File"),
            "Hello, World!",
        );

        let resources = provider.list().await.unwrap();
        assert_eq!(resources.len(), 1);

        let content = provider.read("file:///test.txt").await.unwrap();
        assert_eq!(content.text, Some("Hello, World!".to_string()));
    }

    #[tokio::test]
    async fn test_server_state_transitions() {
        use cortex_mcp_types::JsonRpcNotification;

        let server = McpServerBuilder::new("test-server", "1.0.0")
            .build()
            .unwrap();

        assert_eq!(server.state().await, ServerState::Uninitialized);

        // Initialize
        let request = JsonRpcRequest::new(1, methods::INITIALIZE)
            .with_params(serde_json::to_value(InitializeParams::default()).unwrap());
        server.handle_request(request).await;

        assert_eq!(server.state().await, ServerState::Initializing);

        // Send initialized notification
        let notification = JsonRpcNotification::new(methods::INITIALIZED);
        server.handle_notification(notification).await;

        assert_eq!(server.state().await, ServerState::Ready);

        // Stop
        server.stop().await;
        assert_eq!(server.state().await, ServerState::ShuttingDown);
    }

    #[test]
    fn test_request_id_generation() {
        use cortex_mcp_types::{Implementation, RequestId, ServerCapabilities};

        let server = McpServer::new(
            Implementation::new("test", "1.0.0"),
            ServerCapabilities::default(),
        );

        let id1 = server.next_request_id();
        let id2 = server.next_request_id();
        let id3 = server.next_request_id();

        assert!(matches!(id1, RequestId::Number(1)));
        assert!(matches!(id2, RequestId::Number(2)));
        assert!(matches!(id3, RequestId::Number(3)));
    }
}
