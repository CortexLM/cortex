//! Forge orchestration command.
//!
//! Commands for running validation agents and orchestration checks:
//! - /forge or /forge run - Run all validation agents
//! - /forge status - Show current validation status
//! - /forge config - Show configuration
//! - /forge agents - List available agents
//! - /forge check <agent> - Run specific agent only

use async_trait::async_trait;

use crate::error::Result;

use super::types::{CommandContext, CommandHandler, CommandInvocation, CommandMeta, CommandResult};

/// Forge orchestration command.
pub struct ForgeCommand;

#[async_trait]
impl CommandHandler for ForgeCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        let subcommand = invocation.arg(0).unwrap_or("run");

        match subcommand {
            "run" => {
                // Run all validation agents
                let fail_fast = invocation.has_flag("fail-fast") || invocation.has_flag("f");
                let verbose = invocation.has_flag("verbose") || invocation.has_flag("v");

                Ok(CommandResult::with_data(serde_json::json!({
                    "action": "run_forge",
                    "options": {
                        "fail_fast": fail_fast,
                        "verbose": verbose
                    }
                })))
            }
            "status" => {
                // Show current validation status
                Ok(CommandResult::with_data(serde_json::json!({
                    "action": "forge_status"
                })))
            }
            "config" => {
                // Show configuration
                let edit = invocation.has_flag("edit") || invocation.has_flag("e");
                let format = invocation.get("format").unwrap_or("text");

                Ok(CommandResult::with_data(serde_json::json!({
                    "action": "forge_config",
                    "options": {
                        "edit": edit,
                        "format": format
                    }
                })))
            }
            "agents" => {
                // List available agents
                let verbose = invocation.has_flag("verbose") || invocation.has_flag("v");

                Ok(CommandResult::with_data(serde_json::json!({
                    "action": "forge_agents",
                    "options": {
                        "verbose": verbose
                    }
                })))
            }
            "check" => {
                // Run specific agent only
                let agent_name = invocation.arg(1);

                match agent_name {
                    Some(name) => {
                        let verbose = invocation.has_flag("verbose") || invocation.has_flag("v");

                        Ok(CommandResult::with_data(serde_json::json!({
                            "action": "forge_check",
                            "agent": name,
                            "options": {
                                "verbose": verbose
                            }
                        })))
                    }
                    None => Ok(CommandResult::error(
                        "Usage: /forge check <agent>\n\n\
                         Run '/forge agents' to list available agents.\n\n\
                         Agents are loaded dynamically from .cortex/forge/agents/ directory.\n\
                         Each agent directory should contain a rules.toml configuration file.\n\n\
                         Example: /forge check security",
                    )),
                }
            }
            _ => {
                // Unknown subcommand, treat as run with the argument as potential path
                Ok(CommandResult::error(format!(
                    "Unknown subcommand: '{}'\n\n\
                     Available subcommands:\n  \
                     run      - Run all validation agents (default)\n  \
                     status   - Show current validation status\n  \
                     config   - Show or edit configuration\n  \
                     agents   - List available agents\n  \
                     check    - Run a specific agent\n\n\
                     Usage: /forge [subcommand] [options]",
                    subcommand
                )))
            }
        }
    }

    fn metadata(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| {
            CommandMeta::new("forge", "Run validation agents and orchestration checks")
                .alias("validate")
                .help(
                    "Forge is a validation orchestration system that runs specialized agents\n\
                     to analyze your codebase for security issues, code quality, and best practices.\n\n\
                     Subcommands:\n  \
                     /forge              - Run all validation agents\n  \
                     /forge run          - Same as above\n  \
                     /forge status       - Show current validation status\n  \
                     /forge config       - Show configuration\n  \
                     /forge agents       - List available agents\n  \
                     /forge check <name> - Run specific agent only\n\n\
                     Options:\n  \
                     --fail-fast, -f     - Stop on first error\n  \
                     --verbose, -v       - Show detailed output\n  \
                     --format=<fmt>      - Output format (text, json, markdown)\n\n\
                     Examples:\n  \
                     /forge                    - Run all agents\n  \
                     /forge check security     - Run only security agent\n  \
                     /forge config --edit      - Edit configuration",
                )
                .optional_arg("subcommand", "Action: run, status, config, agents, check")
                .category("Development")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn create_test_context() -> CommandContext {
        CommandContext {
            cwd: PathBuf::from("/test"),
            session_id: "test-session".to_string(),
            cortex_home: PathBuf::from("/home/.cortex"),
            model: "test-model".to_string(),
            token_usage: None,
            skills: None,
            plugins: None,
        }
    }

    #[tokio::test]
    async fn test_forge_default_run() {
        let cmd = ForgeCommand;
        let inv = CommandInvocation::parse("/forge").expect("should parse");
        let ctx = create_test_context();

        let result = cmd.execute(&inv, &ctx).await.expect("should execute");

        assert!(result.success);
        let data = result.data.expect("should have data");
        assert_eq!(data["action"], "run_forge");
    }

    #[tokio::test]
    async fn test_forge_run_explicit() {
        let cmd = ForgeCommand;
        let inv = CommandInvocation::parse("/forge run --fail-fast").expect("should parse");
        let ctx = create_test_context();

        let result = cmd.execute(&inv, &ctx).await.expect("should execute");

        assert!(result.success);
        let data = result.data.expect("should have data");
        assert_eq!(data["action"], "run_forge");
        assert_eq!(data["options"]["fail_fast"], true);
    }

    #[tokio::test]
    async fn test_forge_status() {
        let cmd = ForgeCommand;
        let inv = CommandInvocation::parse("/forge status").expect("should parse");
        let ctx = create_test_context();

        let result = cmd.execute(&inv, &ctx).await.expect("should execute");

        assert!(result.success);
        let data = result.data.expect("should have data");
        assert_eq!(data["action"], "forge_status");
    }

    #[tokio::test]
    async fn test_forge_config() {
        let cmd = ForgeCommand;
        let inv = CommandInvocation::parse("/forge config --edit").expect("should parse");
        let ctx = create_test_context();

        let result = cmd.execute(&inv, &ctx).await.expect("should execute");

        assert!(result.success);
        let data = result.data.expect("should have data");
        assert_eq!(data["action"], "forge_config");
        assert_eq!(data["options"]["edit"], true);
    }

    #[tokio::test]
    async fn test_forge_agents() {
        let cmd = ForgeCommand;
        let inv = CommandInvocation::parse("/forge agents -v").expect("should parse");
        let ctx = create_test_context();

        let result = cmd.execute(&inv, &ctx).await.expect("should execute");

        assert!(result.success);
        let data = result.data.expect("should have data");
        assert_eq!(data["action"], "forge_agents");
        assert_eq!(data["options"]["verbose"], true);
    }

    #[tokio::test]
    async fn test_forge_check_with_agent() {
        let cmd = ForgeCommand;
        let inv = CommandInvocation::parse("/forge check security").expect("should parse");
        let ctx = create_test_context();

        let result = cmd.execute(&inv, &ctx).await.expect("should execute");

        assert!(result.success);
        let data = result.data.expect("should have data");
        assert_eq!(data["action"], "forge_check");
        assert_eq!(data["agent"], "security");
    }

    #[tokio::test]
    async fn test_forge_check_without_agent() {
        let cmd = ForgeCommand;
        let inv = CommandInvocation::parse("/forge check").expect("should parse");
        let ctx = create_test_context();

        let result = cmd.execute(&inv, &ctx).await.expect("should execute");

        assert!(!result.success);
        assert!(result.error.is_some());
        let error = result.error.expect("should have error");
        assert!(error.contains("/forge agents") || error.contains("Usage"));
    }

    #[tokio::test]
    async fn test_forge_unknown_subcommand() {
        let cmd = ForgeCommand;
        let inv = CommandInvocation::parse("/forge unknown").expect("should parse");
        let ctx = create_test_context();

        let result = cmd.execute(&inv, &ctx).await.expect("should execute");

        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(
            result
                .error
                .expect("should have error")
                .contains("Unknown subcommand")
        );
    }

    #[test]
    fn test_forge_metadata() {
        let cmd = ForgeCommand;
        let meta = cmd.metadata();

        assert_eq!(meta.name, "forge");
        assert!(meta.aliases.contains(&"validate".to_string()));
        assert_eq!(meta.category, Some("Development".to_string()));
        assert!(meta.help.is_some());
    }
}
