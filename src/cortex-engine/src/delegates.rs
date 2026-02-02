//! Custom Delegates (Subagents) system for Cortex CLI.
//!
//! Delegates are reusable subagents defined in Markdown files with YAML frontmatter.
//! They provide specialized capabilities with custom prompts, models, and tool access.
//!
//! Delegate locations:
//! - Project delegates: .cortex/delegates/ (shared with team)
//! - Personal delegates: ~/.cortex/delegates/ (personal)
//!
//! File format: YAML frontmatter + Markdown system prompt

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

/// Delegate metadata from YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegateMetadata {
    /// Delegate name (lowercase letters, digits, hyphens, underscores).
    pub name: String,
    /// Description of what this delegate does (max 500 chars).
    #[serde(default)]
    pub description: String,
    /// Model to use: "inherit" or a specific model ID.
    #[serde(default = "default_model")]
    pub model: String,
    /// Reasoning effort (low, medium, high).
    #[serde(default, rename = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
    /// Tools: category name, array of tool IDs, or omit for all tools.
    #[serde(default)]
    pub tools: ToolsConfig,
}

fn default_model() -> String {
    "inherit".to_string()
}

/// Tools configuration for a delegate.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(untagged)]
pub enum ToolsConfig {
    /// All tools enabled.
    #[default]
    All,
    /// Use a category (read-only, edit, execute, web, mcp).
    Category(String),
    /// Explicit list of tool IDs.
    List(Vec<String>),
}

impl ToolsConfig {
    /// Get the list of tool IDs for this config.
    pub fn get_tools(&self, available_tools: &[String]) -> Vec<String> {
        match self {
            Self::All => available_tools.to_vec(),
            Self::Category(cat) => Self::tools_for_category(cat),
            Self::List(tools) => tools.clone(),
        }
    }

    /// Get tools for a category.
    fn tools_for_category(category: &str) -> Vec<String> {
        match category.to_lowercase().as_str() {
            "read-only" | "readonly" => vec![
                "Read".to_string(),
                "LS".to_string(),
                "Grep".to_string(),
                "Glob".to_string(),
            ],
            "edit" => vec![
                "Create".to_string(),
                "Edit".to_string(),
                "ApplyPatch".to_string(),
            ],
            "execute" => vec!["Execute".to_string()],
            "web" => vec!["WebSearch".to_string(), "FetchUrl".to_string()],
            "all" | "" => vec![], // Empty means all
            _ => vec![],          // Unknown category = all
        }
    }

    /// Get display string.
    pub fn display(&self) -> String {
        match self {
            Self::All => "All tools".to_string(),
            Self::Category(cat) => format!("Category: {cat}"),
            Self::List(tools) => format!("{} selected", tools.len()),
        }
    }
}

impl DelegateMetadata {
    /// Validate the metadata.
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(CortexError::InvalidInput(
                "Delegate name cannot be empty".to_string(),
            ));
        }

        if self.name.len() > 64 {
            return Err(CortexError::InvalidInput(
                "Delegate name must be at most 64 characters".to_string(),
            ));
        }

        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(CortexError::InvalidInput(
                "Delegate name must contain only lowercase letters, digits, hyphens, and underscores".to_string(),
            ));
        }

        if self.description.len() > 500 {
            return Err(CortexError::InvalidInput(
                "Delegate description must be at most 500 characters".to_string(),
            ));
        }

        Ok(())
    }
}

/// A loaded delegate definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegate {
    /// Delegate metadata from frontmatter.
    pub metadata: DelegateMetadata,
    /// System prompt (Markdown body).
    pub system_prompt: String,
    /// Path to the delegate file.
    pub path: PathBuf,
    /// Source of the delegate.
    pub source: DelegateSource,
}

/// Source of a delegate definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateSource {
    /// Project delegate from .cortex/delegates/.
    Project,
    /// Personal delegate from ~/.cortex/delegates/.
    Personal,
    /// Built-in delegate.
    Builtin,
}

impl std::fmt::Display for DelegateSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project => write!(f, "Project"),
            Self::Personal => write!(f, "Personal"),
            Self::Builtin => write!(f, "Built-in"),
        }
    }
}

impl Delegate {
    /// Get the delegate's ID.
    pub fn id(&self) -> &str {
        &self.metadata.name
    }

    /// Get the effective model for this delegate.
    pub fn effective_model(&self, parent_model: &str) -> String {
        if self.metadata.model == "inherit" || self.metadata.model.is_empty() {
            parent_model.to_string()
        } else {
            self.metadata.model.clone()
        }
    }

    /// Check if a tool is allowed for this delegate.
    pub fn allows_tool(&self, tool_name: &str, available_tools: &[String]) -> bool {
        let allowed = self.metadata.tools.get_tools(available_tools);
        if allowed.is_empty() {
            true
        } else {
            allowed.iter().any(|t| t == tool_name)
        }
    }
}

/// Delegate registry for managing delegate definitions.
pub struct DelegateRegistry {
    /// Loaded delegates by name.
    delegates: RwLock<HashMap<String, Delegate>>,
    /// Personal delegates directory.
    personal_dir: PathBuf,
    /// Project delegates directory.
    project_dir: Option<PathBuf>,
}

impl DelegateRegistry {
    /// Create a new delegate registry.
    pub fn new(cortex_home: &Path, project_root: Option<&Path>) -> Self {
        let personal_dir = cortex_home.join("delegates");
        let project_dir = project_root.map(|p| p.join(".cortex").join("delegates"));

        Self {
            delegates: RwLock::new(HashMap::new()),
            personal_dir,
            project_dir,
        }
    }

    /// Scan and load all delegates.
    pub async fn scan(&self) -> Result<Vec<Delegate>> {
        let mut all_delegates = Vec::new();
        let mut loaded_names = HashMap::new();

        // Load project delegates first (higher priority)
        if let Some(ref project_dir) = self.project_dir
            && project_dir.exists()
        {
            let delegates = self.scan_directory(project_dir, DelegateSource::Project)?;
            for delegate in delegates {
                loaded_names.insert(delegate.metadata.name.clone(), delegate.clone());
                all_delegates.push(delegate);
            }
        }

        // Load personal delegates
        if self.personal_dir.exists() {
            let delegates = self.scan_directory(&self.personal_dir, DelegateSource::Personal)?;
            for delegate in delegates {
                if !loaded_names.contains_key(&delegate.metadata.name) {
                    loaded_names.insert(delegate.metadata.name.clone(), delegate.clone());
                    all_delegates.push(delegate);
                }
            }
        }

        // Register all delegates
        let mut registry = self.delegates.write().await;
        *registry = loaded_names;

        Ok(all_delegates)
    }

    /// Scan a directory for delegate files.
    fn scan_directory(&self, dir: &Path, source: DelegateSource) -> Result<Vec<Delegate>> {
        let mut delegates = Vec::new();

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e == "md").unwrap_or(false) {
                match self.load_delegate(&path, source) {
                    Ok(delegate) => delegates.push(delegate),
                    Err(e) => {
                        tracing::warn!("Failed to load delegate from {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(delegates)
    }

    /// Load a delegate from a file.
    fn load_delegate(&self, path: &Path, source: DelegateSource) -> Result<Delegate> {
        let content = std::fs::read_to_string(path)?;
        let (metadata, prompt) = parse_delegate_md(&content)?;

        metadata.validate()?;

        Ok(Delegate {
            metadata,
            system_prompt: prompt,
            path: path.to_path_buf(),
            source,
        })
    }

    /// Get a delegate by name.
    pub async fn get(&self, name: &str) -> Option<Delegate> {
        self.delegates.read().await.get(name).cloned()
    }

    /// List all delegates.
    pub async fn list(&self) -> Vec<Delegate> {
        self.delegates.read().await.values().cloned().collect()
    }

    /// Create a new delegate file.
    pub async fn create(
        &self,
        name: &str,
        description: &str,
        model: &str,
        tools: ToolsConfig,
        system_prompt: &str,
        location: DelegateSource,
    ) -> Result<PathBuf> {
        let metadata = DelegateMetadata {
            name: name.to_string(),
            description: description.to_string(),
            model: model.to_string(),
            reasoning_effort: None,
            tools,
        };

        metadata.validate()?;

        let dir = match location {
            DelegateSource::Personal => &self.personal_dir,
            DelegateSource::Project => self
                .project_dir
                .as_ref()
                .ok_or_else(|| CortexError::InvalidInput("No project directory".to_string()))?,
            DelegateSource::Builtin => {
                return Err(CortexError::InvalidInput(
                    "Cannot create builtin delegates".to_string(),
                ));
            }
        };

        std::fs::create_dir_all(dir)?;

        let filename = format!("{}.md", name.to_lowercase().replace(' ', "-"));
        let path = dir.join(&filename);

        let content = generate_delegate_md(&metadata, system_prompt);
        std::fs::write(&path, content)?;

        self.scan().await?;

        Ok(path)
    }

    /// Delete a delegate.
    pub async fn delete(&self, name: &str) -> Result<()> {
        let delegate = self
            .get(name)
            .await
            .ok_or_else(|| CortexError::NotFound(format!("Delegate not found: {name}")))?;

        if delegate.source == DelegateSource::Builtin {
            return Err(CortexError::InvalidInput(
                "Cannot delete builtin delegates".to_string(),
            ));
        }

        std::fs::remove_file(&delegate.path)?;
        self.scan().await?;

        Ok(())
    }

    /// Reload all delegates.
    pub async fn reload(&self) -> Result<Vec<Delegate>> {
        self.delegates.write().await.clear();
        self.scan().await
    }
}

/// Parse a delegate .md file into metadata and prompt.
fn parse_delegate_md(content: &str) -> Result<(DelegateMetadata, String)> {
    let content = content.trim();

    if !content.starts_with("---") {
        return Err(CortexError::InvalidInput(
            "Delegate file must start with YAML frontmatter (---)".to_string(),
        ));
    }

    let rest = &content[3..];
    let end_idx = rest
        .find("\n---")
        .ok_or_else(|| CortexError::InvalidInput("Missing closing ---".to_string()))?;

    let yaml_content = &rest[..end_idx].trim();
    let markdown_content = &rest[end_idx + 4..].trim();

    if markdown_content.is_empty() {
        return Err(CortexError::InvalidInput(
            "Delegate must have a non-empty prompt".to_string(),
        ));
    }

    let metadata: DelegateMetadata = serde_yaml::from_str(yaml_content)
        .map_err(|e| CortexError::InvalidInput(format!("Invalid YAML: {e}")))?;

    Ok((metadata, markdown_content.to_string()))
}

/// Generate a delegate .md file content.
fn generate_delegate_md(metadata: &DelegateMetadata, system_prompt: &str) -> String {
    let mut yaml = String::from("---\n");
    yaml.push_str(&format!("name: {}\n", metadata.name));

    if !metadata.description.is_empty() {
        yaml.push_str(&format!("description: {}\n", metadata.description));
    }

    yaml.push_str(&format!("model: {}\n", metadata.model));

    if let Some(ref effort) = metadata.reasoning_effort {
        yaml.push_str(&format!("reasoningEffort: {effort}\n"));
    }

    match &metadata.tools {
        ToolsConfig::All => {}
        ToolsConfig::Category(cat) => {
            yaml.push_str(&format!("tools: {cat}\n"));
        }
        ToolsConfig::List(tools) => {
            yaml.push_str("tools: [");
            yaml.push_str(
                &tools
                    .iter()
                    .map(|t| format!("\"{t}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            yaml.push_str("]\n");
        }
    }

    yaml.push_str("---\n\n");
    yaml.push_str(system_prompt);

    yaml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_delegate_md() {
        let content = r#"---
name: code-reviewer
description: Reviews code for quality
model: inherit
tools: read-only
---

You are a code reviewer.
"#;

        let (metadata, prompt) = parse_delegate_md(content).unwrap();

        assert_eq!(metadata.name, "code-reviewer");
        assert_eq!(metadata.model, "inherit");
        assert!(prompt.contains("code reviewer"));
    }

    #[test]
    fn test_delegate_metadata_validation() {
        let meta = DelegateMetadata {
            name: "valid-delegate".to_string(),
            description: "Test".to_string(),
            model: "inherit".to_string(),
            reasoning_effort: None,
            tools: ToolsConfig::All,
        };
        assert!(meta.validate().is_ok());

        let meta = DelegateMetadata {
            name: "Invalid Name".to_string(),
            description: "Test".to_string(),
            model: "inherit".to_string(),
            reasoning_effort: None,
            tools: ToolsConfig::All,
        };
        assert!(meta.validate().is_err());
    }
}
