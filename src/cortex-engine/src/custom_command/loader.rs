//! Custom command file loader.
//!
//! Loads custom commands from markdown files with YAML frontmatter:
//! - Personal: ~/.cortex/commands/*.md
//! - Project: .cortex/commands/*.md
//!
//! File format:
//! ```markdown
//! ---
//! name: review
//! description: Review code changes
//! agent: plan
//! model: claude-3-5-sonnet
//! subtask: false
//! ---
//!
//! Please review the following code:
//! {{input}}
//! ```

use std::path::{Path, PathBuf};

use crate::error::{CortexError, Result};

use super::types::{CommandMetadata, CommandSource, CustomCommand};

/// Load a custom command from a markdown file.
pub fn load_command_file(path: &Path, source: CommandSource) -> Result<CustomCommand> {
    let content = std::fs::read_to_string(path)?;
    let (metadata, template) = parse_command_md(&content)?;

    let command = CustomCommand {
        name: metadata.name,
        description: metadata.description,
        template,
        agent: metadata.agent,
        model: metadata.model,
        subtask: metadata.subtask,
        category: metadata.category,
        aliases: metadata.aliases,
        source,
        source_path: Some(path.to_path_buf()),
    };

    command
        .validate()
        .map_err(|e| CortexError::InvalidInput(e))?;

    Ok(command)
}

/// Parse a command markdown file into metadata and template.
fn parse_command_md(content: &str) -> Result<(CommandMetadata, String)> {
    let content = content.trim();

    // Check for YAML frontmatter
    if !content.starts_with("---") {
        return Err(CortexError::InvalidInput(
            "Command file must start with YAML frontmatter (---)".to_string(),
        ));
    }

    // Find the closing ---
    let rest = &content[3..];
    let end_idx = rest.find("\n---").ok_or_else(|| {
        CortexError::InvalidInput("Missing closing --- for YAML frontmatter".to_string())
    })?;

    let yaml_content = rest[..end_idx].trim();
    let template_content = rest[end_idx + 4..].trim();

    if template_content.is_empty() {
        return Err(CortexError::InvalidInput(
            "Command must have a non-empty template".to_string(),
        ));
    }

    // Parse YAML
    let metadata: CommandMetadata = serde_yaml::from_str(yaml_content)
        .map_err(|e| CortexError::InvalidInput(format!("Invalid YAML frontmatter: {e}")))?;

    Ok((metadata, template_content.to_string()))
}

/// Scan a directory for command files.
pub fn scan_directory(dir: &Path, source: CommandSource) -> Result<Vec<CustomCommand>> {
    let mut commands = Vec::new();

    if !dir.exists() {
        return Ok(commands);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        // Only process .md files
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            match load_command_file(&path, source) {
                Ok(command) => commands.push(command),
                Err(e) => {
                    tracing::warn!("Failed to load command from {}: {}", path.display(), e);
                }
            }
        }
    }

    Ok(commands)
}

/// Get the personal commands directory (~/.cortex/commands/).
pub fn personal_commands_dir(cortex_home: &Path) -> PathBuf {
    cortex_home.join("commands")
}

/// Get the project commands directory (.cortex/commands/).
pub fn project_commands_dir(project_root: &Path) -> PathBuf {
    project_root.join(".cortex").join("commands")
}

/// Generate command file content from a CustomCommand.
pub fn generate_command_md(command: &CustomCommand) -> String {
    let mut yaml = String::from("---\n");
    yaml.push_str(&format!("name: {}\n", command.name));

    if !command.description.is_empty() {
        // Escape description if it contains special characters
        if command.description.contains(':') || command.description.contains('#') {
            yaml.push_str(&format!(
                "description: \"{}\"\n",
                command.description.replace('"', "\\\"")
            ));
        } else {
            yaml.push_str(&format!("description: {}\n", command.description));
        }
    }

    if let Some(ref agent) = command.agent {
        yaml.push_str(&format!("agent: {agent}\n"));
    }

    if let Some(ref model) = command.model {
        yaml.push_str(&format!("model: {model}\n"));
    }

    if command.subtask {
        yaml.push_str("subtask: true\n");
    }

    if let Some(ref category) = command.category {
        yaml.push_str(&format!("category: {category}\n"));
    }

    if !command.aliases.is_empty() {
        yaml.push_str(&format!("aliases: [{}]\n", command.aliases.join(", ")));
    }

    yaml.push_str("---\n\n");
    yaml.push_str(&command.template);

    yaml
}

/// Create a new command file.
pub fn create_command_file(dir: &Path, command: &CustomCommand) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;

    let filename = format!("{}.md", command.name);
    let path = dir.join(&filename);

    let content = generate_command_md(command);
    std::fs::write(&path, content)?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_md() {
        let content = r#"---
name: review
description: Review code changes
agent: plan
model: claude-3-5-sonnet
subtask: false
---

Please review the following code changes:

{{input}}

Focus on:
- Code quality
- Potential bugs
"#;

        let (metadata, template) = parse_command_md(content).unwrap();

        assert_eq!(metadata.name, "review");
        assert_eq!(metadata.description, "Review code changes");
        assert_eq!(metadata.agent, Some("plan".to_string()));
        assert_eq!(metadata.model, Some("claude-3-5-sonnet".to_string()));
        assert!(!metadata.subtask);
        assert!(template.contains("{{input}}"));
        assert!(template.contains("Code quality"));
    }

    #[test]
    fn test_parse_minimal_command() {
        let content = r#"---
name: quick
---

Just do: {{input}}
"#;

        let (metadata, template) = parse_command_md(content).unwrap();

        assert_eq!(metadata.name, "quick");
        assert!(metadata.description.is_empty());
        assert!(metadata.agent.is_none());
        assert!(template.contains("{{input}}"));
    }

    #[test]
    fn test_parse_invalid_no_frontmatter() {
        let content = "Just some text without frontmatter";
        assert!(parse_command_md(content).is_err());
    }

    #[test]
    fn test_parse_invalid_no_closing() {
        let content = r#"---
name: test
description: Missing closing
Template content
"#;
        assert!(parse_command_md(content).is_err());
    }

    #[test]
    fn test_parse_invalid_empty_template() {
        let content = r#"---
name: empty
---
"#;
        assert!(parse_command_md(content).is_err());
    }

    #[test]
    fn test_generate_command_md() {
        let command = CustomCommand::new("test", "Test command", "Do: {{input}}")
            .with_agent("coder")
            .with_model("gpt-4")
            .with_subtask(true)
            .with_category("Testing")
            .with_alias("t");

        let content = generate_command_md(&command);

        assert!(content.contains("name: test"));
        assert!(content.contains("description: Test command"));
        assert!(content.contains("agent: coder"));
        assert!(content.contains("model: gpt-4"));
        assert!(content.contains("subtask: true"));
        assert!(content.contains("category: Testing"));
        assert!(content.contains("aliases: [t]"));
        assert!(content.contains("Do: {{input}}"));
    }
}
