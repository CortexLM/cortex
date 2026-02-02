//! Custom command types and definitions.
//!
//! Custom commands are user-defined prompt templates that can be invoked
//! via slash commands or CLI arguments. They support template variables
//! and can be configured per-project or globally.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A custom command definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomCommand {
    /// Command name (lowercase, hyphens allowed, used as /command-name).
    pub name: String,
    /// Brief description shown in autocomplete and help.
    pub description: String,
    /// Prompt template with variable placeholders.
    pub template: String,
    /// Optional agent/delegate to use for execution.
    #[serde(default)]
    pub agent: Option<String>,
    /// Optional model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Whether to run as a subtask (isolated context).
    #[serde(default)]
    pub subtask: bool,
    /// Optional category for grouping in help.
    #[serde(default)]
    pub category: Option<String>,
    /// Optional aliases for the command.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Source of the command.
    #[serde(skip)]
    pub source: CommandSource,
    /// Path to the source file (if loaded from file).
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

impl CustomCommand {
    /// Create a new custom command.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        template: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            template: template.into(),
            agent: None,
            model: None,
            subtask: false,
            category: None,
            aliases: Vec::new(),
            source: CommandSource::Config,
            source_path: None,
        }
    }

    /// Set the agent for this command.
    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    /// Set the model for this command.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set whether to run as subtask.
    pub fn with_subtask(mut self, subtask: bool) -> Self {
        self.subtask = subtask;
        self
    }

    /// Set the category.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Add an alias.
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    /// Validate the command definition.
    pub fn validate(&self) -> Result<(), String> {
        // Name validation
        if self.name.is_empty() {
            return Err("Command name cannot be empty".to_string());
        }

        if self.name.len() > 64 {
            return Err("Command name must be at most 64 characters".to_string());
        }

        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
        {
            return Err(
                "Command name must contain only lowercase letters, digits, and hyphens".to_string(),
            );
        }

        // Description validation
        if self.description.len() > 500 {
            return Err("Command description must be at most 500 characters".to_string());
        }

        // Template validation
        if self.template.is_empty() {
            return Err("Command template cannot be empty".to_string());
        }

        // Validate aliases
        for alias in &self.aliases {
            if !alias
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
            {
                return Err(format!(
                    "Alias '{alias}' must contain only lowercase letters, digits, and hyphens"
                ));
            }
        }

        Ok(())
    }
}

/// Source of a custom command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    /// Personal command from ~/.cortex/commands/
    Personal,
    /// Project command from .cortex/commands/
    Project,
    /// Defined in config.toml
    #[default]
    Config,
    /// Built-in command
    Builtin,
}

impl std::fmt::Display for CommandSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Personal => write!(f, "personal"),
            Self::Project => write!(f, "project"),
            Self::Config => write!(f, "config"),
            Self::Builtin => write!(f, "builtin"),
        }
    }
}

/// Command metadata from YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMetadata {
    /// Command name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Agent to use.
    #[serde(default)]
    pub agent: Option<String>,
    /// Model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Run as subtask.
    #[serde(default)]
    pub subtask: bool,
    /// Command category.
    #[serde(default)]
    pub category: Option<String>,
    /// Command aliases.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// TOML configuration for custom commands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomCommandConfig {
    /// Command name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Prompt template (inline).
    #[serde(default)]
    pub template: String,
    /// Agent to use.
    #[serde(default)]
    pub agent: Option<String>,
    /// Model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Run as subtask.
    #[serde(default)]
    pub subtask: bool,
    /// Category.
    #[serde(default)]
    pub category: Option<String>,
    /// Aliases.
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl From<CustomCommandConfig> for CustomCommand {
    fn from(config: CustomCommandConfig) -> Self {
        Self {
            name: config.name,
            description: config.description,
            template: config.template,
            agent: config.agent,
            model: config.model,
            subtask: config.subtask,
            category: config.category,
            aliases: config.aliases,
            source: CommandSource::Config,
            source_path: None,
        }
    }
}

/// Result of executing a custom command.
#[derive(Debug, Clone)]
pub struct CommandExecutionResult {
    /// The expanded prompt to send to the AI.
    pub prompt: String,
    /// Optional agent to use.
    pub agent: Option<String>,
    /// Optional model override.
    pub model: Option<String>,
    /// Whether to run as subtask.
    pub subtask: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_command_validation() {
        // Valid command
        let cmd = CustomCommand::new("review", "Review code", "Review this: {{input}}");
        assert!(cmd.validate().is_ok());

        // Empty name
        let cmd = CustomCommand::new("", "Test", "Template");
        assert!(cmd.validate().is_err());

        // Invalid name characters
        let cmd = CustomCommand::new("Review Code", "Test", "Template");
        assert!(cmd.validate().is_err());

        // Name too long
        let cmd = CustomCommand::new(&"a".repeat(65), "Test", "Template");
        assert!(cmd.validate().is_err());

        // Empty template
        let cmd = CustomCommand::new("test", "Test", "");
        assert!(cmd.validate().is_err());
    }

    #[test]
    fn test_command_builder() {
        let cmd = CustomCommand::new("deploy", "Deploy to production", "Deploy {{input}}")
            .with_agent("devops")
            .with_model("gpt-4")
            .with_subtask(true)
            .with_category("DevOps")
            .with_alias("d");

        assert_eq!(cmd.name, "deploy");
        assert_eq!(cmd.agent, Some("devops".to_string()));
        assert_eq!(cmd.model, Some("gpt-4".to_string()));
        assert!(cmd.subtask);
        assert_eq!(cmd.category, Some("DevOps".to_string()));
        assert_eq!(cmd.aliases, vec!["d".to_string()]);
    }
}
