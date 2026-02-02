//! Command types and traits.
//!
//! This module contains the core types for the slash commands system:
//! - Argument definitions and types
//! - Command metadata
//! - Command invocation parsing
//! - Command execution results
//! - Handler trait

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Command argument definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandArg {
    /// Argument name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Whether the argument is required.
    #[serde(default)]
    pub required: bool,
    /// Default value.
    pub default: Option<String>,
    /// Argument type.
    #[serde(default)]
    pub arg_type: ArgType,
}

/// Argument types.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgType {
    #[default]
    String,
    Number,
    Boolean,
    Path,
    Choice,
}

/// Command metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMeta {
    /// Command name (without leading /).
    pub name: String,
    /// Aliases for the command.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Brief description.
    pub description: String,
    /// Detailed help text.
    pub help: Option<String>,
    /// Command arguments.
    #[serde(default)]
    pub args: Vec<CommandArg>,
    /// Whether command is hidden from help.
    #[serde(default)]
    pub hidden: bool,
    /// Category for grouping in help.
    pub category: Option<String>,
}

impl CommandMeta {
    /// Create a new command metadata.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            aliases: Vec::new(),
            description: description.into(),
            help: None,
            args: Vec::new(),
            hidden: false,
            category: None,
        }
    }

    /// Add an alias.
    pub fn alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    /// Set detailed help.
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Add a required argument.
    pub fn required_arg(mut self, name: impl Into<String>, desc: impl Into<String>) -> Self {
        self.args.push(CommandArg {
            name: name.into(),
            description: desc.into(),
            required: true,
            default: None,
            arg_type: ArgType::String,
        });
        self
    }

    /// Add an optional argument.
    pub fn optional_arg(mut self, name: impl Into<String>, desc: impl Into<String>) -> Self {
        self.args.push(CommandArg {
            name: name.into(),
            description: desc.into(),
            required: false,
            default: None,
            arg_type: ArgType::String,
        });
        self
    }

    /// Set category.
    pub fn category(mut self, cat: impl Into<String>) -> Self {
        self.category = Some(cat.into());
        self
    }

    /// Mark as hidden.
    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }
}

/// Parsed command invocation.
#[derive(Debug, Clone)]
pub struct CommandInvocation {
    /// Command name.
    pub name: String,
    /// Positional arguments.
    pub args: Vec<String>,
    /// Named arguments.
    pub named_args: HashMap<String, String>,
    /// Raw input string.
    pub raw: String,
}

impl CommandInvocation {
    /// Parse a command string.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();

        if !input.starts_with('/') {
            return None;
        }

        let input = &input[1..]; // Remove leading /
        let mut parts = input.split_whitespace();

        let name = parts.next()?.to_string();
        let mut args = Vec::new();
        let mut named_args = HashMap::new();

        for part in parts {
            if let Some(arg) = part.strip_prefix("--") {
                // Named argument: --key=value or --flag
                if let Some((key, value)) = arg.split_once('=') {
                    named_args.insert(key.to_string(), value.to_string());
                } else {
                    named_args.insert(arg.to_string(), "true".to_string());
                }
            } else if part.starts_with('-') && part.len() == 2 {
                // Short flag: -f
                named_args.insert(part[1..].to_string(), "true".to_string());
            } else {
                args.push(part.to_string());
            }
        }

        Some(Self {
            name,
            args,
            named_args,
            raw: input.to_string(),
        })
    }

    /// Get a positional argument.
    pub fn arg(&self, index: usize) -> Option<&str> {
        self.args.get(index).map(std::string::String::as_str)
    }

    /// Get a named argument.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.named_args.get(name).map(std::string::String::as_str)
    }

    /// Check if a flag is set.
    pub fn has_flag(&self, name: &str) -> bool {
        self.named_args.contains_key(name)
    }

    /// Get all arguments as a single string.
    pub fn rest(&self) -> String {
        self.args.join(" ")
    }
}

/// Result of executing a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    /// Whether the command succeeded.
    pub success: bool,
    /// Output message.
    pub message: Option<String>,
    /// Data output (for structured results).
    pub data: Option<serde_json::Value>,
    /// Whether to continue with normal processing.
    pub continue_processing: bool,
    /// Error message if failed.
    pub error: Option<String>,
}

impl CommandResult {
    /// Create a success result.
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: None,
            continue_processing: false,
            error: None,
        }
    }

    /// Create a success result with data.
    pub fn with_data(data: serde_json::Value) -> Self {
        Self {
            success: true,
            message: None,
            data: Some(data),
            continue_processing: false,
            error: None,
        }
    }

    /// Create an error result.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: None,
            data: None,
            continue_processing: false,
            error: Some(message.into()),
        }
    }

    /// Create a result that continues with AI processing.
    pub fn continue_with(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: None,
            continue_processing: true,
            error: None,
        }
    }

    /// Empty success result.
    pub fn ok() -> Self {
        Self {
            success: true,
            message: None,
            data: None,
            continue_processing: false,
            error: None,
        }
    }
}

/// Command context passed to handlers.
pub struct CommandContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Session ID.
    pub session_id: String,
    /// Cortex home directory.
    pub cortex_home: PathBuf,
    /// Current model.
    pub model: String,
    /// Token usage.
    pub token_usage: Option<TokenUsage>,
    /// Skills registry reference.
    pub skills: Option<Arc<crate::skills::SkillRegistry>>,
    /// Plugins registry reference.
    pub plugins: Option<Arc<crate::plugin::PluginRegistry>>,
}

/// Token usage info.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

/// Command handler trait.
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// Execute the command.
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        ctx: &CommandContext,
    ) -> Result<CommandResult>;

    /// Get command metadata.
    fn metadata(&self) -> &CommandMeta;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_parsing() {
        let inv = CommandInvocation::parse("/help").unwrap();
        assert_eq!(inv.name, "help");
        assert!(inv.args.is_empty());

        let inv = CommandInvocation::parse("/model gpt-4").unwrap();
        assert_eq!(inv.name, "model");
        assert_eq!(inv.arg(0), Some("gpt-4"));

        let inv = CommandInvocation::parse("/config --edit").unwrap();
        assert_eq!(inv.name, "config");
        assert!(inv.has_flag("edit"));

        let inv = CommandInvocation::parse("/bug --priority=high This is a bug").unwrap();
        assert_eq!(inv.name, "bug");
        assert_eq!(inv.get("priority"), Some("high"));
        assert!(inv.rest().contains("This is a bug"));
    }

    #[test]
    fn test_not_a_command() {
        assert!(CommandInvocation::parse("hello").is_none());
        assert!(CommandInvocation::parse("").is_none());
        assert!(CommandInvocation::parse("  no slash  ").is_none());
    }

    #[test]
    fn test_command_result() {
        let result = CommandResult::success("Done");
        assert!(result.success);
        assert_eq!(result.message, Some("Done".to_string()));

        let result = CommandResult::error("Failed");
        assert!(!result.success);
        assert_eq!(result.error, Some("Failed".to_string()));
    }
}
