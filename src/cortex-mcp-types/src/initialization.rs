//! Initialization types for MCP protocol.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::PROTOCOL_VERSION;
use crate::capabilities::{ClientCapabilities, ServerCapabilities};

/// Initialize request parameters.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Protocol version the client supports.
    pub protocol_version: String,
    /// Client capabilities.
    pub capabilities: ClientCapabilities,
    /// Information about the client.
    pub client_info: Implementation,
}

impl Default for InitializeParams {
    fn default() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: Implementation::default(),
        }
    }
}

/// Initialize result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Protocol version the server is using.
    pub protocol_version: String,
    /// Server capabilities.
    pub capabilities: ServerCapabilities,
    /// Information about the server.
    pub server_info: Implementation,
    /// Optional instructions for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

impl InitializeResult {
    /// Create a new initialize result.
    pub fn new(server_info: Implementation, capabilities: ServerCapabilities) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities,
            server_info,
            instructions: None,
        }
    }

    /// Add instructions.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }
}

/// Implementation information (client or server).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Implementation {
    /// Name of the implementation.
    pub name: String,
    /// Version of the implementation.
    pub version: String,
}

impl Default for Implementation {
    fn default() -> Self {
        Self {
            name: "Cortex".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

impl Implementation {
    /// Create a new implementation.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_result() {
        let result = InitializeResult::new(
            Implementation::new("test-server", "1.0.0"),
            ServerCapabilities::default().with_tools(),
        )
        .with_instructions("Use tools carefully");

        assert_eq!(result.server_info.name, "test-server");
        assert!(result.instructions.is_some());
    }
}
