//! Tests for SubmissionBuilder.

use cortex_protocol::{Op, ReviewDecision, UserInput};
use uuid::Uuid;

use super::SubmissionBuilder;

#[test]
fn test_builder_new_generates_uuid() {
    let builder1 = SubmissionBuilder::new();
    let builder2 = SubmissionBuilder::new();
    assert_ne!(builder1.id(), builder2.id());
    assert!(Uuid::parse_str(builder1.id()).is_ok());
}

#[test]
fn test_builder_with_id() {
    let builder = SubmissionBuilder::with_id("custom-123");
    assert_eq!(builder.id(), "custom-123");
}

#[test]
fn test_user_message() {
    let submission = SubmissionBuilder::user_message("Hello, world!")
        .build()
        .expect("should build");

    match submission.op {
        Op::UserInput { items } => {
            assert_eq!(items.len(), 1);
            match &items[0] {
                UserInput::Text { text } => assert_eq!(text, "Hello, world!"),
                _ => panic!("Expected Text input"),
            }
        }
        _ => panic!("Expected UserInput op"),
    }
}

#[test]
fn test_user_input_multiple_items() {
    let items = vec![
        SubmissionBuilder::text_input("Analyze this:"),
        SubmissionBuilder::file_input("/path/to/file.rs"),
    ];
    let submission = SubmissionBuilder::user_input(items)
        .build()
        .expect("should build");

    match submission.op {
        Op::UserInput { items } => {
            assert_eq!(items.len(), 2);
        }
        _ => panic!("Expected UserInput op"),
    }
}

#[test]
fn test_text_input() {
    let input = SubmissionBuilder::text_input("test");
    match input {
        UserInput::Text { text } => assert_eq!(text, "test"),
        _ => panic!("Expected Text input"),
    }
}

#[test]
fn test_file_input() {
    let input = SubmissionBuilder::file_input("/path/to/file");
    match input {
        UserInput::File { path, content } => {
            assert_eq!(path, "/path/to/file");
            assert!(content.is_none());
        }
        _ => panic!("Expected File input"),
    }
}

#[test]
fn test_file_input_with_content() {
    let input = SubmissionBuilder::file_input_with_content("/path", "content");
    match input {
        UserInput::File { path, content } => {
            assert_eq!(path, "/path");
            assert_eq!(content, Some("content".to_string()));
        }
        _ => panic!("Expected File input"),
    }
}

#[test]
fn test_image_input() {
    let input = SubmissionBuilder::image_input("base64data", "image/png");
    match input {
        UserInput::Image { data, media_type } => {
            assert_eq!(data, "base64data");
            assert_eq!(media_type, "image/png");
        }
        _ => panic!("Expected Image input"),
    }
}

#[test]
fn test_image_url_input() {
    let input = SubmissionBuilder::image_url_input("https://example.com/image.png");
    match input {
        UserInput::ImageUrl { url, detail } => {
            assert_eq!(url, "https://example.com/image.png");
            assert!(detail.is_none());
        }
        _ => panic!("Expected ImageUrl input"),
    }
}

#[test]
fn test_interrupt() {
    let submission = SubmissionBuilder::interrupt()
        .build()
        .expect("should build");
    assert!(matches!(submission.op, Op::Interrupt));
}

#[test]
fn test_shutdown() {
    let submission = SubmissionBuilder::shutdown().build().expect("should build");
    assert!(matches!(submission.op, Op::Shutdown));
}

#[test]
fn test_compact() {
    let submission = SubmissionBuilder::compact().build().expect("should build");
    assert!(matches!(submission.op, Op::Compact));
}

#[test]
fn test_undo() {
    let submission = SubmissionBuilder::undo().build().expect("should build");
    assert!(matches!(submission.op, Op::Undo));
}

#[test]
fn test_redo() {
    let submission = SubmissionBuilder::redo().build().expect("should build");
    assert!(matches!(submission.op, Op::Redo));
}

#[test]
fn test_approve() {
    let submission = SubmissionBuilder::approve("call-123")
        .build()
        .expect("should build");
    match submission.op {
        Op::ExecApproval { id, decision } => {
            assert_eq!(id, "call-123");
            assert_eq!(decision, ReviewDecision::Approved);
        }
        _ => panic!("Expected ExecApproval op"),
    }
}

#[test]
fn test_approve_session() {
    let submission = SubmissionBuilder::approve_session("call-123")
        .build()
        .expect("should build");
    match submission.op {
        Op::ExecApproval { id, decision } => {
            assert_eq!(id, "call-123");
            assert_eq!(decision, ReviewDecision::ApprovedForSession);
        }
        _ => panic!("Expected ExecApproval op"),
    }
}

#[test]
fn test_deny() {
    let submission = SubmissionBuilder::deny("call-123")
        .build()
        .expect("should build");
    match submission.op {
        Op::ExecApproval { id, decision } => {
            assert_eq!(id, "call-123");
            assert_eq!(decision, ReviewDecision::Denied);
        }
        _ => panic!("Expected ExecApproval op"),
    }
}

#[test]
fn test_abort() {
    let submission = SubmissionBuilder::abort("call-123")
        .build()
        .expect("should build");
    match submission.op {
        Op::ExecApproval { id, decision } => {
            assert_eq!(id, "call-123");
            assert_eq!(decision, ReviewDecision::Abort);
        }
        _ => panic!("Expected ExecApproval op"),
    }
}

#[test]
fn test_patch_approval() {
    let submission = SubmissionBuilder::approve_patch("call-123")
        .build()
        .expect("should build");
    match submission.op {
        Op::PatchApproval { id, decision } => {
            assert_eq!(id, "call-123");
            assert_eq!(decision, ReviewDecision::Approved);
        }
        _ => panic!("Expected PatchApproval op"),
    }
}

#[test]
fn test_fork_session() {
    let submission = SubmissionBuilder::fork_session(Some("msg-123".to_string()), Some(5))
        .build()
        .expect("should build");
    match submission.op {
        Op::ForkSession {
            fork_point_message_id,
            message_index,
        } => {
            assert_eq!(fork_point_message_id, Some("msg-123".to_string()));
            assert_eq!(message_index, Some(5));
        }
        _ => panic!("Expected ForkSession op"),
    }
}

#[test]
fn test_fork_from_message() {
    let submission = SubmissionBuilder::fork_from_message("msg-456")
        .build()
        .expect("should build");
    match submission.op {
        Op::ForkSession {
            fork_point_message_id,
            message_index,
        } => {
            assert_eq!(fork_point_message_id, Some("msg-456".to_string()));
            assert!(message_index.is_none());
        }
        _ => panic!("Expected ForkSession op"),
    }
}

#[test]
fn test_fork_here() {
    let submission = SubmissionBuilder::fork_here()
        .build()
        .expect("should build");
    match submission.op {
        Op::ForkSession {
            fork_point_message_id,
            message_index,
        } => {
            assert!(fork_point_message_id.is_none());
            assert!(message_index.is_none());
        }
        _ => panic!("Expected ForkSession op"),
    }
}

#[test]
fn test_switch_agent() {
    let submission = SubmissionBuilder::switch_agent("coder")
        .build()
        .expect("should build");
    match submission.op {
        Op::SwitchAgent { name } => assert_eq!(name, "coder"),
        _ => panic!("Expected SwitchAgent op"),
    }
}

#[test]
fn test_share() {
    let submission = SubmissionBuilder::share().build().expect("should build");
    assert!(matches!(submission.op, Op::Share));
}

#[test]
fn test_unshare() {
    let submission = SubmissionBuilder::unshare().build().expect("should build");
    assert!(matches!(submission.op, Op::Unshare));
}

#[test]
fn test_reload_mcp_servers() {
    let submission = SubmissionBuilder::reload_mcp_servers()
        .build()
        .expect("should build");
    assert!(matches!(submission.op, Op::ReloadMcpServers));
}

#[test]
fn test_enable_mcp_server() {
    let submission = SubmissionBuilder::enable_mcp_server("test-server")
        .build()
        .expect("should build");
    match submission.op {
        Op::EnableMcpServer { name } => assert_eq!(name, "test-server"),
        _ => panic!("Expected EnableMcpServer op"),
    }
}

#[test]
fn test_disable_mcp_server() {
    let submission = SubmissionBuilder::disable_mcp_server("test-server")
        .build()
        .expect("should build");
    match submission.op {
        Op::DisableMcpServer { name } => assert_eq!(name, "test-server"),
        _ => panic!("Expected DisableMcpServer op"),
    }
}

#[test]
fn test_list_mcp_tools() {
    let submission = SubmissionBuilder::list_mcp_tools()
        .build()
        .expect("should build");
    assert!(matches!(submission.op, Op::ListMcpTools));
}

#[test]
fn test_add_to_history() {
    let submission = SubmissionBuilder::add_to_history("test entry")
        .build()
        .expect("should build");
    match submission.op {
        Op::AddToHistory { text } => assert_eq!(text, "test entry"),
        _ => panic!("Expected AddToHistory op"),
    }
}

#[test]
fn test_get_history_entry() {
    let submission = SubmissionBuilder::get_history_entry(5, 123)
        .build()
        .expect("should build");
    match submission.op {
        Op::GetHistoryEntryRequest { offset, log_id } => {
            assert_eq!(offset, 5);
            assert_eq!(log_id, 123);
        }
        _ => panic!("Expected GetHistoryEntryRequest op"),
    }
}

#[test]
fn test_list_custom_prompts() {
    let submission = SubmissionBuilder::list_custom_prompts()
        .build()
        .expect("should build");
    assert!(matches!(submission.op, Op::ListCustomPrompts));
}

#[test]
fn test_run_shell_command() {
    let submission = SubmissionBuilder::run_shell_command("ls -la")
        .build()
        .expect("should build");
    match submission.op {
        Op::RunUserShellCommand { command } => assert_eq!(command, "ls -la"),
        _ => panic!("Expected RunUserShellCommand op"),
    }
}

#[test]
fn test_get_session_timeline() {
    let submission = SubmissionBuilder::get_session_timeline()
        .build()
        .expect("should build");
    assert!(matches!(submission.op, Op::GetSessionTimeline));
}

#[test]
fn test_build_returns_none_for_empty_builder() {
    let builder = SubmissionBuilder::new();
    assert!(builder.build().is_none());
}

#[test]
#[should_panic(expected = "no operation set")]
fn test_build_expect_panics_for_empty_builder() {
    let builder = SubmissionBuilder::new();
    builder.build_expect();
}

#[test]
fn test_default() {
    let builder = SubmissionBuilder::default();
    assert!(Uuid::parse_str(builder.id()).is_ok());
}
