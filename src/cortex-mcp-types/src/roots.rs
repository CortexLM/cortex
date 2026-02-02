//! Root types for MCP protocol.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Root definition.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Root {
    /// Root URI.
    pub uri: String,
    /// Root name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Root {
    /// Create a new root.
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            name: None,
        }
    }

    /// Add a name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// List roots result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListRootsResult {
    /// Available roots.
    pub roots: Vec<Root>,
}
