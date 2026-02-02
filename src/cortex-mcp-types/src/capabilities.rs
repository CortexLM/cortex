//! Client and server capability types for MCP protocol.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Client capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    /// Experimental capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, Value>>,
    /// Sampling capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,
    /// Roots capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
}

/// Server capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    /// Experimental capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, Value>>,
    /// Logging capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
    /// Prompts capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    /// Resources capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    /// Tools capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
}

impl ServerCapabilities {
    /// Create capabilities with tools support.
    pub fn with_tools(mut self) -> Self {
        self.tools = Some(ToolsCapability::default());
        self
    }

    /// Create capabilities with resources support.
    pub fn with_resources(mut self) -> Self {
        self.resources = Some(ResourcesCapability::default());
        self
    }

    /// Create capabilities with prompts support.
    pub fn with_prompts(mut self) -> Self {
        self.prompts = Some(PromptsCapability::default());
        self
    }

    /// Create capabilities with logging support.
    pub fn with_logging(mut self) -> Self {
        self.logging = Some(LoggingCapability {});
        self
    }
}

/// Sampling capability (client).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SamplingCapability {}

/// Roots capability (client).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RootsCapability {
    /// Whether the client supports list changed notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Logging capability (server).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct LoggingCapability {}

/// Prompts capability (server).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptsCapability {
    /// Whether the server supports list changed notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Resources capability (server).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesCapability {
    /// Whether the server supports resource subscriptions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    /// Whether the server supports list changed notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Tools capability (server).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCapability {
    /// Whether the server supports list changed notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_capabilities() {
        let caps = ServerCapabilities::default()
            .with_tools()
            .with_resources()
            .with_prompts()
            .with_logging();

        assert!(caps.tools.is_some());
        assert!(caps.resources.is_some());
        assert!(caps.prompts.is_some());
        assert!(caps.logging.is_some());
    }
}
