use crate::transport::Transport;
use anyhow::Result;
use cortex_mcp_types::{CallToolParams, CallToolResult, Implementation, InitializeParams, Tool};

pub struct McpClient {
    transport: Box<dyn Transport>,
}

impl McpClient {
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self { transport }
    }

    pub async fn connect(&self) -> Result<()> {
        let params = InitializeParams {
            protocol_version: cortex_mcp_types::PROTOCOL_VERSION.to_string(),
            capabilities: cortex_mcp_types::ClientCapabilities::default(),
            client_info: Implementation::default(),
        };

        self.transport.initialize(params).await?;
        self.transport.send_initialized().await?;
        Ok(())
    }

    pub async fn discover_tools(&self) -> Result<Vec<Tool>> {
        let result = self.transport.list_tools().await?;
        Ok(result.tools)
    }

    pub async fn call_tool(&self, params: CallToolParams) -> Result<CallToolResult> {
        self.transport.call_tool(params).await
    }

    pub async fn close(&self) -> Result<()> {
        self.transport.close().await
    }
}
