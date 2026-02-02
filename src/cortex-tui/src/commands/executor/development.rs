//! Development & tools command handlers (bug, plugins, delegates, spec, bg_process, review, etc.)

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

impl CommandExecutor {
    pub(super) fn cmd_bug(&self, cmd: &ParsedCommand) -> CommandResult {
        if cmd.args.is_empty() {
            CommandResult::OpenModal(ModalType::Form("bug".to_string()))
        } else {
            CommandResult::Async(format!("bug:report:{}", cmd.args_string()))
        }
    }

    /// Handles the /plugins command for managing plugins.
    ///
    /// Supports:
    /// - `/plugins` or `/plugins list` - List all installed plugins
    /// - `/plugins info <plugin-id>` - Show detailed info about a plugin
    /// - `/plugins enable <plugin-id>` - Enable a disabled plugin
    /// - `/plugins disable <plugin-id>` - Disable a plugin
    /// - `/plugins install <path>` - Install a plugin from path
    /// - `/plugins uninstall <plugin-id>` - Uninstall a plugin
    /// - `/plugins reload` - Reload all plugins
    /// - `/plugins reload <plugin-id>` - Reload a specific plugin
    /// - `/plugins create` - Create a new plugin from template
    pub(super) fn cmd_plugins(&self, cmd: &ParsedCommand) -> CommandResult {
        let action = cmd.first_arg().unwrap_or("list");

        match action {
            "list" | "ls" => CommandResult::Async("plugins:list".to_string()),

            "info" | "show" => {
                if let Some(plugin_id) = cmd.args.get(1) {
                    CommandResult::Async(format!("plugins:info:{}", plugin_id))
                } else {
                    CommandResult::Error("Usage: /plugins info <plugin-id>".to_string())
                }
            }

            "enable" => {
                if let Some(plugin_id) = cmd.args.get(1) {
                    CommandResult::Async(format!("plugins:enable:{}", plugin_id))
                } else {
                    CommandResult::Error("Usage: /plugins enable <plugin-id>".to_string())
                }
            }

            "disable" => {
                if let Some(plugin_id) = cmd.args.get(1) {
                    CommandResult::Async(format!("plugins:disable:{}", plugin_id))
                } else {
                    CommandResult::Error("Usage: /plugins disable <plugin-id>".to_string())
                }
            }

            "install" | "add" => {
                if let Some(path) = cmd.args.get(1) {
                    CommandResult::Async(format!("plugins:install:{}", path))
                } else {
                    CommandResult::Error("Usage: /plugins install <path-to-plugin>".to_string())
                }
            }

            "uninstall" | "remove" | "rm" => {
                if let Some(plugin_id) = cmd.args.get(1) {
                    CommandResult::Async(format!("plugins:uninstall:{}", plugin_id))
                } else {
                    CommandResult::Error("Usage: /plugins uninstall <plugin-id>".to_string())
                }
            }

            "reload" | "refresh" => {
                if let Some(plugin_id) = cmd.args.get(1) {
                    CommandResult::Async(format!("plugins:reload:{}", plugin_id))
                } else {
                    CommandResult::Async("plugins:reload".to_string())
                }
            }

            "create" | "new" | "init" => {
                if let Some(name) = cmd.args.get(1) {
                    CommandResult::Async(format!("plugins:create:{}", name))
                } else {
                    CommandResult::Async("plugins:create".to_string())
                }
            }

            "commands" | "cmds" => {
                // List commands provided by plugins
                CommandResult::Async("plugins:commands".to_string())
            }

            "hooks" => {
                // List hooks registered by plugins
                CommandResult::Async("plugins:hooks".to_string())
            }

            _ => {
                // Unknown action - might be a plugin ID, try to show info
                if action
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    CommandResult::Async(format!("plugins:info:{}", action))
                } else {
                    CommandResult::Error(format!(
                        "Unknown plugins action: '{}'. Use: list, info, enable, disable, install, uninstall, reload, create",
                        action
                    ))
                }
            }
        }
    }

    pub(super) fn cmd_delegates(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some("create") => CommandResult::Async("delegates:create".to_string()),
            Some("list") => CommandResult::Async("delegates:list".to_string()),
            Some("reload") => CommandResult::Async("delegates:reload".to_string()),
            None => CommandResult::Async("delegates:list".to_string()),
            Some(other) => CommandResult::Error(format!(
                "Unknown delegates action: {}. Use: create, list, reload",
                other
            )),
        }
    }

    pub(super) fn cmd_spec(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some("off") | Some("false") => {
                CommandResult::SetValue("spec_mode".to_string(), "false".to_string())
            }
            Some("on") | Some("true") | None => {
                CommandResult::SetValue("spec_mode".to_string(), "true".to_string())
            }
            Some(other) => {
                CommandResult::Error(format!("Invalid spec mode value: {}. Use on|off", other))
            }
        }
    }

    pub(super) fn cmd_bg_process(&self, cmd: &ParsedCommand) -> CommandResult {
        let action = cmd.first_arg().unwrap_or("list");
        match action {
            "list" => CommandResult::Async("bg:list".to_string()),
            "start" => {
                if let Some(target) = cmd.args.get(1) {
                    CommandResult::Async(format!("bg:start:{}", target))
                } else {
                    CommandResult::Error("Usage: /bg-process start <command>".to_string())
                }
            }
            "stop" | "kill" => {
                if let Some(target) = cmd.args.get(1) {
                    CommandResult::Async(format!("bg:stop:{}", target))
                } else {
                    CommandResult::Error("Usage: /bg-process stop <pid>".to_string())
                }
            }
            _ => CommandResult::Error(format!(
                "Unknown action: {}. Use: list, start, stop, kill",
                action
            )),
        }
    }

    pub(super) fn cmd_review(&self, cmd: &ParsedCommand) -> CommandResult {
        let target = cmd.first_arg().unwrap_or("uncommitted");
        // Check for --base flag
        let base_branch = cmd
            .args
            .iter()
            .find(|a| a.starts_with("--base="))
            .map(|a| a.trim_start_matches("--base="));

        match base_branch {
            Some(base) => CommandResult::Async(format!("review:{}:base={}", target, base)),
            None => CommandResult::Async(format!("review:{}", target)),
        }
    }

    pub(super) fn cmd_experimental(&self, cmd: &ParsedCommand) -> CommandResult {
        let feature = cmd.first_arg();
        let enable = cmd.args.iter().any(|a| a == "--enable");
        let disable = cmd.args.iter().any(|a| a == "--disable");

        match (feature, enable, disable) {
            (Some(f), true, false) => CommandResult::Async(format!("experimental:enable:{}", f)),
            (Some(f), false, true) => CommandResult::Async(format!("experimental:disable:{}", f)),
            (Some(f), false, false) => CommandResult::Async(format!("experimental:status:{}", f)),
            (None, _, _) => CommandResult::Async("experimental:list".to_string()),
            _ => CommandResult::Error("Cannot use both --enable and --disable".to_string()),
        }
    }

    pub(super) fn cmd_ghost(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some("list") => CommandResult::Async("ghost:list".to_string()),
            Some("cleanup") => CommandResult::Async("ghost:cleanup".to_string()),
            Some("status") | None => CommandResult::Async("ghost:status".to_string()),
            Some(other) => CommandResult::Error(format!(
                "Unknown ghost action: {}. Use: status, list, cleanup",
                other
            )),
        }
    }

    pub(super) fn cmd_multiedit(&self, cmd: &ParsedCommand) -> CommandResult {
        if cmd.args.len() < 2 {
            return CommandResult::Error(
                "Usage: /multiedit <pattern> <replacement> [--glob=pattern]".to_string(),
            );
        }

        let pattern = &cmd.args[0];
        let replacement = &cmd.args[1];

        // Check for --glob flag
        let glob = cmd
            .args
            .iter()
            .find(|a| a.starts_with("--glob="))
            .map(|a| a.trim_start_matches("--glob="));

        match glob {
            Some(g) => {
                CommandResult::Async(format!("multiedit:{}:{}:glob={}", pattern, replacement, g))
            }
            None => CommandResult::Async(format!("multiedit:{}:{}", pattern, replacement)),
        }
    }

    pub(super) fn cmd_diagnostics(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some(file) => CommandResult::Async(format!("diagnostics:{}", file)),
            None => CommandResult::Async("diagnostics:all".to_string()),
        }
    }

    pub(super) fn cmd_hooks(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some("list") | None => CommandResult::Async("hooks:list".to_string()),
            Some("run") => CommandResult::Async("hooks:run".to_string()),
            Some("enable") => CommandResult::Async("hooks:enable".to_string()),
            Some("disable") => CommandResult::Async("hooks:disable".to_string()),
            Some(other) => CommandResult::Error(format!(
                "Unknown hooks action: {}. Use: list, run, enable, disable",
                other
            )),
        }
    }

    pub(super) fn cmd_custom_commands(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.first_arg() {
            Some("create") => CommandResult::Async("custom-commands:create".to_string()),
            Some("list") | None => CommandResult::Async("custom-commands:list".to_string()),
            Some("reload") => CommandResult::Async("custom-commands:reload".to_string()),
            Some(other) => CommandResult::Error(format!(
                "Unknown action: {}. Use: create, list, reload",
                other
            )),
        }
    }
}
