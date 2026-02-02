//! Tool types for MCP protocol.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::content::Content;

/// MCP tool definition.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    /// Unique name for the tool.
    pub name: String,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: ToolInputSchema,
}

impl Tool {
    /// Create a new tool.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            input_schema: ToolInputSchema::object(),
        }
    }

    /// Create a tool without description.
    pub fn new_simple(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            input_schema: ToolInputSchema::object(),
        }
    }

    /// Set the input schema.
    pub fn with_schema(mut self, schema: ToolInputSchema) -> Self {
        self.input_schema = schema;
        self
    }
}

/// JSON Schema for tool input parameters.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolInputSchema {
    /// Schema type (usually "object").
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Property definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, PropertySchema>>,
    /// Required property names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    /// Additional properties allowed.
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "additionalProperties"
    )]
    pub additional_properties: Option<bool>,
    /// Property description (for non-object types).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Enum values (for string enums).
    #[serde(skip_serializing_if = "Option::is_none", rename = "enum")]
    pub enum_values: Option<Vec<String>>,
    /// Array item schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<PropertySchema>>,
}

impl ToolInputSchema {
    /// Create an object schema.
    pub fn object() -> Self {
        Self {
            schema_type: "object".to_string(),
            properties: Some(HashMap::new()),
            required: None,
            additional_properties: Some(false),
            description: None,
            enum_values: None,
            items: None,
        }
    }

    /// Create a string schema.
    pub fn string() -> Self {
        Self {
            schema_type: "string".to_string(),
            properties: None,
            required: None,
            additional_properties: None,
            description: None,
            enum_values: None,
            items: None,
        }
    }

    /// Create a number schema.
    pub fn number() -> Self {
        Self {
            schema_type: "number".to_string(),
            properties: None,
            required: None,
            additional_properties: None,
            description: None,
            enum_values: None,
            items: None,
        }
    }

    /// Create an integer schema.
    pub fn integer() -> Self {
        Self {
            schema_type: "integer".to_string(),
            properties: None,
            required: None,
            additional_properties: None,
            description: None,
            enum_values: None,
            items: None,
        }
    }

    /// Create a boolean schema.
    pub fn boolean() -> Self {
        Self {
            schema_type: "boolean".to_string(),
            properties: None,
            required: None,
            additional_properties: None,
            description: None,
            enum_values: None,
            items: None,
        }
    }

    /// Create an array schema.
    pub fn array(items: PropertySchema) -> Self {
        Self {
            schema_type: "array".to_string(),
            properties: None,
            required: None,
            additional_properties: None,
            description: None,
            enum_values: None,
            items: Some(Box::new(items)),
        }
    }

    /// Add a description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add a property to an object schema.
    pub fn property(mut self, name: impl Into<String>, schema: PropertySchema) -> Self {
        if let Some(ref mut props) = self.properties {
            props.insert(name.into(), schema);
        }
        self
    }

    /// Set required properties.
    pub fn required(mut self, required: Vec<impl Into<String>>) -> Self {
        self.required = Some(required.into_iter().map(std::convert::Into::into).collect());
        self
    }

    /// Allow additional properties.
    pub fn allow_additional(mut self) -> Self {
        self.additional_properties = Some(true);
        self
    }

    /// Add enum values for a string schema.
    pub fn enum_values(mut self, values: Vec<impl Into<String>>) -> Self {
        self.enum_values = Some(values.into_iter().map(std::convert::Into::into).collect());
        self
    }
}

impl Default for ToolInputSchema {
    fn default() -> Self {
        Self::object()
    }
}

/// JSON Schema for a property.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PropertySchema {
    /// Property type.
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Property description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Default value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Enum values.
    #[serde(skip_serializing_if = "Option::is_none", rename = "enum")]
    pub enum_values: Option<Vec<String>>,
    /// Minimum value (for numbers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    /// Maximum value (for numbers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    /// Minimum length (for strings/arrays).
    #[serde(skip_serializing_if = "Option::is_none", rename = "minLength")]
    pub min_length: Option<u64>,
    /// Maximum length (for strings/arrays).
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxLength")]
    pub max_length: Option<u64>,
    /// Pattern (for strings).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Array item schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<PropertySchema>>,
    /// Object properties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, PropertySchema>>,
    /// Required properties (for objects).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
}

impl PropertySchema {
    /// Create a string property.
    pub fn string() -> Self {
        Self {
            schema_type: "string".to_string(),
            description: None,
            default: None,
            enum_values: None,
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            items: None,
            properties: None,
            required: None,
        }
    }

    /// Create a number property.
    pub fn number() -> Self {
        Self {
            schema_type: "number".to_string(),
            description: None,
            default: None,
            enum_values: None,
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            items: None,
            properties: None,
            required: None,
        }
    }

    /// Create an integer property.
    pub fn integer() -> Self {
        Self {
            schema_type: "integer".to_string(),
            description: None,
            default: None,
            enum_values: None,
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            items: None,
            properties: None,
            required: None,
        }
    }

    /// Create a boolean property.
    pub fn boolean() -> Self {
        Self {
            schema_type: "boolean".to_string(),
            description: None,
            default: None,
            enum_values: None,
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            items: None,
            properties: None,
            required: None,
        }
    }

    /// Create an array property.
    pub fn array(items: PropertySchema) -> Self {
        Self {
            schema_type: "array".to_string(),
            description: None,
            default: None,
            enum_values: None,
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            items: Some(Box::new(items)),
            properties: None,
            required: None,
        }
    }

    /// Create an object property.
    pub fn object() -> Self {
        Self {
            schema_type: "object".to_string(),
            description: None,
            default: None,
            enum_values: None,
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            items: None,
            properties: Some(HashMap::new()),
            required: None,
        }
    }

    /// Add a description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set default value.
    pub fn default_value(mut self, value: Value) -> Self {
        self.default = Some(value);
        self
    }

    /// Set enum values.
    pub fn enum_values(mut self, values: Vec<impl Into<String>>) -> Self {
        self.enum_values = Some(values.into_iter().map(std::convert::Into::into).collect());
        self
    }

    /// Set minimum (for numbers).
    pub fn min(mut self, min: f64) -> Self {
        self.minimum = Some(min);
        self
    }

    /// Set maximum (for numbers).
    pub fn max(mut self, max: f64) -> Self {
        self.maximum = Some(max);
        self
    }

    /// Set min length (for strings/arrays).
    pub fn min_len(mut self, len: u64) -> Self {
        self.min_length = Some(len);
        self
    }

    /// Set max length (for strings/arrays).
    pub fn max_len(mut self, len: u64) -> Self {
        self.max_length = Some(len);
        self
    }

    /// Set pattern (for strings).
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }
}

/// List tools request parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListToolsParams {
    /// Pagination cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// List tools result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    /// Available tools.
    pub tools: Vec<Tool>,
    /// Next page cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl ListToolsResult {
    /// Create a new result with tools.
    pub fn new(tools: Vec<Tool>) -> Self {
        Self {
            tools,
            next_cursor: None,
        }
    }
}

/// Call tool request parameters.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CallToolParams {
    /// Tool name to call.
    pub name: String,
    /// Tool arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

impl CallToolParams {
    /// Create new call params.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: None,
        }
    }

    /// Add arguments.
    pub fn with_arguments(mut self, args: Value) -> Self {
        self.arguments = Some(args);
        self
    }
}

/// Call tool result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    /// Result content.
    pub content: Vec<Content>,
    /// Whether the result is an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

impl CallToolResult {
    /// Create a success result with text content.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::text(text)],
            is_error: None,
        }
    }

    /// Create an error result.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![Content::text(message)],
            is_error: Some(true),
        }
    }

    /// Create a result with multiple content items.
    pub fn with_content(content: Vec<Content>) -> Self {
        Self {
            content,
            is_error: None,
        }
    }

    /// Check if result is an error.
    pub fn is_error(&self) -> bool {
        self.is_error.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_creation() {
        let tool = Tool::new("search", "Search files").with_schema(
            ToolInputSchema::object()
                .property(
                    "query",
                    PropertySchema::string().description("Search query"),
                )
                .required(vec!["query"]),
        );

        assert_eq!(tool.name, "search");
        assert!(tool.description.is_some());
        assert!(tool.input_schema.properties.is_some());
    }

    #[test]
    fn test_tool_input_schema() {
        let schema = ToolInputSchema::object()
            .property("name", PropertySchema::string().description("Name"))
            .property("age", PropertySchema::integer().min(0.0).max(150.0))
            .property("active", PropertySchema::boolean())
            .required(vec!["name"]);

        assert_eq!(schema.schema_type, "object");
        assert_eq!(schema.properties.as_ref().map(|p| p.len()), Some(3));
        assert_eq!(schema.required.as_ref().map(|r| r.len()), Some(1));
    }

    #[test]
    fn test_call_tool_result() {
        let success = CallToolResult::text("Success!");
        assert!(!success.is_error());

        let error = CallToolResult::error("Something went wrong");
        assert!(error.is_error());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let tool = Tool::new("test", "Test tool");
        let json = serde_json::to_string(&tool).expect("serialization should succeed");
        let parsed: Tool = serde_json::from_str(&json).expect("deserialization should succeed");
        assert_eq!(parsed.name, tool.name);
    }
}
