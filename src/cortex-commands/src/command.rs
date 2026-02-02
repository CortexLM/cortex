//! Command structure and parsing.

use std::path::PathBuf;

use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur when working with commands.
#[derive(Debug, Error)]
pub enum CommandError {
    /// Invalid frontmatter format.
    #[error("Invalid frontmatter: {0}")]
    InvalidFrontmatter(String),

    /// Missing required field.
    #[error("Missing required field: {0}")]
    MissingField(String),

    /// YAML parsing error.
    #[error("YAML parsing error: {0}")]
    YamlError(#[from] serde_yaml::Error),

    /// Invalid placeholder format.
    #[error("Invalid placeholder format: {0}")]
    InvalidPlaceholder(String),
}

/// Configuration for a custom command, parsed from YAML frontmatter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandConfig {
    /// Human-readable description of the command.
    #[serde(default)]
    pub description: Option<String>,

    /// Specific agent to use for this command.
    #[serde(default)]
    pub agent: Option<String>,

    /// Specific model to use for this command.
    #[serde(default)]
    pub model: Option<String>,

    /// Whether to execute this command as a subtask.
    #[serde(default)]
    pub subtask: Option<bool>,

    /// Aliases for this command.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// A custom command loaded from a markdown file.
#[derive(Debug, Clone)]
pub struct Command {
    /// Name of the command (filename without extension).
    pub name: String,

    /// Configuration from frontmatter.
    pub config: CommandConfig,

    /// Template content (after frontmatter).
    pub template: String,

    /// Path to the source file.
    pub source_path: PathBuf,
}

impl Command {
    /// Create a new command from parsed components.
    pub fn new(
        name: impl Into<String>,
        config: CommandConfig,
        template: impl Into<String>,
        source_path: PathBuf,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            template: template.into(),
            source_path,
        }
    }

    /// Parse a command from file content.
    ///
    /// The content should be a markdown file with optional YAML frontmatter.
    pub fn parse(
        name: impl Into<String>,
        content: &str,
        source_path: PathBuf,
    ) -> Result<Self, CommandError> {
        let (config, template) = parse_frontmatter(content)?;
        Ok(Self::new(name, config, template, source_path))
    }

    /// Get the description of the command.
    pub fn description(&self) -> &str {
        self.config
            .description
            .as_deref()
            .unwrap_or("Custom command")
    }

    /// Get the hints (placeholders) expected by this command's template.
    pub fn hints(&self) -> Vec<String> {
        hints(&self.template)
    }

    /// Check if this command expects arguments.
    pub fn expects_arguments(&self) -> bool {
        !self.hints().is_empty()
    }

    /// Substitute placeholders in the template with the given arguments.
    pub fn substitute(&self, arguments: &str) -> String {
        substitute_placeholders(&self.template, arguments)
    }
}

/// Parse YAML frontmatter from markdown content.
///
/// Frontmatter is delimited by `---` at the start and end.
/// Returns the parsed config and the remaining template content.
fn parse_frontmatter(content: &str) -> Result<(CommandConfig, String), CommandError> {
    let content = content.trim();

    // Check if content starts with frontmatter delimiter
    if !content.starts_with("---") {
        // No frontmatter, entire content is the template
        return Ok((CommandConfig::default(), content.to_string()));
    }

    // Find the closing delimiter
    let rest = &content[3..];
    let end_delimiter = rest.find("\n---");

    match end_delimiter {
        Some(end_pos) => {
            let yaml_content = &rest[..end_pos].trim();
            let template = rest[end_pos + 4..].trim();

            // Parse YAML
            let config: CommandConfig = if yaml_content.is_empty() {
                CommandConfig::default()
            } else {
                serde_yaml::from_str(yaml_content)?
            };

            Ok((config, template.to_string()))
        }
        None => {
            // No closing delimiter found, treat as no frontmatter
            Err(CommandError::InvalidFrontmatter(
                "Missing closing '---' delimiter".to_string(),
            ))
        }
    }
}

/// Extract placeholder hints from a template.
///
/// Returns a list of unique placeholders found in the template.
/// Examples: `$ARGUMENTS`, `$1`, `$2`, etc.
pub fn hints(template: &str) -> Vec<String> {
    let mut found = Vec::new();

    // Match $ARGUMENTS
    if template.contains("$ARGUMENTS") {
        found.push("$ARGUMENTS".to_string());
    }

    // Match $1, $2, ..., $9, $10, etc.
    let re = Regex::new(r"\$(\d+)").unwrap_or_else(|_| {
        // Fallback simple check if regex fails
        Regex::new(r"\$\d").unwrap_or_else(|_| panic!("Regex compilation failed"))
    });

    let mut numbered: Vec<u32> = re
        .captures_iter(template)
        .filter_map(|cap| cap.get(1))
        .filter_map(|m| m.as_str().parse::<u32>().ok())
        .collect();

    numbered.sort_unstable();
    numbered.dedup();

    for n in numbered {
        found.push(format!("${n}"));
    }

    found
}

/// Substitute placeholders in a template with arguments.
///
/// - `$ARGUMENTS` is replaced with all arguments.
/// - `$1`, `$2`, etc. are replaced with individual arguments.
/// - The last numbered placeholder captures all remaining arguments.
pub fn substitute_placeholders(template: &str, arguments: &str) -> String {
    let mut result = template.to_string();

    // Replace $ARGUMENTS first
    result = result.replace("$ARGUMENTS", arguments);

    // Parse arguments (respecting quotes)
    let args: Vec<&str> = parse_arguments(arguments);

    // Find the highest numbered placeholder
    let max_placeholder = find_max_placeholder(&result);

    // Replace numbered placeholders
    for i in 1..=max_placeholder {
        let placeholder = format!("${i}");
        let replacement = if i == max_placeholder {
            // Last placeholder captures all remaining arguments
            if i as usize <= args.len() {
                args[(i as usize - 1)..].join(" ")
            } else {
                String::new()
            }
        } else {
            // Regular placeholder gets single argument
            args.get(i as usize - 1).copied().unwrap_or("").to_string()
        };

        result = result.replace(&placeholder, &replacement);
    }

    result
}

/// Parse arguments, respecting quoted strings.
fn parse_arguments(input: &str) -> Vec<&str> {
    let input = input.trim();
    if input.is_empty() {
        return Vec::new();
    }

    // Simple split by whitespace for now
    // Basic quote handling - enhanced parsing planned for future
    input.split_whitespace().collect()
}

/// Find the highest numbered placeholder in the template.
fn find_max_placeholder(template: &str) -> u32 {
    let re = Regex::new(r"\$(\d+)").unwrap_or_else(|_| {
        Regex::new(r"\$\d").unwrap_or_else(|_| panic!("Regex compilation failed"))
    });

    re.captures_iter(template)
        .filter_map(|cap| cap.get(1))
        .filter_map(|m| m.as_str().parse::<u32>().ok())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_with_yaml() {
        let content = r#"---
description: "Test command"
agent: "build"
model: "gpt-4o"
subtask: true
---

This is the template content."#;

        let (config, template) = parse_frontmatter(content).unwrap();

        assert_eq!(config.description, Some("Test command".to_string()));
        assert_eq!(config.agent, Some("build".to_string()));
        assert_eq!(config.model, Some("gpt-4o".to_string()));
        assert_eq!(config.subtask, Some(true));
        assert_eq!(template, "This is the template content.");
    }

    #[test]
    fn test_parse_frontmatter_empty() {
        let content = r#"---
---

Template only."#;

        let (config, template) = parse_frontmatter(content).unwrap();

        assert!(config.description.is_none());
        assert_eq!(template, "Template only.");
    }

    #[test]
    fn test_parse_frontmatter_no_yaml() {
        let content = "Just a template without frontmatter.";

        let (config, template) = parse_frontmatter(content).unwrap();

        assert!(config.description.is_none());
        assert_eq!(template, "Just a template without frontmatter.");
    }

    #[test]
    fn test_parse_frontmatter_missing_closing() {
        let content = r#"---
description: "Test"

No closing delimiter"#;

        let result = parse_frontmatter(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_hints_with_arguments() {
        let template = "Run with $ARGUMENTS and $1 then $2";
        let hints = hints(template);

        assert!(hints.contains(&"$ARGUMENTS".to_string()));
        assert!(hints.contains(&"$1".to_string()));
        assert!(hints.contains(&"$2".to_string()));
    }

    #[test]
    fn test_hints_numbered_only() {
        let template = "First: $1, Second: $2, Third: $3";
        let hints = hints(template);

        assert_eq!(hints.len(), 3);
        assert_eq!(hints[0], "$1");
        assert_eq!(hints[1], "$2");
        assert_eq!(hints[2], "$3");
    }

    #[test]
    fn test_hints_no_placeholders() {
        let template = "Static template with no placeholders";
        let hints = hints(template);

        assert!(hints.is_empty());
    }

    #[test]
    fn test_substitute_arguments() {
        let template = "Echo: $ARGUMENTS";
        let result = substitute_placeholders(template, "hello world");

        assert_eq!(result, "Echo: hello world");
    }

    #[test]
    fn test_substitute_numbered() {
        let template = "First: $1, Second: $2";
        let result = substitute_placeholders(template, "one two");

        assert_eq!(result, "First: one, Second: two");
    }

    #[test]
    fn test_substitute_last_captures_rest() {
        let template = "Cmd: $1 with rest: $2";
        let result = substitute_placeholders(template, "first second third fourth");

        assert_eq!(result, "Cmd: first with rest: second third fourth");
    }

    #[test]
    fn test_substitute_missing_args() {
        let template = "A: $1, B: $2, C: $3";
        let result = substitute_placeholders(template, "only_one");

        assert_eq!(result, "A: only_one, B: , C: ");
    }

    #[test]
    fn test_command_parse() {
        let content = r#"---
description: "Build command"
---

Build the project with $ARGUMENTS"#;

        let cmd = Command::parse("build", content, PathBuf::from("/test/build.md")).unwrap();

        assert_eq!(cmd.name, "build");
        assert_eq!(cmd.description(), "Build command");
        assert!(cmd.expects_arguments());
        assert_eq!(
            cmd.substitute("--release"),
            "Build the project with --release"
        );
    }
}
