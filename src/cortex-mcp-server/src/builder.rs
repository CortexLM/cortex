//! MCP Server builder for easy server construction.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};

use anyhow::Result;
use cortex_mcp_types::{CallToolResult, Implementation, LogLevel, ServerCapabilities, Tool};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::handlers::{FnToolHandler, NoOpToolHandler, ToolHandler};
use crate::providers::{PromptProvider, ResourceProvider};
use crate::server::{McpServer, ServerState};

/// Builder for creating MCP servers.
pub struct McpServerBuilder {
    name: String,
    version: String,
    capabilities: ServerCapabilities,
    tools: Vec<Arc<dyn ToolHandler>>,
    resource_provider: Option<Arc<dyn ResourceProvider>>,
    prompt_provider: Option<Arc<dyn PromptProvider>>,
    instructions: Option<String>,
}

impl McpServerBuilder {
    /// Create a new server builder.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            capabilities: ServerCapabilities::default(),
            tools: Vec::new(),
            resource_provider: None,
            prompt_provider: None,
            instructions: None,
        }
    }

    /// Set server capabilities.
    pub fn capabilities(mut self, capabilities: ServerCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Enable tools capability.
    pub fn with_tools_capability(mut self) -> Self {
        self.capabilities = self.capabilities.with_tools();
        self
    }

    /// Enable resources capability.
    pub fn with_resources_capability(mut self) -> Self {
        self.capabilities = self.capabilities.with_resources();
        self
    }

    /// Enable prompts capability.
    pub fn with_prompts_capability(mut self) -> Self {
        self.capabilities = self.capabilities.with_prompts();
        self
    }

    /// Enable logging capability.
    pub fn with_logging_capability(mut self) -> Self {
        self.capabilities = self.capabilities.with_logging();
        self
    }

    /// Add a tool definition (creates a no-op handler).
    pub fn tool(mut self, tool: Tool) -> Self {
        let handler = Arc::new(NoOpToolHandler { tool });
        self.tools.push(handler);
        self.capabilities = self.capabilities.with_tools();
        self
    }

    /// Add a tool handler.
    pub fn tool_handler(mut self, handler: Arc<dyn ToolHandler>) -> Self {
        self.tools.push(handler);
        self.capabilities = self.capabilities.with_tools();
        self
    }

    /// Add a tool with a synchronous handler function.
    pub fn tool_fn<F>(mut self, tool: Tool, handler: F) -> Self
    where
        F: Fn(Value) -> Result<CallToolResult> + Send + Sync + 'static,
    {
        let handler = Arc::new(FnToolHandler::new(tool, handler));
        self.tools.push(handler);
        self.capabilities = self.capabilities.with_tools();
        self
    }

    /// Set the resource provider.
    pub fn resource_provider(mut self, provider: Arc<dyn ResourceProvider>) -> Self {
        self.resource_provider = Some(provider);
        self.capabilities = self.capabilities.with_resources();
        self
    }

    /// Set the prompt provider.
    pub fn prompt_provider(mut self, provider: Arc<dyn PromptProvider>) -> Self {
        self.prompt_provider = Some(provider);
        self.capabilities = self.capabilities.with_prompts();
        self
    }

    /// Set instructions for clients.
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Build the server.
    pub fn build(self) -> Result<Arc<McpServer>> {
        let info = Implementation::new(&self.name, &self.version);
        let server = McpServer {
            info,
            capabilities: self.capabilities,
            tools: RwLock::new(HashMap::new()),
            resource_provider: RwLock::new(self.resource_provider),
            prompt_provider: RwLock::new(self.prompt_provider),
            log_level: RwLock::new(LogLevel::Info),
            state: RwLock::new(ServerState::Uninitialized),
            running: AtomicBool::new(false),
            request_id: AtomicU64::new(1),
            pending_requests: RwLock::new(HashMap::new()),
            client_info: RwLock::new(None),
            protocol_version: RwLock::new(None),
            instructions: self.instructions,
        };

        let server = Arc::new(server);

        // Register tools
        let tools = self.tools;
        let server_clone = server.clone();
        tokio::spawn(async move {
            for handler in tools {
                server_clone.register_tool(handler).await;
            }
        });

        Ok(server)
    }

    /// Build and run the server with stdio transport.
    pub async fn build_and_run_stdio(self) -> Result<()> {
        let server = self.build()?;
        server.run_stdio().await
    }
}
