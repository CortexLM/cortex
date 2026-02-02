//! Tests for the command executor.

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

#[test]
fn test_execute_quit() {
    let executor = CommandExecutor::new();
    let cmd = ParsedCommand::new("quit".to_string(), vec![], "/quit".to_string());
    assert!(matches!(executor.execute(&cmd), CommandResult::Quit));
}

#[test]
fn test_execute_quit_alias() {
    let executor = CommandExecutor::new();
    let cmd = ParsedCommand::new("q".to_string(), vec![], "/q".to_string());
    assert!(matches!(executor.execute(&cmd), CommandResult::Quit));
}

#[test]
fn test_execute_exit_alias() {
    let executor = CommandExecutor::new();
    let cmd = ParsedCommand::new("exit".to_string(), vec![], "/exit".to_string());
    assert!(matches!(executor.execute(&cmd), CommandResult::Quit));
}

#[test]
fn test_execute_help() {
    let executor = CommandExecutor::new();
    let cmd = ParsedCommand::new("help".to_string(), vec![], "/help".to_string());
    assert!(matches!(
        executor.execute(&cmd),
        CommandResult::OpenModal(ModalType::Help(None))
    ));
}

#[test]
fn test_execute_help_topic() {
    let executor = CommandExecutor::new();
    let cmd = ParsedCommand::new(
        "help".to_string(),
        vec!["keys".to_string()],
        "/help keys".to_string(),
    );
    if let CommandResult::OpenModal(ModalType::Help(Some(topic))) = executor.execute(&cmd) {
        assert_eq!(topic, "keys");
    } else {
        panic!("Expected Help modal with topic");
    }
}

#[test]
fn test_execute_unknown() {
    let executor = CommandExecutor::new();
    let cmd = ParsedCommand::new("foobar".to_string(), vec![], "/foobar".to_string());
    assert!(matches!(executor.execute(&cmd), CommandResult::NotFound(_)));
}

#[test]
fn test_execute_str() {
    let executor = CommandExecutor::new();
    assert!(matches!(executor.execute_str("/quit"), CommandResult::Quit));
    assert!(matches!(
        executor.execute_str("/clear"),
        CommandResult::Clear
    ));
}

#[test]
fn test_execute_str_invalid() {
    let executor = CommandExecutor::new();
    assert!(matches!(
        executor.execute_str("not a command"),
        CommandResult::Error(_)
    ));
}

#[test]
fn test_model_with_arg() {
    let executor = CommandExecutor::new();
    let result = executor.execute_str("/model gpt-4");
    if let CommandResult::SetValue(key, value) = result {
        assert_eq!(key, "model");
        assert_eq!(value, "gpt-4");
    } else {
        panic!("Expected SetValue");
    }
}

#[test]
fn test_model_without_arg() {
    let executor = CommandExecutor::new();
    let result = executor.execute_str("/model");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::ModelPicker)
    ));
}

#[test]
fn test_needs_args() {
    let executor = CommandExecutor::new();
    let result = executor.execute_str("/rename");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::Form(_))
    ));
}

#[test]
fn test_clear() {
    let executor = CommandExecutor::new();
    assert!(matches!(
        executor.execute_str("/clear"),
        CommandResult::Clear
    ));
    assert!(matches!(executor.execute_str("/cls"), CommandResult::Clear));
}

#[test]
fn test_new_session() {
    let executor = CommandExecutor::new();
    assert!(matches!(
        executor.execute_str("/new"),
        CommandResult::NewSession
    ));
    assert!(matches!(
        executor.execute_str("/n"),
        CommandResult::NewSession
    ));
}

#[test]
fn test_resume_with_id() {
    let executor = CommandExecutor::new();
    let result = executor.execute_str("/resume abc123");
    if let CommandResult::ResumeSession(id) = result {
        assert_eq!(id, "abc123");
    } else {
        panic!("Expected ResumeSession");
    }
}

#[test]
fn test_resume_without_id() {
    let executor = CommandExecutor::new();
    let result = executor.execute_str("/resume");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::Sessions)
    ));
}

#[test]
fn test_toggle_commands() {
    let executor = CommandExecutor::new();

    // compact
    let result = executor.execute_str("/compact");
    assert!(matches!(result, CommandResult::Toggle(ref s) if s == "compact"));

    // favorite
    let result = executor.execute_str("/favorite");
    assert!(matches!(result, CommandResult::Toggle(ref s) if s == "favorite"));

    // sandbox without arg
    let result = executor.execute_str("/sandbox");
    assert!(matches!(result, CommandResult::Toggle(ref s) if s == "sandbox"));
}

#[test]
fn test_sandbox_with_args() {
    let executor = CommandExecutor::new();

    let result = executor.execute_str("/sandbox on");
    assert!(matches!(
        result,
        CommandResult::SetValue(ref k, ref v) if k == "sandbox" && v == "true"
    ));

    let result = executor.execute_str("/sandbox off");
    assert!(matches!(
        result,
        CommandResult::SetValue(ref k, ref v) if k == "sandbox" && v == "false"
    ));

    let result = executor.execute_str("/sandbox invalid");
    assert!(matches!(result, CommandResult::Error(_)));
}

#[test]
fn test_temperature_validation() {
    let executor = CommandExecutor::new();

    // Valid
    let result = executor.execute_str("/temperature 0.7");
    assert!(matches!(
        result,
        CommandResult::SetValue(ref k, ref v) if k == "temperature" && v == "0.7"
    ));

    // Out of range
    let result = executor.execute_str("/temperature 3.0");
    assert!(matches!(result, CommandResult::Error(_)));

    // Invalid number
    let result = executor.execute_str("/temperature abc");
    assert!(matches!(result, CommandResult::Error(_)));

    // Missing arg
    let result = executor.execute_str("/temperature");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::Form(_))
    ));
}

#[test]
fn test_approval_modes() {
    let executor = CommandExecutor::new();

    for mode in &["ask", "session", "always", "never"] {
        let result = executor.execute_str(&format!("/approval {}", mode));
        assert!(matches!(
            result,
            CommandResult::SetValue(ref k, _) if k == "approval"
        ));
    }

    let result = executor.execute_str("/approval invalid");
    assert!(matches!(result, CommandResult::Error(_)));
}

#[test]
fn test_async_commands() {
    let executor = CommandExecutor::new();

    assert!(matches!(
        executor.execute_str("/undo"),
        CommandResult::Async(ref s) if s == "undo"
    ));

    assert!(matches!(
        executor.execute_str("/redo"),
        CommandResult::Async(ref s) if s == "redo"
    ));

    assert!(matches!(
        executor.execute_str("/transcript"),
        CommandResult::Async(ref s) if s == "transcript"
    ));

    assert!(matches!(
        executor.execute_str("/status"),
        CommandResult::Async(ref s) if s == "status"
    ));
}

#[test]
fn test_add_command() {
    let executor = CommandExecutor::new();

    // Without args opens file picker
    let result = executor.execute_str("/add");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::FilePicker)
    ));

    // With args triggers async add (space-separated, not comma-separated)
    let result = executor.execute_str("/add file1.txt file2.txt");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "add:file1.txt file2.txt"
    ));

    // With quoted args containing spaces
    let result = executor.execute_str("/add \"my file.txt\" other.txt");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "add:\"my file.txt\" other.txt"
    ));
}

#[test]
fn test_search_command() {
    let executor = CommandExecutor::new();

    let result = executor.execute_str("/search");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::Form(_))
    ));

    let result = executor.execute_str("/search pattern");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "search:pattern"
    ));
}

#[test]
fn test_mcp_subcommands() {
    let executor = CommandExecutor::new();

    // /mcp and /mcp list now open the MCP Manager modal
    let result = executor.execute_str("/mcp");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::McpManager)
    ));

    let result = executor.execute_str("/mcp list");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::McpManager)
    ));

    // All mcp subcommands now redirect to the interactive panel
    let result = executor.execute_str("/mcp status");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::McpManager)
    ));

    let result = executor.execute_str("/mcp add myserver");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::McpManager)
    ));

    let result = executor.execute_str("/mcp add");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::McpManager)
    ));
}

#[test]
fn test_rewind_command() {
    let executor = CommandExecutor::new();

    // Without arg defaults to 1
    let result = executor.execute_str("/rewind");
    assert!(matches!(
        result,
        CommandResult::SetValue(ref k, ref v) if k == "rewind" && v == "1"
    ));

    // With valid number
    let result = executor.execute_str("/rewind 5");
    assert!(matches!(
        result,
        CommandResult::SetValue(ref k, ref v) if k == "rewind" && v == "5"
    ));

    // With invalid number
    let result = executor.execute_str("/rewind abc");
    assert!(matches!(result, CommandResult::Error(_)));
}

#[test]
fn test_version() {
    let executor = CommandExecutor::new();
    let result = executor.execute_str("/version");
    if let CommandResult::Message(msg) = result {
        assert!(msg.contains("Cortex TUI"));
    } else {
        panic!("Expected Message");
    }
}

#[test]
fn test_registry_access() {
    let executor = CommandExecutor::new();
    let registry = executor.registry();
    assert!(registry.exists("help"));
    assert!(registry.exists("quit"));
}

#[test]
fn test_goto_validation() {
    let executor = CommandExecutor::new();

    let result = executor.execute_str("/goto 5");
    assert!(matches!(
        result,
        CommandResult::SetValue(ref k, ref v) if k == "goto" && v == "5"
    ));

    let result = executor.execute_str("/goto abc");
    assert!(matches!(result, CommandResult::Error(_)));

    let result = executor.execute_str("/goto");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::Form(_))
    ));
}

#[test]
fn test_scroll_command() {
    let executor = CommandExecutor::new();

    let result = executor.execute_str("/scroll top");
    assert!(matches!(
        result,
        CommandResult::SetValue(ref k, ref v) if k == "scroll" && v == "top"
    ));

    let result = executor.execute_str("/scroll bottom");
    assert!(matches!(
        result,
        CommandResult::SetValue(ref k, ref v) if k == "scroll" && v == "bottom"
    ));

    let result = executor.execute_str("/scroll 10");
    assert!(matches!(
        result,
        CommandResult::SetValue(ref k, ref v) if k == "scroll" && v == "10"
    ));

    let result = executor.execute_str("/scroll");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::Form(_))
    ));
}

#[test]
fn test_default_impl() {
    let executor = CommandExecutor::default();
    assert!(executor.registry().exists("help"));
}

#[test]
fn test_init_command() {
    let executor = CommandExecutor::new();

    let result = executor.execute_str("/init");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "init"
    ));

    let result = executor.execute_str("/init --force");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "init:force"
    ));

    let result = executor.execute_str("/init -f");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "init:force"
    ));
}

#[test]
fn test_commands_command() {
    let executor = CommandExecutor::new();

    let result = executor.execute_str("/commands");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "commands:list"
    ));
}

#[test]
fn test_agents_command() {
    let executor = CommandExecutor::new();

    let result = executor.execute_str("/agents");
    assert!(matches!(
        result,
        CommandResult::OpenModal(ModalType::Agents)
    ));
}

#[test]
fn test_share_command() {
    let executor = CommandExecutor::new();

    let result = executor.execute_str("/share");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "share"
    ));

    let result = executor.execute_str("/share 1h");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "share:1h"
    ));

    let result = executor.execute_str("/share 7d");
    assert!(matches!(
        result,
        CommandResult::Async(ref s) if s == "share:7d"
    ));
}
