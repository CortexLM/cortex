//! Navigation and control commands.
//!
//! Commands for navigating conversation history and controlling the session:
//! - help, clear, compact, undo, exit
//! - sessions, resume, rewind, favorite

use async_trait::async_trait;

use crate::error::Result;

use super::types::{CommandContext, CommandHandler, CommandInvocation, CommandMeta, CommandResult};

/// Help command.
pub struct HelpCommand;

#[async_trait]
impl CommandHandler for HelpCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        if let Some(cmd_name) = invocation.arg(0) {
            // Show help for specific command
            Ok(CommandResult::success(format!(
                "Help for /{cmd_name} - Use /help for all commands"
            )))
        } else {
            // Show all commands
            let help = r#"Available Commands:

Navigation & Control:
  /help [command]    - Show help for commands
  /clear            - Clear conversation history
  /compact          - Compact context to save tokens
  /undo             - Undo last action
  /exit, /quit      - Exit Cortex

Information:
  /skills           - List available skills
  /plugins          - List installed plugins
  /model [name]     - Show or change current model
  /cost             - Show token usage and estimated cost
  /config           - Show configuration

Feedback:
  /bug              - Report a bug

Type /help <command> for detailed help on a specific command."#;

            Ok(CommandResult::success(help))
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("help", "Show available commands")
                .alias("h")
                .alias("?")
                .optional_arg("command", "Command to get help for")
                .category("Navigation")
        })
    }
}

/// Clear command.
pub struct ClearCommand;

#[async_trait]
impl CommandHandler for ClearCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        // This will be handled by the session to clear history
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "clear_history"
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("clear", "Clear conversation history").category("Navigation")
        })
    }
}

/// Compact command.
pub struct CompactCommand;

#[async_trait]
impl CommandHandler for CompactCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "compact_context"
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("compact", "Compact conversation context to save tokens")
                .category("Navigation")
        })
    }
}

/// Undo command.
pub struct UndoCommand;

#[async_trait]
impl CommandHandler for UndoCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "undo"
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("undo", "Undo the last action")
                .alias("u")
                .category("Navigation")
        })
    }
}

/// Exit command.
pub struct ExitCommand;

#[async_trait]
impl CommandHandler for ExitCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "exit"
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("exit", "Exit Cortex")
                .alias("quit")
                .alias("q")
                .category("Navigation")
        })
    }
}

/// Sessions command.
pub struct SessionsCommand;

#[async_trait]
impl CommandHandler for SessionsCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        let show_all = invocation.has_flag("all") || invocation.has_flag("a");
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "list_sessions",
            "show_all": show_all
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("sessions", "List previous sessions").category("Navigation")
        })
    }
}

/// Resume command.
pub struct ResumeCommand;

#[async_trait]
impl CommandHandler for ResumeCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        let session_id = invocation.arg(0).map(std::string::ToString::to_string);
        let last = invocation.has_flag("last") || invocation.has_flag("l");

        Ok(CommandResult::with_data(serde_json::json!({
            "action": "resume_session",
            "session_id": session_id,
            "last": last
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("resume", "Resume a previous session")
                .optional_arg("session_id", "Session ID or 'last'")
                .category("Navigation")
        })
    }
}

/// Rewind command.
pub struct RewindCommand;

#[async_trait]
impl CommandHandler for RewindCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        let target = invocation.arg(0).map(std::string::ToString::to_string);

        Ok(CommandResult::with_data(serde_json::json!({
            "action": "rewind",
            "target": target
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("rewind", "Rewind conversation to a previous point")
                .optional_arg("target", "Message ID or 'last'")
                .category("Navigation")
        })
    }
}

/// Favorite command.
pub struct FavoriteCommand;

#[async_trait]
impl CommandHandler for FavoriteCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "toggle_favorite"
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("favorite", "Toggle favorite status for current session")
                .alias("pin")
                .category("Navigation")
        })
    }
}
