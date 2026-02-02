//! Comprehensive tests for models module.

use crate::models::*;

#[test]
fn test_content_item_input_text() {
    let item = ContentItem::input_text("Hello from user");

    assert_eq!(item.as_text(), Some("Hello from user"));
}

#[test]
fn test_content_item_output_text() {
    let item = ContentItem::output_text("Response from model");

    assert_eq!(item.as_text(), Some("Response from model"));
}

#[test]
fn test_content_item_as_text_non_text() {
    let image = ContentItem::InputImage {
        image_url: "https://example.com/img.png".to_string(),
        detail: None,
    };

    assert_eq!(image.as_text(), None);

    let refusal = ContentItem::Refusal {
        refusal: "I cannot do that".to_string(),
    };

    assert_eq!(refusal.as_text(), None);
}

#[test]
fn test_content_item_all_variants_serialization() {
    let variants: Vec<ContentItem> = vec![
        ContentItem::InputText {
            text: "user text".to_string(),
        },
        ContentItem::OutputText {
            text: "model text".to_string(),
        },
        ContentItem::InputImage {
            image_url: "https://example.com/img.jpg".to_string(),
            detail: Some("high".to_string()),
        },
        ContentItem::InputImageBase64 {
            data: "base64data".to_string(),
            media_type: "image/png".to_string(),
            detail: None,
        },
        ContentItem::Refusal {
            refusal: "Cannot comply".to_string(),
        },
        ContentItem::ToolUse {
            id: "tool_1".to_string(),
            name: "search".to_string(),
            input: serde_json::json!({"query": "test"}),
        },
        ContentItem::ToolResult {
            tool_use_id: "tool_1".to_string(),
            content: "Search results".to_string(),
            is_error: false,
        },
    ];

    for variant in variants {
        let json = serde_json::to_string(&variant).expect("serialize");
        let parsed: ContentItem = serde_json::from_str(&json).expect("deserialize");

        // Verify roundtrip by comparing JSON
        let json2 = serde_json::to_string(&parsed).expect("serialize again");
        assert_eq!(json, json2);
    }
}

#[test]
fn test_response_item_message_assistant() {
    let item = ResponseItem::Message {
        id: Some("msg_1".to_string()),
        parent_id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::output_text("Hello!")],
    };

    assert!(item.is_assistant_message());
    assert!(!item.is_user_message());
    assert!(!item.is_function_call());
    assert_eq!(item.get_text_content(), Some("Hello!".to_string()));
}

#[test]
fn test_response_item_message_user() {
    let item = ResponseItem::Message {
        id: None,
        parent_id: None,
        role: "user".to_string(),
        content: vec![ContentItem::input_text("Hi there")],
    };

    assert!(!item.is_assistant_message());
    assert!(item.is_user_message());
    assert_eq!(item.get_text_content(), Some("Hi there".to_string()));
}

#[test]
fn test_response_item_function_call() {
    let item = ResponseItem::FunctionCall {
        id: "fc_1".to_string(),
        call_id: "call_123".to_string(),
        name: "get_weather".to_string(),
        arguments: r#"{"location":"NYC"}"#.to_string(),
    };

    assert!(!item.is_assistant_message());
    assert!(!item.is_user_message());
    assert!(item.is_function_call());
    assert_eq!(item.get_text_content(), None);
}

#[test]
fn test_response_item_function_call_output() {
    let item = ResponseItem::FunctionCallOutput {
        id: "fco_1".to_string(),
        call_id: "call_123".to_string(),
        output: r#"{"temp":"72F"}"#.to_string(),
    };

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("function_call_output"));
}

#[test]
fn test_response_item_reasoning() {
    let item = ResponseItem::Reasoning {
        id: Some("reason_1".to_string()),
        content: vec![
            ReasoningContent::Summary {
                text: "Brief summary".to_string(),
            },
            ReasoningContent::Thinking {
                text: "Deep thoughts...".to_string(),
            },
        ],
    };

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("reasoning"));
    assert!(json.contains("summary"));
    assert!(json.contains("thinking"));
}

#[test]
fn test_response_item_file_citation() {
    let item = ResponseItem::FileCitation {
        file_id: "file_abc".to_string(),
        quote: Some("relevant excerpt".to_string()),
    };

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("file_citation"));
}

#[test]
fn test_response_item_web_search_result() {
    let item = ResponseItem::WebSearchResult {
        id: "search_1".to_string(),
        url: "https://rust-lang.org".to_string(),
        title: "Rust Programming Language".to_string(),
        snippet: Some("A language empowering everyone...".to_string()),
    };

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("web_search_result"));
    assert!(json.contains("rust-lang.org"));
}

#[test]
fn test_response_item_get_text_content_multiple() {
    let item = ResponseItem::Message {
        id: None,
        parent_id: None,
        role: "assistant".to_string(),
        content: vec![
            ContentItem::output_text("First part. "),
            ContentItem::output_text("Second part."),
        ],
    };

    assert_eq!(
        item.get_text_content(),
        Some("First part. Second part.".to_string())
    );
}

#[test]
fn test_response_item_get_text_content_mixed() {
    let item = ResponseItem::Message {
        id: None,
        parent_id: None,
        role: "assistant".to_string(),
        content: vec![
            ContentItem::output_text("Text here"),
            ContentItem::InputImage {
                image_url: "https://example.com/img.png".to_string(),
                detail: None,
            },
            ContentItem::output_text(" more text"),
        ],
    };

    assert_eq!(
        item.get_text_content(),
        Some("Text here more text".to_string())
    );
}

#[test]
fn test_response_item_get_text_content_empty() {
    let item = ResponseItem::Message {
        id: None,
        parent_id: None,
        role: "assistant".to_string(),
        content: vec![],
    };

    assert_eq!(item.get_text_content(), None);
}

#[test]
fn test_reasoning_content_variants() {
    let summary = ReasoningContent::Summary {
        text: "Summary of thought process".to_string(),
    };

    let thinking = ReasoningContent::Thinking {
        text: "Detailed reasoning...".to_string(),
    };

    let summary_json = serde_json::to_string(&summary).expect("serialize");
    let thinking_json = serde_json::to_string(&thinking).expect("serialize");

    assert!(summary_json.contains("\"type\":\"summary\""));
    assert!(thinking_json.contains("\"type\":\"thinking\""));
}

#[test]
fn test_local_shell_action_exec() {
    let action = LocalShellAction::Exec(LocalShellExecAction {
        command: vec!["ls".to_string(), "-la".to_string()],
        workdir: Some("/project".to_string()),
        timeout_ms: Some(30000),
    });

    let json = serde_json::to_string(&action).expect("serialize");
    assert!(json.contains("exec"));
    assert!(json.contains("ls"));
}

#[test]
fn test_local_shell_action_read_file() {
    let action = LocalShellAction::ReadFile {
        path: "/etc/hosts".to_string(),
    };

    let json = serde_json::to_string(&action).expect("serialize");
    assert!(json.contains("read_file"));
}

#[test]
fn test_local_shell_action_write_file() {
    let action = LocalShellAction::WriteFile {
        path: "/tmp/test.txt".to_string(),
        content: "Hello, world!".to_string(),
    };

    let json = serde_json::to_string(&action).expect("serialize");
    assert!(json.contains("write_file"));
}

#[test]
fn test_local_shell_action_list_dir() {
    let action = LocalShellAction::ListDir {
        path: "/home".to_string(),
    };

    let json = serde_json::to_string(&action).expect("serialize");
    assert!(json.contains("list_dir"));
}

#[test]
fn test_local_shell_status_success() {
    let status = LocalShellStatus::Success {
        exit_code: 0,
        stdout: "output".to_string(),
        stderr: "".to_string(),
    };

    let json = serde_json::to_string(&status).expect("serialize");
    assert!(json.contains("success"));
    assert!(json.contains("\"exit_code\":0"));
}

#[test]
fn test_local_shell_status_error() {
    let status = LocalShellStatus::Error {
        message: "Command failed".to_string(),
        exit_code: Some(1),
    };

    let json = serde_json::to_string(&status).expect("serialize");
    assert!(json.contains("error"));
}

#[test]
fn test_local_shell_status_timeout() {
    let status = LocalShellStatus::Timeout {
        stdout: "partial output".to_string(),
        stderr: "".to_string(),
    };

    let json = serde_json::to_string(&status).expect("serialize");
    assert!(json.contains("timeout"));
}

#[test]
fn test_local_shell_status_pending_approval() {
    let status = LocalShellStatus::PendingApproval;

    let json = serde_json::to_string(&status).expect("serialize");
    assert!(json.contains("pending_approval"));
}

#[test]
fn test_local_shell_status_denied() {
    let status = LocalShellStatus::Denied;

    let json = serde_json::to_string(&status).expect("serialize");
    assert!(json.contains("denied"));
}

#[test]
fn test_content_item_tool_result_error() {
    let item = ContentItem::ToolResult {
        tool_use_id: "tool_err".to_string(),
        content: "Error: File not found".to_string(),
        is_error: true,
    };

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("is_error"));
    assert!(json.contains("true"));
}

#[test]
fn test_local_shell_exec_action_minimal() {
    let action = LocalShellExecAction {
        command: vec!["pwd".to_string()],
        workdir: None,
        timeout_ms: None,
    };

    let json = serde_json::to_string(&action).expect("serialize");
    // Optional fields should be omitted
    assert!(!json.contains("workdir") || json.contains("null"));
}
