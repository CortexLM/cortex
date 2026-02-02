//! Comprehensive tests for TurnItem types.

use crate::items::*;
use crate::user_input::UserInput;

#[test]
fn test_turn_item_user_message() {
    let inputs = vec![
        UserInput::text("Hello, world!"),
        UserInput::text("How are you?"),
    ];
    let item = TurnItem::UserMessage(UserMessageItem::new(&inputs));

    assert!(item.is_user_message());
    assert!(!item.is_agent_message());
    assert!(!item.is_tool_call());
    assert!(!item.id().is_empty());
}

#[test]
fn test_turn_item_agent_message() {
    let item = TurnItem::AgentMessage(AgentMessageItem::new("I'm doing great!"));

    assert!(!item.is_user_message());
    assert!(item.is_agent_message());
    assert!(!item.is_tool_call());
    assert!(!item.id().is_empty());
}

#[test]
fn test_turn_item_tool_call() {
    let item = TurnItem::ToolCall(ToolCallItem {
        id: "tool_1".to_string(),
        call_id: "call_123".to_string(),
        name: "read_file".to_string(),
        arguments: serde_json::json!({"path": "/tmp/test.txt"}),
    });

    assert!(!item.is_user_message());
    assert!(!item.is_agent_message());
    assert!(item.is_tool_call());
    assert_eq!(item.id(), "tool_1");
}

#[test]
fn test_turn_item_tool_result() {
    let item = TurnItem::ToolResult(ToolResultItem {
        id: "result_1".to_string(),
        call_id: "call_123".to_string(),
        output: "File content here".to_string(),
        is_error: false,
    });

    assert_eq!(item.id(), "result_1");
}

#[test]
fn test_turn_item_tool_result_error() {
    let item = TurnItem::ToolResult(ToolResultItem {
        id: "result_err".to_string(),
        call_id: "call_456".to_string(),
        output: "File not found".to_string(),
        is_error: true,
    });

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("is_error"));
    assert!(json.contains("true"));
}

#[test]
fn test_turn_item_web_search() {
    let item = TurnItem::WebSearch(WebSearchItem {
        id: "search_1".to_string(),
        query: "rust async programming".to_string(),
    });

    assert_eq!(item.id(), "search_1");

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("web_search"));
    assert!(json.contains("rust async programming"));
}

#[test]
fn test_turn_item_reasoning() {
    let item = TurnItem::Reasoning(ReasoningItem {
        id: "reason_1".to_string(),
        content: "Let me think about this...".to_string(),
        is_summary: false,
    });

    assert_eq!(item.id(), "reason_1");

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("reasoning"));
}

#[test]
fn test_turn_item_reasoning_summary() {
    let item = TurnItem::Reasoning(ReasoningItem {
        id: "reason_summary".to_string(),
        content: "Summary: I analyzed the problem...".to_string(),
        is_summary: true,
    });

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("is_summary"));
    assert!(json.contains("true"));
}

#[test]
fn test_user_message_item_creation() {
    let inputs = vec![
        UserInput::text("First message"),
        UserInput::image("base64data", "image/png"),
    ];

    let item = UserMessageItem::new(&inputs);

    assert_eq!(item.content.len(), 2);
    assert!(!item.id.is_empty());

    // Verify UUID format
    assert!(uuid::Uuid::parse_str(&item.id).is_ok());
}

#[test]
fn test_agent_message_item_creation() {
    let item1 = AgentMessageItem::new("Hello from agent");
    let item2 = AgentMessageItem::new(String::from("Another message"));

    assert_eq!(item1.content, "Hello from agent");
    assert_eq!(item2.content, "Another message");
    assert_ne!(item1.id, item2.id);
}

#[test]
fn test_turn_item_serialization_roundtrip() {
    let items = vec![
        TurnItem::UserMessage(UserMessageItem::new(&[UserInput::text("Hi")])),
        TurnItem::AgentMessage(AgentMessageItem::new("Hello")),
        TurnItem::ToolCall(ToolCallItem {
            id: "t1".to_string(),
            call_id: "c1".to_string(),
            name: "test".to_string(),
            arguments: serde_json::json!({}),
        }),
        TurnItem::ToolResult(ToolResultItem {
            id: "r1".to_string(),
            call_id: "c1".to_string(),
            output: "done".to_string(),
            is_error: false,
        }),
        TurnItem::WebSearch(WebSearchItem {
            id: "s1".to_string(),
            query: "test".to_string(),
        }),
        TurnItem::Reasoning(ReasoningItem {
            id: "re1".to_string(),
            content: "thinking".to_string(),
            is_summary: false,
        }),
    ];

    for item in items {
        let json = serde_json::to_string(&item).expect("serialize");
        let parsed: TurnItem = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(item.id(), parsed.id());
        assert_eq!(item.is_user_message(), parsed.is_user_message());
        assert_eq!(item.is_agent_message(), parsed.is_agent_message());
        assert_eq!(item.is_tool_call(), parsed.is_tool_call());
    }
}

#[test]
fn test_turn_item_tag_serialization() {
    let user_msg = TurnItem::UserMessage(UserMessageItem::new(&[]));
    let json = serde_json::to_string(&user_msg).expect("serialize");
    assert!(json.contains("\"type\":\"user_message\""));

    let agent_msg = TurnItem::AgentMessage(AgentMessageItem::new("test"));
    let json = serde_json::to_string(&agent_msg).expect("serialize");
    assert!(json.contains("\"type\":\"agent_message\""));

    let tool_call = TurnItem::ToolCall(ToolCallItem {
        id: "1".to_string(),
        call_id: "2".to_string(),
        name: "test".to_string(),
        arguments: serde_json::Value::Null,
    });
    let json = serde_json::to_string(&tool_call).expect("serialize");
    assert!(json.contains("\"type\":\"tool_call\""));
}

#[test]
fn test_tool_call_item_with_complex_arguments() {
    let args = serde_json::json!({
        "file_path": "/src/main.rs",
        "old_str": "fn old()",
        "new_str": "fn new()",
        "change_all": false,
        "nested": {
            "array": [1, 2, 3],
            "object": {"key": "value"}
        }
    });

    let item = TurnItem::ToolCall(ToolCallItem {
        id: "complex".to_string(),
        call_id: "call_complex".to_string(),
        name: "Edit".to_string(),
        arguments: args.clone(),
    });

    let json = serde_json::to_string(&item).expect("serialize");
    let parsed: TurnItem = serde_json::from_str(&json).expect("deserialize");

    if let TurnItem::ToolCall(tc) = parsed {
        assert_eq!(tc.arguments, args);
    } else {
        panic!("Expected ToolCall");
    }
}

#[test]
fn test_empty_user_message() {
    let item = UserMessageItem::new(&[]);
    assert!(item.content.is_empty());
    assert!(!item.id.is_empty());
}

#[test]
fn test_multiple_content_types_in_user_message() {
    let inputs = vec![
        UserInput::text("Check this image:"),
        UserInput::image("data:base64...", "image/jpeg"),
        UserInput::file("/path/to/file.txt"),
        UserInput::image_url("https://example.com/image.png"),
    ];

    let item = UserMessageItem::new(&inputs);
    assert_eq!(item.content.len(), 4);

    assert!(item.content[0].is_text());
    assert!(item.content[1].is_image());
    assert!(!item.content[2].is_image());
    assert!(item.content[3].is_image());
}
