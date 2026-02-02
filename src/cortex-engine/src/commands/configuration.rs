//! Configuration commands.
//!
//! Commands for managing settings and configuration:
//! - auto (autonomy level), spec (specification mode)
//! - delegates, experimental
//! - install-github-app, hooks
//! - custom-commands

use async_trait::async_trait;

use crate::error::Result;

use super::types::{CommandContext, CommandHandler, CommandInvocation, CommandMeta, CommandResult};

/// Delegates command.
pub struct DelegatesCommand;

#[async_trait]
impl CommandHandler for DelegatesCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        match invocation.arg(0) {
            Some("create") => Ok(CommandResult::with_data(serde_json::json!({
                "action": "create_delegate"
            }))),
            Some("list") | None => Ok(CommandResult::with_data(serde_json::json!({
                "action": "list_delegates"
            }))),
            Some("reload") => Ok(CommandResult::with_data(serde_json::json!({
                "action": "reload_delegates"
            }))),
            _ => Ok(CommandResult::error(
                "Unknown subcommand. Use: create, list, reload",
            )),
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("delegates", "Manage custom delegates (subagents)")
                .optional_arg("action", "Action: create, list, reload")
                .category("Configuration")
        })
    }
}

/// Auto command - control autonomy level.
pub struct AutoCommand;

#[async_trait]
impl CommandHandler for AutoCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        match invocation.arg(0) {
            Some("off") | Some("manual") => Ok(CommandResult::with_data(serde_json::json!({
                "action": "set_autonomy",
                "level": "manual"
            }))),
            Some("low") => Ok(CommandResult::with_data(serde_json::json!({
                "action": "set_autonomy",
                "level": "low"
            }))),
            Some("medium") | Some("med") => Ok(CommandResult::with_data(serde_json::json!({
                "action": "set_autonomy",
                "level": "medium"
            }))),
            Some("high") => Ok(CommandResult::with_data(serde_json::json!({
                "action": "set_autonomy",
                "level": "high"
            }))),
            None => {
                let help = r#"Autonomy Levels:

  /auto off      - Manual mode: approve all actions
  /auto low      - Auto-approve: file edits, read-only commands
  /auto medium   - + package installs, builds, git commits
  /auto high     - + all commands except dangerous patterns

Current level shown in status bar. Press Shift+Tab to cycle."#;
                Ok(CommandResult::success(help))
            }
            _ => Ok(CommandResult::error(
                "Unknown level. Use: off, low, medium, high",
            )),
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("auto", "Set autonomy level")
                .optional_arg("level", "Level: off, low, medium, high")
                .category("Configuration")
        })
    }
}

/// Spec command - specification mode.
pub struct SpecCommand;

#[async_trait]
impl CommandHandler for SpecCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        if invocation.has_flag("off") || invocation.arg(0) == Some("off") {
            Ok(CommandResult::with_data(serde_json::json!({
                "action": "spec_mode",
                "enabled": false
            })))
        } else {
            Ok(CommandResult::with_data(serde_json::json!({
                "action": "spec_mode",
                "enabled": true
            })))
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("spec", "Toggle specification mode (plan before executing)")
                .optional_arg("state", "off to disable")
                .category("Configuration")
        })
    }
}

/// GitHub App command.
pub struct InstallGithubAppCommand;

#[async_trait]
impl CommandHandler for InstallGithubAppCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "install_github_app"
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("install-github-app", "Install the Cortex GitHub App")
                .category("Configuration")
        })
    }
}

/// Experimental features command.
pub struct ExperimentalCommand;

#[async_trait]
impl CommandHandler for ExperimentalCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        let feature = invocation.arg(0);
        let action = if invocation.has_flag("enable") {
            "enable"
        } else if invocation.has_flag("disable") {
            "disable"
        } else {
            "list"
        };

        Ok(CommandResult::with_data(serde_json::json!({
            "action": "experimental_features",
            "feature": feature,
            "operation": action
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("experimental", "Manage experimental features")
                .alias("exp")
                .alias("features")
                .help("Enable or disable experimental features.\n\nUsage:\n  /experimental                    - Show all features\n  /experimental --enable ghost     - Enable ghost commits\n  /experimental --disable lsp      - Disable LSP integration")
                .optional_arg("feature", "Feature ID to toggle")
                .category("Configuration")
        })
    }
}

/// Hooks command.
pub struct HooksCommand;

#[async_trait]
impl CommandHandler for HooksCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        let action = invocation.arg(0).unwrap_or("list");

        Ok(CommandResult::with_data(serde_json::json!({
            "action": "hooks",
            "operation": action
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("hooks", "Manage file hooks (formatters, linters)")
                .help("Manage hooks that run on file changes.\n\nUsage:\n  /hooks           - List configured hooks\n  /hooks run       - Run hooks on current files")
                .optional_arg("action", "Action: list, run, enable, disable")
                .category("Development")
        })
    }
}

/// Custom commands management command.
pub struct CustomCommandsCommand;

#[async_trait]
impl CommandHandler for CustomCommandsCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        let action = invocation.arg(0).unwrap_or("list");

        match action {
            "list" => Ok(CommandResult::with_data(serde_json::json!({
                "action": "custom_commands",
                "operation": "list"
            }))),
            "create" => Ok(CommandResult::with_data(serde_json::json!({
                "action": "custom_commands",
                "operation": "create"
            }))),
            "reload" => Ok(CommandResult::with_data(serde_json::json!({
                "action": "custom_commands",
                "operation": "reload"
            }))),
            _ => Ok(CommandResult::error(
                "Unknown action. Use: list, create, reload",
            )),
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("custom-commands", "Manage custom commands")
                .alias("cc")
                .help("Manage custom prompt commands.\n\nUsage:\n  /custom-commands           - List all custom commands\n  /custom-commands create    - Create a new command\n  /custom-commands reload    - Reload commands from disk\n\nCustom commands are loaded from:\n  - Personal: ~/.cortex/commands/*.md\n  - Project: .cortex/commands/*.md\n  - Config: [[commands]] in config.toml")
                .optional_arg("action", "Action: list, create, reload")
                .category("Configuration")
        })
    }
}
