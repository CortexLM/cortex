use cortex_mcp_types::Tool;
use serde_json::{Value, json};
use std::collections::HashMap;

pub struct ToolDiscovery {
    tools: HashMap<String, Tool>,
}

impl ToolDiscovery {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register_tool(&mut self, tool: Tool) {
        self.tools.insert(tool.name.clone(), tool);
    }

    pub fn get_tool(&self, name: &str) -> Option<&Tool> {
        self.tools.get(name)
    }

    pub fn list_tools(&self) -> Vec<&Tool> {
        self.tools.values().collect()
    }

    pub fn clear(&mut self) {
        self.tools.clear();
    }

    /// Convert MCP Tool to cortex-engine ToolDefinition format.
    ///
    /// MCP uses `input_schema` with JSON Schema, while cortex-engine uses `parameters`.
    /// This function performs the conversion.
    pub fn to_tool_definition(tool: &Tool) -> ToolDefinition {
        ToolDefinition {
            name: tool.name.clone(),
            description: tool.description.clone().unwrap_or_default(),
            parameters: convert_input_schema_to_parameters(&tool.input_schema),
        }
    }

    /// Convert multiple MCP Tools to cortex-engine ToolDefinitions.
    pub fn to_tool_definitions(tools: &[Tool]) -> Vec<ToolDefinition> {
        tools.iter().map(Self::to_tool_definition).collect()
    }
}

impl Default for ToolDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

/// cortex-engine ToolDefinition format.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Convert MCP ToolInputSchema to cortex-engine parameters format.
///
/// MCP format:
/// ```json
/// {
///   "type": "object",
///   "properties": { ... },
///   "required": [ ... ]
/// }
/// ```
///
/// cortex-engine expects the same JSON Schema format.
fn convert_input_schema_to_parameters(schema: &cortex_mcp_types::ToolInputSchema) -> Value {
    let mut params = json!({
        "type": schema.schema_type,
    });

    if let Some(ref properties) = schema.properties {
        let mut props_map = serde_json::Map::new();
        for (key, prop) in properties {
            props_map.insert(key.clone(), convert_property_schema(prop));
        }
        params["properties"] = Value::Object(props_map);
    }

    if let Some(ref required) = schema.required {
        params["required"] = json!(required);
    }

    if let Some(additional) = schema.additional_properties {
        params["additionalProperties"] = json!(additional);
    }

    if let Some(ref description) = schema.description {
        params["description"] = json!(description);
    }

    if let Some(ref enum_values) = schema.enum_values {
        params["enum"] = json!(enum_values);
    }

    if let Some(ref items) = schema.items {
        params["items"] = convert_property_schema(items);
    }

    params
}

/// Convert MCP PropertySchema to JSON Value.
fn convert_property_schema(prop: &cortex_mcp_types::PropertySchema) -> Value {
    let mut schema = json!({
        "type": prop.schema_type,
    });

    if let Some(ref description) = prop.description {
        schema["description"] = json!(description);
    }

    if let Some(ref default) = prop.default {
        schema["default"] = default.clone();
    }

    if let Some(ref enum_values) = prop.enum_values {
        schema["enum"] = json!(enum_values);
    }

    if let Some(minimum) = prop.minimum {
        schema["minimum"] = json!(minimum);
    }

    if let Some(maximum) = prop.maximum {
        schema["maximum"] = json!(maximum);
    }

    if let Some(min_length) = prop.min_length {
        schema["minLength"] = json!(min_length);
    }

    if let Some(max_length) = prop.max_length {
        schema["maxLength"] = json!(max_length);
    }

    if let Some(ref pattern) = prop.pattern {
        schema["pattern"] = json!(pattern);
    }

    if let Some(ref items) = prop.items {
        schema["items"] = convert_property_schema(items);
    }

    if let Some(ref properties) = prop.properties {
        let mut props_map = serde_json::Map::new();
        for (key, prop_schema) in properties {
            props_map.insert(key.clone(), convert_property_schema(prop_schema));
        }
        schema["properties"] = Value::Object(props_map);
    }

    if let Some(ref required) = prop.required {
        schema["required"] = json!(required);
    }

    schema
}
