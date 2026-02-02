//! Import functionality for external command formats.
//!
//! Supports importing commands from:
//! - `.claude/commands/` (Claude Code format)
//! - `.agents/` (Agent configuration)
//! - Other compatible markdown command formats

use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, info, warn};

/// Errors that can occur during import.
#[derive(Debug, Error)]
pub enum ImportError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Path not found.
    #[error("Path not found: {0}")]
    PathNotFound(PathBuf),

    /// Invalid format.
    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    /// Conversion error.
    #[error("Conversion error: {0}")]
    ConversionError(String),
}

/// Type of item being imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportType {
    /// Custom command.
    Command,
    /// Custom agent configuration.
    Agent,
}

impl std::fmt::Display for ImportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportType::Command => write!(f, "command"),
            ImportType::Agent => write!(f, "agent"),
        }
    }
}

/// Result of scanning for importable items.
#[derive(Debug, Default)]
pub struct ImportScan {
    /// Found command files.
    pub commands: Vec<PathBuf>,
    /// Found custom agent files.
    pub agents: Vec<PathBuf>,
}

impl ImportScan {
    /// Create a new empty scan result.
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of items found.
    pub fn total(&self) -> usize {
        self.commands.len() + self.agents.len()
    }

    /// Check if no items were found.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.agents.is_empty()
    }
}

/// Importer for commands and agents from external formats.
pub struct ClaudeImporter {
    /// Source directories to scan.
    source_dirs: Vec<PathBuf>,
}

impl Default for ClaudeImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeImporter {
    /// Create a new importer with default source directories.
    pub fn new() -> Self {
        let mut dirs = Vec::new();

        // ~/.claude/commands/
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".claude/commands"));
            dirs.push(home.join(".claude/agents"));
        }

        Self { source_dirs: dirs }
    }

    /// Create an importer with custom source directories.
    pub fn with_dirs(dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            source_dirs: dirs.into_iter().collect(),
        }
    }

    /// Add a source directory.
    pub fn add_source_dir(&mut self, dir: PathBuf) {
        if !self.source_dirs.contains(&dir) {
            self.source_dirs.push(dir);
        }
    }

    /// Scan for commands and agents to import.
    pub fn scan(&self) -> ImportScan {
        let mut scan = ImportScan::new();

        for dir in &self.source_dirs {
            if !dir.exists() {
                debug!("Import directory {:?} does not exist, skipping", dir);
                continue;
            }

            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();

                    // Only process .md files
                    if path.extension().is_some_and(|e| e == "md") {
                        // Determine type based on parent directory name
                        if dir.file_name().is_some_and(|n| n == "agents") {
                            scan.agents.push(path);
                        } else {
                            scan.commands.push(path);
                        }
                    }
                }
            }
        }

        info!(
            "Import scan found {} commands and {} agents",
            scan.commands.len(),
            scan.agents.len()
        );

        scan
    }

    /// Import selected items to the target directory.
    pub async fn import(
        &self,
        items: &[PathBuf],
        target: &Path,
        as_type: ImportType,
    ) -> Result<usize, ImportError> {
        // Create target directory if needed
        tokio::fs::create_dir_all(target).await?;

        let mut count = 0;

        for path in items {
            if !path.exists() {
                warn!("Import source {:?} does not exist, skipping", path);
                continue;
            }

            let content = tokio::fs::read_to_string(path).await?;
            let converted = self.convert(&content, as_type)?;

            let dest = target.join(
                path.file_name()
                    .ok_or_else(|| ImportError::InvalidFormat("Invalid filename".to_string()))?,
            );

            tokio::fs::write(&dest, converted).await?;
            debug!("Imported {:?} to {:?}", path, dest);
            count += 1;
        }

        info!("Imported {} {}s to {:?}", count, as_type, target);

        Ok(count)
    }

    /// Convert content from external format to Cortex format.
    fn convert(&self, content: &str, as_type: ImportType) -> Result<String, ImportError> {
        // Parse frontmatter if present
        let (frontmatter, body) = parse_frontmatter(content)?;

        // Build new frontmatter with mapped fields
        let mut new_frontmatter = String::new();

        // Map common fields
        if let Some(desc) = frontmatter.get("description") {
            new_frontmatter.push_str(&format!("description: {}\n", desc));
        }

        // Map type-specific fields
        match as_type {
            ImportType::Command => {
                // Command-specific fields
                if let Some(agent) = frontmatter.get("agent") {
                    new_frontmatter.push_str(&format!("agent: {}\n", agent));
                }

                if let Some(model) = frontmatter.get("model") {
                    new_frontmatter.push_str(&format!("model: {}\n", model));
                }

                if let Some(subtask) = frontmatter.get("subtask") {
                    new_frontmatter.push_str(&format!("subtask: {}\n", subtask));
                }
            }
            ImportType::Agent => {
                // Agent-specific fields
                if let Some(name) = frontmatter.get("name") {
                    new_frontmatter.push_str(&format!("name: {}\n", name));
                }

                if let Some(model) = frontmatter.get("model") {
                    new_frontmatter.push_str(&format!("model: {}\n", model));
                }

                // Map reasoningEffort
                if let Some(effort) = frontmatter.get("reasoningEffort") {
                    new_frontmatter.push_str(&format!("reasoning_effort: {}\n", effort));
                }

                // Map tools configuration
                if let Some(tools) = frontmatter.get("tools") {
                    new_frontmatter.push_str(&format!("tools: {}\n", tools));
                }

                // Map allowed_tools (Claude Code format)
                if let Some(tools) = frontmatter.get("allowed_tools") {
                    new_frontmatter.push_str(&format!("tools: {}\n", tools));
                }
            }
        }

        // Construct final content
        let result = if new_frontmatter.is_empty() {
            body
        } else {
            format!("---\n{}---\n\n{}", new_frontmatter, body)
        };

        Ok(result)
    }

    /// Import all found items automatically.
    pub async fn import_all(
        &self,
        command_target: &Path,
        agent_target: &Path,
    ) -> Result<usize, ImportError> {
        let scan = self.scan();
        let mut total = 0;

        if !scan.commands.is_empty() {
            total += self
                .import(&scan.commands, command_target, ImportType::Command)
                .await?;
        }

        if !scan.agents.is_empty() {
            total += self
                .import(&scan.agents, agent_target, ImportType::Agent)
                .await?;
        }

        Ok(total)
    }
}

/// Parse YAML-like frontmatter from content.
fn parse_frontmatter(
    content: &str,
) -> Result<(std::collections::HashMap<String, String>, String), ImportError> {
    let content = content.trim();
    let mut frontmatter = std::collections::HashMap::new();

    // Check if content starts with frontmatter delimiter
    if !content.starts_with("---") {
        return Ok((frontmatter, content.to_string()));
    }

    // Find the closing delimiter
    let rest = &content[3..];
    let end_delimiter = rest.find("\n---");

    match end_delimiter {
        Some(end_pos) => {
            let yaml_content = &rest[..end_pos].trim();
            let body = rest[end_pos + 4..].trim();

            // Simple YAML-like parsing (key: value)
            for line in yaml_content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }

                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim().to_string();
                    let value = value
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string();
                    frontmatter.insert(key, value);
                }
            }

            Ok((frontmatter, body.to_string()))
        }
        None => {
            // No closing delimiter, treat entire content as body
            Ok((frontmatter, content.to_string()))
        }
    }
}

/// Helper function to get default import source directories.
pub fn default_import_sources() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".claude/commands"));
        dirs.push(home.join(".claude/agents"));
        dirs.push(home.join(".agents"));
    }

    dirs
}

/// Helper function to get the default command import target.
pub fn default_command_target(project_path: Option<&Path>) -> PathBuf {
    project_path
        .map(|p| p.join(".cortex/commands"))
        .unwrap_or_else(|| {
            dirs::config_dir()
                .expect("Config directory should exist")
                .join("cortex/commands")
        })
}

/// Helper function to get the default agent import target.
pub fn default_agent_target(project_path: Option<&Path>) -> PathBuf {
    project_path
        .map(|p| p.join(".cortex/agents"))
        .unwrap_or_else(|| {
            dirs::config_dir()
                .expect("Config directory should exist")
                .join("cortex/agents")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_frontmatter() {
        let content = r#"---
description: "Test command"
agent: "build"
model: "gpt-4"
---

This is the template."#;

        let (fm, body) = parse_frontmatter(content).unwrap();

        assert_eq!(fm.get("description"), Some(&"Test command".to_string()));
        assert_eq!(fm.get("agent"), Some(&"build".to_string()));
        assert_eq!(fm.get("model"), Some(&"gpt-4".to_string()));
        assert_eq!(body, "This is the template.");
    }

    #[test]
    fn test_parse_frontmatter_no_yaml() {
        let content = "Just a template without frontmatter.";
        let (fm, body) = parse_frontmatter(content).unwrap();

        assert!(fm.is_empty());
        assert_eq!(body, content);
    }

    #[test]
    fn test_import_scan() {
        let temp = TempDir::new().unwrap();

        // Create commands directory
        let commands_dir = temp.path().join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(commands_dir.join("cmd1.md"), "# Command 1").unwrap();
        std::fs::write(commands_dir.join("cmd2.md"), "# Command 2").unwrap();

        // Create agents directory
        let agents_dir = temp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("agent1.md"), "# Agent 1").unwrap();

        let importer = ClaudeImporter::with_dirs([commands_dir, agents_dir]);
        let scan = importer.scan();

        assert_eq!(scan.commands.len(), 2);
        assert_eq!(scan.agents.len(), 1);
        assert_eq!(scan.total(), 3);
    }

    #[tokio::test]
    async fn test_import_commands() {
        let source = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();

        // Create source command
        std::fs::write(
            source.path().join("test.md"),
            r#"---
description: "Test"
---

Template"#,
        )
        .unwrap();

        let importer = ClaudeImporter::new();
        let items = vec![source.path().join("test.md")];

        let count = importer
            .import(&items, target.path(), ImportType::Command)
            .await
            .unwrap();

        assert_eq!(count, 1);
        assert!(target.path().join("test.md").exists());
    }

    #[test]
    fn test_convert_command() {
        let importer = ClaudeImporter::new();

        let content = r#"---
description: "Test command"
agent: "build"
---

Template content"#;

        let converted = importer.convert(content, ImportType::Command).unwrap();

        assert!(converted.contains("description:"));
        assert!(converted.contains("agent:"));
        assert!(converted.contains("Template content"));
    }

    #[test]
    fn test_convert_agent() {
        let importer = ClaudeImporter::new();

        let content = r#"---
name: "test-agent"
description: "A test agent"
model: "gpt-4"
reasoningEffort: high
tools: read-only
---

Agent prompt content"#;

        let converted = importer.convert(content, ImportType::Agent).unwrap();

        assert!(converted.contains("name:"));
        assert!(converted.contains("description:"));
        assert!(converted.contains("reasoning_effort:"));
        assert!(converted.contains("tools:"));
        assert!(converted.contains("Agent prompt content"));
    }

    #[test]
    fn test_default_import_sources() {
        let sources = default_import_sources();
        // Should have at least the .claude directories if home exists
        if dirs::home_dir().is_some() {
            assert!(!sources.is_empty());
        }
    }
}
