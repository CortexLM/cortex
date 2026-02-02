//! Debug command handlers (debug, cfg, logs, dump, eval, etc.)

use chrono::Local;

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

impl CommandExecutor {
    pub(super) fn cmd_debug(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some("on") | Some("true") => {
                CommandResult::SetValue("debug".to_string(), "true".to_string())
            }
            Some("off") | Some("false") => {
                CommandResult::SetValue("debug".to_string(), "false".to_string())
            }
            None => CommandResult::Toggle("debug".to_string()),
            Some(other) => {
                CommandResult::Error(format!("Invalid debug value: {}. Use on|off", other))
            }
        }
    }

    pub(super) fn cmd_cfg(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(key) => CommandResult::Async(format!("config:get:{}", key)),
            None => CommandResult::Async("config:list".to_string()),
        }
    }

    pub(super) fn cmd_logs(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(level) => CommandResult::Async(format!("logs:{}", level)),
            None => CommandResult::OpenModal(ModalType::LogLevelPicker),
        }
    }

    pub(super) fn cmd_dump(&self, cmd: &ParsedCommand) -> CommandResult {
        let file = match cmd.first_arg() {
            Some(path) => path.to_string(),
            None => {
                // Generate unique filename with timestamp to prevent overwrites
                let timestamp = Local::now().format("%Y%m%d_%H%M%S");
                format!("session_dump_{}.json", timestamp)
            }
        };
        CommandResult::Async(format!("dump:{}", file))
    }

    pub(super) fn cmd_eval(&self, cmd: &ParsedCommand) -> CommandResult {
        if cmd.args.is_empty() {
            CommandResult::OpenModal(ModalType::Form("eval".to_string()))
        } else {
            CommandResult::Async(format!("eval:{}", cmd.args_string()))
        }
    }
}
