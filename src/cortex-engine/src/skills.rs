//! Skills system for Cortex CLI.
//!
//! Skills are modular capabilities that extend the agent's functionality.
//! Each skill consists of a SKILL.md file with YAML frontmatter and Markdown instructions.
//!
//! Skills can be stored in:
//! - Personal skills: ~/.cortex/skills/
//! - Project skills: .cortex/skills/
//! - Plugin skills: bundled with installed plugins
//!
//! Skills are model-invoked - the agent autonomously decides when to use them
//! based on the request and the skill's description.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

/// Skill metadata from YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Skill name (lowercase, hyphens, max 64 chars).
    pub name: String,
    /// Brief description of what the skill does and when to use it.
    pub description: String,
    /// Optional list of allowed tools when this skill is active.
    #[serde(default, alias = "allowed-tools")]
    pub allowed_tools: Option<Vec<String>>,
    /// Optional version string.
    pub version: Option<String>,
    /// Optional author.
    pub author: Option<String>,
    /// Optional tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl SkillMetadata {
    /// Validate the metadata.
    pub fn validate(&self) -> Result<()> {
        // Name validation: lowercase, hyphens, numbers only, max 64 chars
        if self.name.is_empty() || self.name.len() > 64 {
            return Err(CortexError::InvalidInput(
                "Skill name must be 1-64 characters".to_string(),
            ));
        }

        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
        {
            return Err(CortexError::InvalidInput(
                "Skill name must contain only lowercase letters, numbers, and hyphens".to_string(),
            ));
        }

        // Description validation: max 1024 chars
        if self.description.len() > 1024 {
            return Err(CortexError::InvalidInput(
                "Skill description must be at most 1024 characters".to_string(),
            ));
        }

        Ok(())
    }
}

/// A loaded skill with metadata and content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Skill metadata from frontmatter.
    pub metadata: SkillMetadata,
    /// Markdown content (instructions).
    pub content: String,
    /// Path to the skill directory.
    pub path: PathBuf,
    /// Source of the skill.
    pub source: SkillSource,
    /// Supporting files in the skill directory.
    #[serde(default)]
    pub supporting_files: Vec<String>,
}

impl Skill {
    /// Get the skill's ID (same as name).
    pub fn id(&self) -> &str {
        &self.metadata.name
    }

    /// Check if this skill allows a specific tool.
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        match &self.metadata.allowed_tools {
            Some(tools) => tools.iter().any(|t| t == tool_name),
            None => true, // No restriction means all tools allowed
        }
    }

    /// Get the full instructions including any referenced files.
    pub fn get_instructions(&self) -> String {
        self.content.clone()
    }

    /// Read a supporting file from the skill directory.
    pub fn read_supporting_file(&self, filename: &str) -> Result<String> {
        let file_path = self.path.join(filename);
        if !file_path.exists() {
            return Err(CortexError::NotFound(format!(
                "Supporting file not found: {filename}"
            )));
        }
        std::fs::read_to_string(&file_path).map_err(std::convert::Into::into)
    }
}

/// Source of a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Personal skill from ~/.cortex/skills/
    Personal,
    /// Project skill from .cortex/skills/, .agents/, or .agent/
    Project,
    /// Plugin-provided skill
    Plugin,
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Personal => write!(f, "personal"),
            Self::Project => write!(f, "project"),
            Self::Plugin => write!(f, "plugin"),
        }
    }
}

/// Skill registry that manages discovery and loading of skills.
pub struct SkillRegistry {
    /// Loaded skills by name.
    skills: RwLock<HashMap<String, Skill>>,
    /// Personal skills directory (~/.cortex/skills/).
    personal_dir: PathBuf,
    /// Project skills directories (.agents/, .agent/, .cortex/skills/).
    project_dirs: Vec<PathBuf>,
    /// Plugin skills directories.
    plugin_dirs: Vec<PathBuf>,
}

impl SkillRegistry {
    /// Create a new skill registry.
    ///
    /// Skills are searched in the following order:
    /// 1. `.agents/` (project, https://agent.md/ compatible)
    /// 2. `.agent/` (project, https://agent.md/ compatible)
    /// 3. `.cortex/skills/` (project, traditional format)
    /// 4. `~/.cortex/skills/` (personal)
    /// 5. Plugin directories
    pub fn new(cortex_home: &Path, project_root: Option<&Path>) -> Self {
        let personal_dir = cortex_home.join("skills");

        // Support multiple project skill directories
        let project_dirs = if let Some(root) = project_root {
            vec![
                root.join(".agents"),                // agent.md format
                root.join(".agent"),                 // agent.md format (singular)
                root.join(".cortex").join("skills"), // traditional format
            ]
        } else {
            Vec::new()
        };

        Self {
            skills: RwLock::new(HashMap::new()),
            personal_dir,
            project_dirs,
            plugin_dirs: Vec::new(),
        }
    }

    /// Add a plugin skills directory.
    pub fn add_plugin_dir(&mut self, dir: PathBuf) {
        self.plugin_dirs.push(dir);
    }

    /// Scan and load all available skills.
    pub async fn scan(&self) -> Result<Vec<Skill>> {
        let mut all_skills = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        // Scan project skills first (from .agents/, .agent/, .cortex/skills/)
        // Project skills take precedence
        for project_dir in &self.project_dirs {
            if project_dir.exists() {
                let skills = self.scan_directory(project_dir, SkillSource::Project)?;
                for skill in skills {
                    if !seen_names.contains(&skill.metadata.name) {
                        seen_names.insert(skill.metadata.name.clone());
                        all_skills.push(skill);
                    }
                }
            }
        }

        // Scan personal skills
        if self.personal_dir.exists() {
            let skills = self.scan_directory(&self.personal_dir, SkillSource::Personal)?;
            for skill in skills {
                if !seen_names.contains(&skill.metadata.name) {
                    seen_names.insert(skill.metadata.name.clone());
                    all_skills.push(skill);
                }
            }
        }

        // Scan plugin skills
        for plugin_dir in &self.plugin_dirs {
            if plugin_dir.exists() {
                let skills = self.scan_directory(plugin_dir, SkillSource::Plugin)?;
                for skill in skills {
                    if !seen_names.contains(&skill.metadata.name) {
                        seen_names.insert(skill.metadata.name.clone());
                        all_skills.push(skill);
                    }
                }
            }
        }

        // Register all skills
        let mut registry = self.skills.write().await;
        for skill in &all_skills {
            registry.insert(skill.metadata.name.clone(), skill.clone());
        }

        Ok(all_skills)
    }

    /// Scan a single directory for skills.
    fn scan_directory(&self, dir: &Path, source: SkillSource) -> Result<Vec<Skill>> {
        let mut skills = Vec::new();

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.exists() {
                    match self.load_skill(&path, source) {
                        Ok(skill) => skills.push(skill),
                        Err(e) => {
                            tracing::warn!("Failed to load skill from {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }

        Ok(skills)
    }

    /// Load a skill from a directory.
    fn load_skill(&self, skill_dir: &Path, source: SkillSource) -> Result<Skill> {
        let skill_md_path = skill_dir.join("SKILL.md");
        let content = std::fs::read_to_string(&skill_md_path)?;

        // Parse YAML frontmatter
        let (metadata, markdown) = parse_skill_md(&content)?;

        // Validate metadata
        metadata.validate()?;

        // Find supporting files
        let supporting_files = self.find_supporting_files(skill_dir)?;

        Ok(Skill {
            metadata,
            content: markdown,
            path: skill_dir.to_path_buf(),
            source,
            supporting_files,
        })
    }

    /// Find supporting files in a skill directory.
    fn find_supporting_files(&self, skill_dir: &Path) -> Result<Vec<String>> {
        let mut files = Vec::new();

        for entry in std::fs::read_dir(skill_dir)? {
            let entry = entry?;
            let path = entry.path();
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Skip SKILL.md itself
            if filename == "SKILL.md" {
                continue;
            }

            // Include markdown files, scripts, and common formats
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(
                ext,
                "md" | "py" | "sh" | "js" | "ts" | "json" | "yaml" | "yml" | "txt"
            ) && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                files.push(name.to_string());
            }

            // Include script directories
            if path.is_dir() && matches!(filename, "scripts" | "templates" | "examples") {
                files.push(filename.to_string());
            }
        }

        Ok(files)
    }

    /// Get a skill by name.
    pub async fn get(&self, name: &str) -> Option<Skill> {
        self.skills.read().await.get(name).cloned()
    }

    /// List all loaded skills.
    pub async fn list(&self) -> Vec<Skill> {
        self.skills.read().await.values().cloned().collect()
    }

    /// Find skills relevant to a query based on description matching.
    pub async fn find_relevant(&self, query: &str) -> Vec<Skill> {
        let query_lower = query.to_lowercase();
        let skills = self.skills.read().await;

        skills
            .values()
            .filter(|skill| {
                let desc_lower = skill.metadata.description.to_lowercase();
                let name_lower = skill.metadata.name.to_lowercase();

                // Check if query terms appear in description or name
                query_lower
                    .split_whitespace()
                    .any(|term| desc_lower.contains(term) || name_lower.contains(term))
            })
            .cloned()
            .collect()
    }

    /// Get skills by source.
    pub async fn get_by_source(&self, source: SkillSource) -> Vec<Skill> {
        self.skills
            .read()
            .await
            .values()
            .filter(|s| s.source == source)
            .cloned()
            .collect()
    }

    /// Reload all skills.
    pub async fn reload(&self) -> Result<Vec<Skill>> {
        self.skills.write().await.clear();
        self.scan().await
    }
}

/// Parse a SKILL.md file into metadata and content.
fn parse_skill_md(content: &str) -> Result<(SkillMetadata, String)> {
    let content = content.trim();

    // Check for YAML frontmatter
    if !content.starts_with("---") {
        return Err(CortexError::InvalidInput(
            "SKILL.md must start with YAML frontmatter (---)".to_string(),
        ));
    }

    // Find the closing ---
    let rest = &content[3..];
    let end_idx = rest.find("\n---").ok_or_else(|| {
        CortexError::InvalidInput("Missing closing --- for YAML frontmatter".to_string())
    })?;

    let yaml_content = &rest[..end_idx].trim();
    let markdown_content = &rest[end_idx + 4..].trim();

    // Parse YAML
    let metadata: SkillMetadata = serde_yaml::from_str(yaml_content)
        .map_err(|e| CortexError::InvalidInput(format!("Invalid YAML frontmatter: {e}")))?;

    Ok((metadata, markdown_content.to_string()))
}

/// Builder for creating skills programmatically.
pub struct SkillBuilder {
    metadata: SkillMetadata,
    content: String,
    path: PathBuf,
    source: SkillSource,
}

impl SkillBuilder {
    /// Create a new skill builder.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            metadata: SkillMetadata {
                name: name.into(),
                description: String::new(),
                allowed_tools: None,
                version: None,
                author: None,
                tags: Vec::new(),
            },
            content: String::new(),
            path: PathBuf::new(),
            source: SkillSource::Personal,
        }
    }

    /// Set the description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.metadata.description = desc.into();
        self
    }

    /// Set allowed tools.
    pub fn allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.metadata.allowed_tools = Some(tools);
        self
    }

    /// Set the content.
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }

    /// Set the source.
    pub fn source(mut self, source: SkillSource) -> Self {
        self.source = source;
        self
    }

    /// Set the path.
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = path.into();
        self
    }

    /// Set version.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.metadata.version = Some(version.into());
        self
    }

    /// Add a tag.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.metadata.tags.push(tag.into());
        self
    }

    /// Build the skill.
    pub fn build(self) -> Result<Skill> {
        self.metadata.validate()?;

        Ok(Skill {
            metadata: self.metadata,
            content: self.content,
            path: self.path,
            source: self.source,
            supporting_files: Vec::new(),
        })
    }
}

/// Generate the SKILL.md content for a skill.
pub fn generate_skill_md(skill: &Skill) -> String {
    let mut yaml = String::from("---\n");
    yaml.push_str(&format!("name: {}\n", skill.metadata.name));
    yaml.push_str(&format!("description: {}\n", skill.metadata.description));

    if let Some(ref tools) = skill.metadata.allowed_tools {
        yaml.push_str(&format!("allowed-tools: {}\n", tools.join(", ")));
    }

    if let Some(ref version) = skill.metadata.version {
        yaml.push_str(&format!("version: {version}\n"));
    }

    if let Some(ref author) = skill.metadata.author {
        yaml.push_str(&format!("author: {author}\n"));
    }

    if !skill.metadata.tags.is_empty() {
        yaml.push_str(&format!("tags: [{}]\n", skill.metadata.tags.join(", ")));
    }

    yaml.push_str("---\n\n");
    yaml.push_str(&skill.content);

    yaml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_md() {
        let content = r#"---
name: test-skill
description: A test skill for unit testing
allowed-tools: [Read, Grep, Glob]
---

# Test Skill

## Instructions
This is a test skill.
"#;

        let (metadata, markdown) = parse_skill_md(content).unwrap();

        assert_eq!(metadata.name, "test-skill");
        assert_eq!(metadata.description, "A test skill for unit testing");
        assert_eq!(
            metadata.allowed_tools,
            Some(vec![
                "Read".to_string(),
                "Grep".to_string(),
                "Glob".to_string()
            ])
        );
        assert!(markdown.contains("# Test Skill"));
    }

    #[test]
    fn test_skill_metadata_validation() {
        // Valid name
        let meta = SkillMetadata {
            name: "valid-skill-123".to_string(),
            description: "Test".to_string(),
            allowed_tools: None,
            version: None,
            author: None,
            tags: Vec::new(),
        };
        assert!(meta.validate().is_ok());

        // Invalid name (uppercase)
        let meta = SkillMetadata {
            name: "Invalid-Skill".to_string(),
            description: "Test".to_string(),
            allowed_tools: None,
            version: None,
            author: None,
            tags: Vec::new(),
        };
        assert!(meta.validate().is_err());

        // Invalid name (too long)
        let meta = SkillMetadata {
            name: "a".repeat(65),
            description: "Test".to_string(),
            allowed_tools: None,
            version: None,
            author: None,
            tags: Vec::new(),
        };
        assert!(meta.validate().is_err());
    }

    #[test]
    fn test_skill_builder() {
        let skill = SkillBuilder::new("my-skill")
            .description("A custom skill")
            .allowed_tools(vec!["Read".to_string()])
            .content("# Instructions\nDo something")
            .version("1.0.0")
            .tag("testing")
            .build()
            .unwrap();

        assert_eq!(skill.id(), "my-skill");
        assert!(skill.allows_tool("Read"));
        assert!(!skill.allows_tool("Execute"));
    }

    #[test]
    fn test_skill_allows_tool() {
        let skill = SkillBuilder::new("restricted")
            .description("Restricted skill")
            .allowed_tools(vec!["Read".to_string(), "Grep".to_string()])
            .build()
            .unwrap();

        assert!(skill.allows_tool("Read"));
        assert!(skill.allows_tool("Grep"));
        assert!(!skill.allows_tool("Execute"));
        assert!(!skill.allows_tool("Write"));

        // Skill without restrictions
        let unrestricted = SkillBuilder::new("unrestricted")
            .description("No restrictions")
            .build()
            .unwrap();

        assert!(unrestricted.allows_tool("Execute"));
        assert!(unrestricted.allows_tool("Write"));
    }
}
