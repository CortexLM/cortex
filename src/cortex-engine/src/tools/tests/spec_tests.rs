//! Tests for tool specifications.

use crate::tools::spec::*;

#[test]
fn test_tool_result_success() {
    let result = ToolResult::success("Operation completed");

    assert!(result.success);
    assert_eq!(result.output, "Operation completed");
    assert!(result.error.is_none());
    assert!(result.metadata.is_none());
}

#[test]
fn test_tool_result_error() {
    let result = ToolResult::error("Something went wrong");

    assert!(!result.success);
    assert_eq!(result.output, "Something went wrong");
    assert_eq!(result.error, Some("Something went wrong".to_string()));
}

#[test]
fn test_tool_result_with_metadata() {
    let metadata = ToolMetadata {
        duration_ms: 150,
        exit_code: Some(0),
        files_modified: vec!["file1.rs".to_string(), "file2.rs".to_string()],
        data: None,
    };

    let result = ToolResult::success("Done").with_metadata(metadata);

    assert!(result.metadata.is_some());
    let meta = result.metadata.unwrap();
    assert_eq!(meta.duration_ms, 150);
    assert_eq!(meta.exit_code, Some(0));
    assert_eq!(meta.files_modified.len(), 2);
}

#[test]
fn test_tool_result_content() {
    let result = ToolResult::success("File content here");
    assert_eq!(result.content(), "File content here");
}

#[test]
fn test_tool_result_is_error() {
    let success = ToolResult::success("ok");
    let error = ToolResult::error("fail");

    assert!(!success.is_error());
    assert!(error.is_error());
}

#[test]
fn test_tool_call_creation() {
    let call = ToolCall {
        id: "call_123".to_string(),
        name: "read_file".to_string(),
        arguments: serde_json::json!({"path": "/tmp/test.txt"}),
    };

    assert_eq!(call.id, "call_123");
    assert_eq!(call.name, "read_file");
}

#[test]
fn test_tool_call_serialization() {
    let call = ToolCall {
        id: "tc_1".to_string(),
        name: "Execute".to_string(),
        arguments: serde_json::json!({
            "command": "ls -la",
            "timeout": 30
        }),
    };

    let json = serde_json::to_string(&call).expect("serialize");
    assert!(json.contains("Execute"));
    assert!(json.contains("ls -la"));

    let parsed: ToolCall = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.id, "tc_1");
    assert_eq!(parsed.name, "Execute");
}

#[test]
fn test_tool_definition_new() {
    let def = ToolDefinition::new(
        "Read",
        "Read a file from disk",
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file"
                }
            },
            "required": ["file_path"]
        }),
    );

    assert_eq!(def.name, "Read");
    assert_eq!(def.description, "Read a file from disk");
    assert!(def.parameters.is_object());
}

#[test]
fn test_tool_definition_serialization() {
    let def = ToolDefinition {
        name: "Grep".to_string(),
        description: "Search for patterns".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"}
            }
        }),
    };

    let json = serde_json::to_string(&def).expect("serialize");
    let parsed: ToolDefinition = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.name, "Grep");
    assert_eq!(parsed.description, "Search for patterns");
}

#[test]
fn test_tool_metadata_creation() {
    let meta = ToolMetadata {
        duration_ms: 1000,
        exit_code: None,
        files_modified: vec![],
        data: None,
    };

    assert_eq!(meta.duration_ms, 1000);
    assert!(meta.exit_code.is_none());
    assert!(meta.files_modified.is_empty());
    assert!(meta.data.is_none());
}

#[test]
fn test_tool_names_constants() {
    use tools::*;

    assert_eq!(LOCAL_SHELL, "local_shell");
    assert_eq!(APPLY_PATCH, "apply_patch");
    assert_eq!(READ_FILE, "read_file");
    assert_eq!(LIST_DIR, "list_dir");
    assert_eq!(WRITE_FILE, "write_file");
    assert_eq!(SEARCH_FILES, "search_files");
    assert_eq!(WEB_SEARCH, "web_search");
    assert_eq!(VIEW_IMAGE, "view_image");
    assert_eq!(EDIT_FILE, "edit_file");
    assert_eq!(GREP, "grep");
    assert_eq!(GLOB, "glob");
    assert_eq!(FETCH_URL, "fetch_url");
    assert_eq!(TODO_WRITE, "todo_write");
    assert_eq!(TODO_READ, "todo_read");
}

#[test]
fn test_tool_result_empty_output() {
    let result = ToolResult::success("");
    assert!(result.success);
    assert_eq!(result.output, "");
}

#[test]
fn test_tool_result_multiline_output() {
    let output = "Line 1\nLine 2\nLine 3";
    let result = ToolResult::success(output);

    assert_eq!(result.content(), output);
}

#[test]
fn test_tool_call_with_complex_arguments() {
    let call = ToolCall {
        id: "complex".to_string(),
        name: "Edit".to_string(),
        arguments: serde_json::json!({
            "file_path": "/src/main.rs",
            "old_str": "fn main() {}",
            "new_str": "fn main() {\n    println!(\"Hello\");\n}",
            "change_all": false
        }),
    };

    let json = serde_json::to_string(&call).expect("serialize");
    let parsed: ToolCall = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.arguments["file_path"], "/src/main.rs");
    assert_eq!(parsed.arguments["change_all"], false);
}

#[test]
fn test_tool_definition_with_complex_schema() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Shell command to execute"
            },
            "timeout": {
                "type": "number",
                "description": "Timeout in seconds",
                "default": 30
            },
            "env": {
                "type": "object",
                "additionalProperties": {"type": "string"}
            }
        },
        "required": ["command"]
    });

    let def = ToolDefinition::new("Execute", "Execute a shell command", schema);

    assert!(def.parameters["properties"]["command"].is_object());
    assert!(def.parameters["properties"]["timeout"].is_object());
}
