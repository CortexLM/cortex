//! MCP (Model Context Protocol) operation methods for SubmissionBuilder.

use cortex_protocol::Op;

use super::SubmissionBuilder;

impl SubmissionBuilder {
    /// Reload all MCP servers.
    ///
    /// Triggers a reconnection/reload of all configured MCP servers.
    pub fn reload_mcp_servers() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::ReloadMcpServers);
        builder
    }

    /// Enable a specific MCP server.
    ///
    /// # Arguments
    ///
    /// * `name` - The name/identifier of the MCP server to enable
    pub fn enable_mcp_server(name: impl Into<String>) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::EnableMcpServer { name: name.into() });
        builder
    }

    /// Disable a specific MCP server.
    ///
    /// # Arguments
    ///
    /// * `name` - The name/identifier of the MCP server to disable
    pub fn disable_mcp_server(name: impl Into<String>) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::DisableMcpServer { name: name.into() });
        builder
    }

    /// List available MCP tools.
    pub fn list_mcp_tools() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::ListMcpTools);
        builder
    }
}
