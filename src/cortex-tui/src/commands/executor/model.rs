//! Model command handlers (model, approval, sandbox, auto, provider, temperature, tokens, etc.)

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

impl CommandExecutor {
    pub(super) fn cmd_model(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(model) => CommandResult::SetValue("model".to_string(), model.to_string()),
            None => CommandResult::OpenModal(ModalType::ModelPicker),
        }
    }

    pub(super) fn cmd_approval(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(mode) => match mode.to_lowercase().as_str() {
                "ask" | "always" | "session" | "never" => {
                    CommandResult::SetValue("approval".to_string(), mode.to_string())
                }
                _ => CommandResult::Error(format!(
                    "Invalid approval mode: {}. Use ask|session|always|never",
                    mode
                )),
            },
            None => CommandResult::OpenModal(ModalType::ApprovalPicker),
        }
    }

    pub(super) fn cmd_sandbox(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some("on") | Some("true") => {
                CommandResult::SetValue("sandbox".to_string(), "true".to_string())
            }
            Some("off") | Some("false") => {
                CommandResult::SetValue("sandbox".to_string(), "false".to_string())
            }
            None => CommandResult::Toggle("sandbox".to_string()),
            Some(other) => {
                CommandResult::Error(format!("Invalid sandbox value: {}. Use on|off", other))
            }
        }
    }

    pub(super) fn cmd_auto(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some("on") | Some("true") => {
                CommandResult::SetValue("auto".to_string(), "true".to_string())
            }
            Some("off") | Some("false") => {
                CommandResult::SetValue("auto".to_string(), "false".to_string())
            }
            None => CommandResult::Toggle("auto".to_string()),
            Some(other) => {
                CommandResult::Error(format!("Invalid auto value: {}. Use on|off", other))
            }
        }
    }

    pub(super) fn cmd_provider(&self, _cmd: &ParsedCommand) -> CommandResult {
        // Provider command is deprecated - Cortex is now a unified platform
        CommandResult::Message(
            "Warning: The /provider command is deprecated.\n\n\
             Cortex is now a unified platform - all model access goes through the Cortex backend.\n\
             Use /model to switch between available models instead."
                .to_string(),
        )
    }

    pub(super) fn cmd_providers(&self) -> CommandResult {
        // Providers command is deprecated - Cortex is now a unified platform
        CommandResult::Message(
            "Warning: The /providers command is deprecated.\n\n\
             Cortex is now a unified platform with a single backend.\n\
             Use /models to see available models instead."
                .to_string(),
        )
    }

    pub(super) fn cmd_temperature(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(temp) => {
                if let Ok(t) = temp.parse::<f32>() {
                    if (0.0..=2.0).contains(&t) {
                        CommandResult::SetValue("temperature".to_string(), temp.to_string())
                    } else {
                        CommandResult::Error("Temperature must be between 0.0 and 2.0".to_string())
                    }
                } else {
                    CommandResult::Error(format!("Invalid temperature: {}", temp))
                }
            }
            None => CommandResult::OpenModal(ModalType::Form("temperature".to_string())),
        }
    }

    pub(super) fn cmd_tokens(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(n) => {
                if n.parse::<u32>().is_ok() {
                    CommandResult::SetValue("max_tokens".to_string(), n.to_string())
                } else {
                    CommandResult::Error(format!("Invalid token count: {}", n))
                }
            }
            None => CommandResult::OpenModal(ModalType::Form("tokens".to_string())),
        }
    }
}
