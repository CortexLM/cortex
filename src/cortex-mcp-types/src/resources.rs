//! Resource types for MCP protocol.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// MCP resource definition.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    /// Resource URI.
    pub uri: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type of the resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

impl Resource {
    /// Create a new resource.
    pub fn new(uri: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            name: name.into(),
            description: None,
            mime_type: None,
        }
    }

    /// Add a description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set MIME type.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }
}

/// Resource template for dynamic resources.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTemplate {
    /// URI template (RFC 6570).
    pub uri_template: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

impl ResourceTemplate {
    /// Create a new resource template.
    pub fn new(uri_template: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uri_template: uri_template.into(),
            name: name.into(),
            description: None,
            mime_type: None,
        }
    }
}

/// List resources request parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListResourcesParams {
    /// Pagination cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// List resources result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListResourcesResult {
    /// Available resources.
    pub resources: Vec<Resource>,
    /// Next page cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl ListResourcesResult {
    /// Create a new result.
    pub fn new(resources: Vec<Resource>) -> Self {
        Self {
            resources,
            next_cursor: None,
        }
    }
}

/// List resource templates result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListResourceTemplatesResult {
    /// Resource templates.
    pub resource_templates: Vec<ResourceTemplate>,
    /// Next page cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Read resource request parameters.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadResourceParams {
    /// Resource URI to read.
    pub uri: String,
}

impl ReadResourceParams {
    /// Create new params.
    pub fn new(uri: impl Into<String>) -> Self {
        Self { uri: uri.into() }
    }
}

/// Read resource result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadResourceResult {
    /// Resource contents.
    pub contents: Vec<ResourceContent>,
}

impl ReadResourceResult {
    /// Create a result with text content.
    pub fn text(uri: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            contents: vec![ResourceContent::text(uri, text)],
        }
    }

    /// Create a result with binary content.
    pub fn blob(
        uri: impl Into<String>,
        blob: impl Into<String>,
        mime_type: impl Into<String>,
    ) -> Self {
        Self {
            contents: vec![ResourceContent::blob(uri, blob, mime_type)],
        }
    }
}

/// Resource content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    /// Resource URI.
    pub uri: String,
    /// MIME type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Text content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Binary content (base64 encoded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

impl ResourceContent {
    /// Create text content.
    pub fn text(uri: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            mime_type: Some("text/plain".to_string()),
            text: Some(text.into()),
            blob: None,
        }
    }

    /// Create binary content.
    pub fn blob(
        uri: impl Into<String>,
        blob: impl Into<String>,
        mime_type: impl Into<String>,
    ) -> Self {
        Self {
            uri: uri.into(),
            mime_type: Some(mime_type.into()),
            text: None,
            blob: Some(blob.into()),
        }
    }
}

/// Subscribe to resource request parameters.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SubscribeParams {
    /// Resource URI to subscribe to.
    pub uri: String,
}

/// Unsubscribe from resource request parameters.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UnsubscribeParams {
    /// Resource URI to unsubscribe from.
    pub uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_creation() {
        let resource = Resource::new("file:///test.txt", "Test File")
            .with_description("A test file")
            .with_mime_type("text/plain");

        assert_eq!(resource.uri, "file:///test.txt");
        assert_eq!(resource.name, "Test File");
        assert!(resource.description.is_some());
        assert_eq!(resource.mime_type, Some("text/plain".to_string()));
    }
}
