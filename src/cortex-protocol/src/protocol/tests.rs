//! Tests for protocol types.

use super::*;

#[test]
fn test_serialize_sandbox_policy() {
    let policy = SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![std::path::PathBuf::from("/tmp/test")],
        network_access: true,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
    };

    let json = serde_json::to_string(&policy).expect("serialize");
    assert!(json.contains("workspace-write"));
    assert!(json.contains("/tmp/test"));
}

#[test]
fn test_serialize_event() {
    let event = Event {
        id: "1".to_string(),
        msg: EventMsg::TaskComplete(TaskCompleteEvent {
            last_agent_message: Some("Done!".to_string()),
        }),
    };

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("task_complete"));
    assert!(json.contains("Done!"));
}

#[test]
fn test_message_part_text() {
    let part = MessagePart::Text {
        content: "Hello, world!".to_string(),
        synthetic: None,
        ignored: None,
        metadata: None,
    };

    let json = serde_json::to_string(&part).expect("serialize");
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("Hello, world!"));

    let deserialized: MessagePart = serde_json::from_str(&json).expect("deserialize");
    match deserialized {
        MessagePart::Text { content, .. } => {
            assert_eq!(content, "Hello, world!");
        }
        _ => panic!("Expected Text part"),
    }
}

#[test]
fn test_message_part_tool() {
    let part = MessagePart::Tool {
        call_id: "call_123".to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({"path": "/tmp/test.txt"}),
        state: ToolState::Pending { raw: None },
        output: None,
        error: None,
        metadata: None,
    };

    let json = serde_json::to_string(&part).expect("serialize");
    assert!(json.contains("\"type\":\"tool\""));
    assert!(json.contains("call_123"));
    assert!(json.contains("read_file"));
    assert!(json.contains("pending"));
}

#[test]
fn test_tool_state_transitions() {
    // Test Pending state
    let pending = ToolState::Pending {
        raw: Some("raw args".to_string()),
    };
    let json = serde_json::to_string(&pending).expect("serialize");
    assert!(json.contains("pending"));

    // Test Running state
    let running = ToolState::Running {
        title: Some("Reading file...".to_string()),
        metadata: None,
    };
    let json = serde_json::to_string(&running).expect("serialize");
    assert!(json.contains("running"));

    // Test Completed state
    let completed = ToolState::Completed {
        title: "Read file".to_string(),
        metadata: serde_json::json!({"bytes": 100}),
        attachments: None,
    };
    let json = serde_json::to_string(&completed).expect("serialize");
    assert!(json.contains("completed"));

    // Test Error state
    let error = ToolState::Error {
        message: "File not found".to_string(),
        metadata: None,
    };
    let json = serde_json::to_string(&error).expect("serialize");
    assert!(json.contains("error"));
    assert!(json.contains("File not found"));
}

#[test]
fn test_message_part_reasoning() {
    let part = MessagePart::Reasoning {
        content: "Let me think about this...".to_string(),
        signature: Some("sig_123".to_string()),
        metadata: None,
    };

    let json = serde_json::to_string(&part).expect("serialize");
    assert!(json.contains("\"type\":\"reasoning\""));
    assert!(json.contains("think about"));
}

#[test]
fn test_message_part_subtask() {
    let part = MessagePart::Subtask {
        task_id: "task_1".to_string(),
        description: "Analyze codebase".to_string(),
        status: SubtaskStatus::Running,
        agent: Some("code-review".to_string()),
        prompt: None,
        command: None,
    };

    let json = serde_json::to_string(&part).expect("serialize");
    assert!(json.contains("\"type\":\"subtask\""));
    assert!(json.contains("running"));
    assert!(json.contains("Analyze codebase"));
}

#[test]
fn test_message_with_parts() {
    let session_id = crate::conversation_id::ConversationId::new();
    let mut message = MessageWithParts::user("msg_1".to_string(), session_id);

    // Add a text part
    message.add_text("part_1".to_string(), "Hello!".to_string());

    // Add a tool call
    message.add_tool_call(
        "part_2".to_string(),
        "call_123".to_string(),
        "read_file".to_string(),
        serde_json::json!({"path": "/tmp/test"}),
    );

    assert_eq!(message.parts.len(), 2);
    assert!(message.has_tool_calls());

    let text = message.get_text_content();
    assert_eq!(text, "Hello!");
}

#[test]
fn test_part_timing() {
    let mut timing = PartTiming::now();
    assert!(timing.end.is_none());
    assert!(timing.compacted.is_none());

    // Complete the timing
    timing.complete();
    assert!(timing.end.is_some());

    // Check duration is reasonable
    let duration = timing.duration_ms();
    assert!(duration.is_some());
    assert!(duration.unwrap() < 1000); // Less than 1 second
}

#[test]
fn test_part_updated_event() {
    let session_id = crate::conversation_id::ConversationId::new();
    let event = PartUpdatedEvent {
        session_id,
        message_id: "msg_1".to_string(),
        part_index: 0,
        part_id: "part_1".to_string(),
        part: MessagePart::Text {
            content: "Updated content".to_string(),
            synthetic: None,
            ignored: None,
            metadata: None,
        },
        timing: Some(PartTiming::now()),
    };

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("msg_1"));
    assert!(json.contains("Updated content"));
}

#[test]
fn test_part_delta_event() {
    let session_id = crate::conversation_id::ConversationId::new();
    let event = PartDeltaEvent {
        session_id,
        message_id: "msg_1".to_string(),
        part_index: 0,
        part_id: "part_1".to_string(),
        delta: PartDelta::Text {
            content: "Hello ".to_string(),
        },
    };

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("Hello "));
}

#[test]
fn test_message_complete_tool() {
    let session_id = crate::conversation_id::ConversationId::new();
    let mut message = MessageWithParts::assistant(
        "msg_1".to_string(),
        session_id,
        "parent_1".to_string(),
        "gpt-4".to_string(),
        "openai".to_string(),
    );

    // Add a tool call
    message.add_tool_call(
        "part_1".to_string(),
        "call_123".to_string(),
        "read_file".to_string(),
        serde_json::json!({"path": "/tmp/test"}),
    );

    // Complete the tool
    let completed = message.complete_tool(
        "call_123",
        "File contents".to_string(),
        "Read /tmp/test".to_string(),
        serde_json::json!({"bytes": 13}),
    );
    assert!(completed);

    // Verify state changed
    let tool_parts = message.get_tool_parts();
    assert_eq!(tool_parts.len(), 1);
    if let MessagePart::Tool { state, output, .. } = &tool_parts[0].part {
        assert!(matches!(state, ToolState::Completed { .. }));
        assert_eq!(output.as_deref(), Some("File contents"));
    }
}
