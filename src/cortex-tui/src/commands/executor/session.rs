//! Session command handlers (resume, fork, rename, export, share, rewind, delete, etc.)

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

impl CommandExecutor {
    pub(super) fn cmd_resume(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(id) => CommandResult::ResumeSession(id.to_string()),
            None => CommandResult::OpenModal(ModalType::Sessions),
        }
    }

    pub(super) fn cmd_fork(&self, _cmd: &ParsedCommand) -> CommandResult {
        // Fork modal handles the name input
        CommandResult::OpenModal(ModalType::Fork)
    }

    pub(super) fn cmd_rename(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(name) => CommandResult::SetValue("session_name".to_string(), name.to_string()),
            None => CommandResult::OpenModal(ModalType::Form("rename".to_string())),
        }
    }

    pub(super) fn cmd_export(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(format) => match format.to_lowercase().as_str() {
                "md" | "markdown" => CommandResult::Async("export:markdown".to_string()),
                "json" => CommandResult::Async("export:json".to_string()),
                "txt" | "text" => CommandResult::Async("export:text".to_string()),
                _ => CommandResult::Error(format!(
                    "Unknown export format: {}. Use: md, json, or txt",
                    format
                )),
            },
            None => CommandResult::OpenModal(ModalType::Export(None)),
        }
    }

    /// Handles the /share command to generate a share link for the current session.
    ///
    /// Supports:
    /// - `/share` - Generate a share link with default duration (24h)
    /// - `/share 1h` - Generate a share link that expires in 1 hour
    /// - `/share 7d` - Generate a share link that expires in 7 days
    pub(super) fn cmd_share(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(duration) => CommandResult::Async(format!("share:{}", duration)),
            None => CommandResult::Async("share".to_string()),
        }
    }

    pub(super) fn cmd_rewind(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(steps) => {
                if let Ok(n) = steps.parse::<usize>() {
                    CommandResult::SetValue("rewind".to_string(), n.to_string())
                } else {
                    CommandResult::Error(format!("Invalid number: {}", steps))
                }
            }
            None => CommandResult::SetValue("rewind".to_string(), "1".to_string()),
        }
    }

    pub(super) fn cmd_delete(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(id) => {
                CommandResult::OpenModal(ModalType::Confirm(format!("Delete session {}?", id)))
            }
            None => CommandResult::OpenModal(ModalType::Form("delete".to_string())),
        }
    }
}
