//! General command handlers (help, version, theme, init, skill, etc.)

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

impl CommandExecutor {
    pub(super) fn cmd_help(&self, cmd: &ParsedCommand) -> CommandResult {
        let topic = cmd.first_arg().map(|s| s.to_string());
        CommandResult::OpenModal(ModalType::Help(topic))
    }

    pub(super) fn cmd_version(&self) -> CommandResult {
        CommandResult::Message(format!(
            "Cortex TUI v{}\ncortex-core v{}",
            env!("CARGO_PKG_VERSION"),
            cortex_core::VERSION
        ))
    }

    pub(super) fn cmd_theme(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(theme) => CommandResult::SetValue("theme".to_string(), theme.to_string()),
            None => CommandResult::OpenModal(ModalType::ThemePicker),
        }
    }

    /// Handles the /init command to initialize AGENTS.md in the project.
    ///
    /// Supports:
    /// - `/init` - Create AGENTS.md if it doesn't exist
    /// - `/init --force` - Overwrite existing AGENTS.md
    pub(super) fn cmd_init(&self, cmd: &ParsedCommand) -> CommandResult {
        let force = cmd.args.iter().any(|a| a == "--force" || a == "-f");
        if force {
            CommandResult::Async("init:force".to_string())
        } else {
            CommandResult::Async("init".to_string())
        }
    }

    /// Handles the /skill command to invoke a skill by name.
    ///
    /// Supports:
    /// - `/skill <name>` - Invoke a skill with no arguments
    /// - `/skill <name> <args...>` - Invoke a skill with arguments
    /// - `/skill <pattern>` - Invoke skills matching a pattern (e.g., code-*)
    pub(super) fn cmd_skill(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(name) => {
                // Validate skill name (basic input validation)
                if name.is_empty() {
                    return CommandResult::Error("Skill name cannot be empty.".to_string());
                }

                // Check for invalid characters in skill name
                let valid = name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '*');
                if !valid {
                    return CommandResult::Error(format!(
                        "Invalid skill name: '{}'. Names can only contain alphanumeric characters, hyphens, underscores, and wildcards.",
                        name
                    ));
                }

                // Build the async command with skill name and args
                if cmd.args.len() > 1 {
                    let skill_args = cmd.args[1..].join(" ");
                    CommandResult::Async(format!("skill:invoke:{}:{}", name, skill_args))
                } else {
                    CommandResult::Async(format!("skill:invoke:{}", name))
                }
            }
            None => {
                // No skill name provided - open skills picker modal
                CommandResult::OpenModal(ModalType::Skills)
            }
        }
    }
}
