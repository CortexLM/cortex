//! File command handlers (add, remove, search, ls, mention, images, tree, etc.)

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

impl CommandExecutor {
    pub(super) fn cmd_add(&self, cmd: &ParsedCommand) -> CommandResult {
        if cmd.args.is_empty() {
            CommandResult::OpenModal(ModalType::FilePicker)
        } else {
            // Use args_string_quoted to preserve filenames with spaces
            CommandResult::Async(format!("add:{}", cmd.args_string_quoted()))
        }
    }

    pub(super) fn cmd_remove(&self, cmd: &ParsedCommand) -> CommandResult {
        if cmd.args.is_empty() {
            CommandResult::OpenModal(ModalType::Form("remove".to_string()))
        } else {
            // Use args_string_quoted to preserve filenames with spaces
            CommandResult::Async(format!("remove:{}", cmd.args_string_quoted()))
        }
    }

    pub(super) fn cmd_search(&self, cmd: &ParsedCommand) -> CommandResult {
        if cmd.args.is_empty() {
            CommandResult::OpenModal(ModalType::Form("search".to_string()))
        } else {
            CommandResult::Async(format!("search:{}", cmd.args_string()))
        }
    }

    pub(super) fn cmd_ls(&self, cmd: &ParsedCommand) -> CommandResult {
        let path = cmd.first_arg().unwrap_or(".");
        CommandResult::Async(format!("ls:{}", path))
    }

    pub(super) fn cmd_mention(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(item) => CommandResult::Async(format!("mention:{}", item)),
            None => CommandResult::OpenModal(ModalType::Form("mention".to_string())),
        }
    }

    pub(super) fn cmd_images(&self, cmd: &ParsedCommand) -> CommandResult {
        if cmd.args.is_empty() {
            CommandResult::Async("images:list".to_string())
        } else {
            CommandResult::Async(format!("images:add:{}", cmd.args.join(",")))
        }
    }

    pub(super) fn cmd_tree(&self, cmd: &ParsedCommand) -> CommandResult {
        let path = cmd.first_arg().unwrap_or(".");
        CommandResult::Async(format!("tree:{}", path))
    }
}
