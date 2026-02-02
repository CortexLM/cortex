//! Information commands.
//!
//! Commands for displaying information:
//! - skills, plugins, agents
//! - model, cost, config
//! - ratelimits

use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::Result;

use super::types::{CommandContext, CommandHandler, CommandInvocation, CommandMeta, CommandResult};

/// Skills command.
pub struct SkillsCommand;

#[async_trait]
impl CommandHandler for SkillsCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        ctx: &CommandContext,
    ) -> Result<CommandResult> {
        if let Some(ref skills_registry) = ctx.skills {
            let skills = skills_registry.list().await;

            if skills.is_empty() {
                return Ok(CommandResult::success(
                    "No skills installed.\n\nTo add skills:\n\
                     - .agents/<skill-name>/SKILL.md (project, agent.md format)\n\
                     - .agent/<skill-name>/SKILL.md (project, agent.md format)\n\
                     - .cortex/skills/<skill-name>/SKILL.md (project)\n\
                     - ~/.cortex/skills/<skill-name>/SKILL.md (personal)",
                ));
            }

            let mut output = String::from("Available Skills:\n\n");

            // Group by source
            let mut by_source: HashMap<String, Vec<_>> = HashMap::new();
            for skill in &skills {
                by_source
                    .entry(skill.source.to_string())
                    .or_default()
                    .push(skill);
            }

            for (source, skills) in by_source {
                output.push_str(&format!("{}:\n", source.to_uppercase()));
                for skill in skills {
                    output.push_str(&format!(
                        "  {} - {}\n",
                        skill.metadata.name, skill.metadata.description
                    ));
                }
                output.push('\n');
            }

            Ok(CommandResult::success(output))
        } else {
            Ok(CommandResult::success("Skills system not initialized"))
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("skills", "List available skills").category("Information")
        })
    }
}

/// Plugins command.
pub struct PluginsCommand;

#[async_trait]
impl CommandHandler for PluginsCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        ctx: &CommandContext,
    ) -> Result<CommandResult> {
        if let Some(ref plugins_registry) = ctx.plugins {
            let plugins = plugins_registry.list().await;

            if plugins.is_empty() {
                return Ok(CommandResult::success(
                    "No plugins installed.\n\nTo install plugins, add them to ~/.cortex/plugins/ or .cortex/plugins/",
                ));
            }

            let mut output = String::from("Installed Plugins:\n\n");

            for plugin in plugins {
                let status = if plugin.is_active() {
                    "active"
                } else {
                    "disabled"
                };
                output.push_str(&format!(
                    "  {} v{} [{}] - {}\n",
                    plugin.info.name, plugin.info.version, status, plugin.info.description
                ));
            }

            Ok(CommandResult::success(output))
        } else {
            Ok(CommandResult::success("Plugin system not initialized"))
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("plugins", "List installed plugins")
                .alias("plugin")
                .category("Information")
        })
    }
}

/// Agents command.
pub struct AgentsCommand;

#[async_trait]
impl CommandHandler for AgentsCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "list_agents"
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("agents", "List available agents").category("Information")
        })
    }
}

/// Config command.
pub struct ConfigCommand;

#[async_trait]
impl CommandHandler for ConfigCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        ctx: &CommandContext,
    ) -> Result<CommandResult> {
        let config_path = ctx.cortex_home.join("config.toml");

        match invocation.arg(0) {
            Some("edit") => Ok(CommandResult::with_data(serde_json::json!({
                "action": "edit_config",
                "path": config_path.to_string_lossy()
            }))),
            Some("permissions") | Some("perms") => {
                Ok(CommandResult::with_data(serde_json::json!({
                    "action": "show_permissions"
                })))
            }
            _ => {
                let output = format!(
                    "Configuration:\n\
                     Model: {}\n\
                     CWD: {}\n\
                     Cortex Home: {}\n\
                     Config File: {}\n\n\
                     Subcommands:\n\
                     /config edit        - Edit configuration file\n\
                     /config permissions - Show permission configuration\n\n\
                     Permission Configuration Example:\n\
                     [permission]\n\
                     edit = \"ask\"            # ask, allow, deny\n\
                     webfetch = \"allow\"\n\
                     doom_loop = \"ask\"\n\
                     external_directory = \"ask\"\n\n\
                     [permission.bash]\n\
                     \"git *\" = \"allow\"       # Pattern-based bash permissions\n\
                     \"npm *\" = \"allow\"\n\
                     \"rm -rf *\" = \"deny\"\n\
                     \"*\" = \"ask\"             # Default for unmatched\n\n\
                     [permission.skill]\n\
                     \"*\" = \"ask\"\n\
                     \"trusted-skill\" = \"allow\"",
                    ctx.model,
                    ctx.cwd.display(),
                    ctx.cortex_home.display(),
                    config_path.display()
                );
                Ok(CommandResult::success(output))
            }
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("config", "Show or edit configuration")
                .optional_arg("action", "Action: edit, permissions")
                .help(
                    "Manage configuration.\n\n\
                     Usage:\n\
                     /config             - Show configuration overview\n\
                     /config edit        - Open config file in editor\n\
                     /config permissions - Show permission settings\n\n\
                     Permission settings control what actions require approval.",
                )
                .category("Information")
        })
    }
}

/// Model command.
pub struct ModelCommand;

#[async_trait]
impl CommandHandler for ModelCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        ctx: &CommandContext,
    ) -> Result<CommandResult> {
        if let Some(model_name) = invocation.arg(0) {
            Ok(CommandResult::with_data(serde_json::json!({
                "action": "change_model",
                "model": model_name
            })))
        } else {
            Ok(CommandResult::success(format!(
                "Current model: {}",
                ctx.model
            )))
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("model", "Show or change the current model")
                .optional_arg("name", "Model name to switch to")
                .category("Information")
        })
    }
}

/// Cost command.
pub struct CostCommand;

#[async_trait]
impl CommandHandler for CostCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        ctx: &CommandContext,
    ) -> Result<CommandResult> {
        if let Some(ref usage) = ctx.token_usage {
            let output = format!(
                "Token Usage:\n\
                 Input tokens: {}\n\
                 Output tokens: {}\n\
                 Total tokens: {}",
                usage.input_tokens, usage.output_tokens, usage.total_tokens
            );
            Ok(CommandResult::success(output))
        } else {
            Ok(CommandResult::success("No token usage data available"))
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("cost", "Show token usage and estimated cost")
                .alias("tokens")
                .alias("usage")
                .category("Information")
        })
    }
}

/// Rate limits command.
pub struct RateLimitsCommand;

#[async_trait]
impl CommandHandler for RateLimitsCommand {
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "show_rate_limits"
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("ratelimits", "Show API rate limits and usage")
                .alias("limits")
                .alias("quota")
                .category("Information")
        })
    }
}
