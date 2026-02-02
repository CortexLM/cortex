use cortex_mcp_client::discovery::ToolDiscovery;
use cortex_mcp_types::{PropertySchema, Tool, ToolInputSchema};
use serde_json::json;

#[test]
fn test_tool_conversion_simple() {
    let mcp_tool = Tool::new("test_tool", "A test tool").with_schema(
        ToolInputSchema::object()
            .property(
                "query",
                PropertySchema::string().description("Search query"),
            )
            .required(vec!["query"]),
    );

    let tool_def = ToolDiscovery::to_tool_definition(&mcp_tool);

    assert_eq!(tool_def.name, "test_tool");
    assert_eq!(tool_def.description, "A test tool");
    assert_eq!(tool_def.parameters["type"], "object");
    assert!(tool_def.parameters["properties"].is_object());
    assert_eq!(tool_def.parameters["required"], json!(["query"]));
}

#[test]
fn test_tool_conversion_complex_schema() {
    let mcp_tool = Tool::new("complex_tool", "Complex tool with nested schema").with_schema(
        ToolInputSchema::object()
            .property(
                "name",
                PropertySchema::string()
                    .description("Name field")
                    .min_len(1)
                    .max_len(100),
            )
            .property(
                "age",
                PropertySchema::integer()
                    .description("Age field")
                    .min(0.0)
                    .max(150.0),
            )
            .property("tags", PropertySchema::array(PropertySchema::string()))
            .required(vec!["name", "age"]),
    );

    let tool_def = ToolDiscovery::to_tool_definition(&mcp_tool);

    assert_eq!(tool_def.name, "complex_tool");
    assert_eq!(tool_def.parameters["type"], "object");

    let properties = &tool_def.parameters["properties"];
    assert_eq!(properties["name"]["type"], "string");
    assert_eq!(properties["name"]["minLength"], 1);
    assert_eq!(properties["name"]["maxLength"], 100);

    assert_eq!(properties["age"]["type"], "integer");
    assert_eq!(properties["age"]["minimum"], 0.0);
    assert_eq!(properties["age"]["maximum"], 150.0);

    assert_eq!(properties["tags"]["type"], "array");
    assert_eq!(properties["tags"]["items"]["type"], "string");

    assert_eq!(tool_def.parameters["required"], json!(["name", "age"]));
}

#[test]
fn test_tool_conversion_enum() {
    let mcp_tool = Tool::new("enum_tool", "Tool with enum parameter").with_schema(
        ToolInputSchema::object()
            .property(
                "status",
                PropertySchema::string()
                    .description("Status field")
                    .enum_values(vec!["active", "inactive", "pending"]),
            )
            .required(vec!["status"]),
    );

    let tool_def = ToolDiscovery::to_tool_definition(&mcp_tool);

    let properties = &tool_def.parameters["properties"];
    assert_eq!(properties["status"]["type"], "string");
    assert_eq!(
        properties["status"]["enum"],
        json!(["active", "inactive", "pending"])
    );
}

#[test]
fn test_tool_conversion_no_description() {
    let mcp_tool = Tool::new_simple("simple_tool").with_schema(ToolInputSchema::object());

    let tool_def = ToolDiscovery::to_tool_definition(&mcp_tool);

    assert_eq!(tool_def.name, "simple_tool");
    assert_eq!(tool_def.description, "");
}

#[test]
fn test_multiple_tools_conversion() {
    let tools = vec![
        Tool::new("tool1", "First tool")
            .with_schema(ToolInputSchema::object().property("param1", PropertySchema::string())),
        Tool::new("tool2", "Second tool")
            .with_schema(ToolInputSchema::object().property("param2", PropertySchema::number())),
    ];

    let tool_defs = ToolDiscovery::to_tool_definitions(&tools);

    assert_eq!(tool_defs.len(), 2);
    assert_eq!(tool_defs[0].name, "tool1");
    assert_eq!(tool_defs[1].name, "tool2");
}

#[test]
fn test_tool_discovery_register_and_list() {
    let mut discovery = ToolDiscovery::new();

    let tool1 = Tool::new("tool1", "First tool").with_schema(ToolInputSchema::object());
    let tool2 = Tool::new("tool2", "Second tool").with_schema(ToolInputSchema::object());

    discovery.register_tool(tool1);
    discovery.register_tool(tool2);

    let tools = discovery.list_tools();
    assert_eq!(tools.len(), 2);

    assert!(discovery.get_tool("tool1").is_some());
    assert!(discovery.get_tool("tool2").is_some());
    assert!(discovery.get_tool("nonexistent").is_none());
}

#[test]
fn test_tool_discovery_clear() {
    let mut discovery = ToolDiscovery::new();

    let tool = Tool::new("tool1", "First tool").with_schema(ToolInputSchema::object());

    discovery.register_tool(tool);
    assert_eq!(discovery.list_tools().len(), 1);

    discovery.clear();
    assert_eq!(discovery.list_tools().len(), 0);
}

#[test]
fn test_nested_object_schema() {
    let mcp_tool = Tool::new("nested_tool", "Tool with nested objects").with_schema(
        ToolInputSchema::object()
            .property("user", PropertySchema::object().description("User object")),
    );

    let tool_def = ToolDiscovery::to_tool_definition(&mcp_tool);

    let properties = &tool_def.parameters["properties"];
    assert_eq!(properties["user"]["type"], "object");
    assert_eq!(properties["user"]["description"], "User object");
}

#[test]
fn test_pattern_validation() {
    let mcp_tool = Tool::new("pattern_tool", "Tool with pattern validation").with_schema(
        ToolInputSchema::object().property(
            "email",
            PropertySchema::string().pattern(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"),
        ),
    );

    let tool_def = ToolDiscovery::to_tool_definition(&mcp_tool);

    let properties = &tool_def.parameters["properties"];
    assert_eq!(
        properties["email"]["pattern"],
        r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
    );
}

#[test]
fn test_default_values() {
    let mcp_tool = Tool::new("default_tool", "Tool with default values").with_schema(
        ToolInputSchema::object()
            .property("count", PropertySchema::integer().default_value(json!(10))),
    );

    let tool_def = ToolDiscovery::to_tool_definition(&mcp_tool);

    let properties = &tool_def.parameters["properties"];
    assert_eq!(properties["count"]["default"], 10);
}
