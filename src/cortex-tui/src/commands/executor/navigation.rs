//! Navigation command handlers (diff, scroll, goto, etc.)

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

impl CommandExecutor {
    pub(super) fn cmd_diff(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(file) => CommandResult::Async(format!("diff:{}", file)),
            None => CommandResult::Async("diff".to_string()),
        }
    }

    pub(super) fn cmd_scroll(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some("top") => CommandResult::SetValue("scroll".to_string(), "top".to_string()),
            Some("bottom") => CommandResult::SetValue("scroll".to_string(), "bottom".to_string()),
            Some(n) => CommandResult::SetValue("scroll".to_string(), n.to_string()),
            None => CommandResult::OpenModal(ModalType::Form("scroll".to_string())),
        }
    }

    pub(super) fn cmd_goto(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(n) => {
                if n.parse::<usize>().is_ok() {
                    CommandResult::SetValue("goto".to_string(), n.to_string())
                } else {
                    CommandResult::Error(format!("Invalid message number: {}", n))
                }
            }
            None => CommandResult::OpenModal(ModalType::Form("goto".to_string())),
        }
    }
}
