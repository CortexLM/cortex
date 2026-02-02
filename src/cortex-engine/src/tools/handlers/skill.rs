//! Skill tool handler for loading skills.
//!
//! This handler allows the agent to load skill files for specialized task guidance.
//! Skills are discovered from (in priority order):
//! - Current directory `SKILL.md` files (local)
//! - `.agents/` (project skills, https://agent.md/ compatible)
//! - `.agent/` (project skills, https://agent.md/ compatible)
//! - `.cortex/skills/` (project skills)
//! - `~/.cortex/skills/` (personal skills)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::RwLock;

use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::{CortexError, Result};
use crate::tools::spec::ToolDefinition;

/// Skill argument definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillArg {
    /// Argument name.
    pub name: String,
    /// Description of the argument.
    pub description: String,
    /// Whether the argument is required.
    #[serde(default)]
    pub required: bool,
    /// Default value if not provided.
    pub default: Option<String>,
}

/// Extended skill metadata with argument support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    /// Skill name (lowercase, hyphens, max 64 chars).
    pub name: String,
    /// Brief description of what the skill does.
    pub description: String,
    /// Arguments the skill accepts.
    #[serde(default)]
    pub args: Vec<SkillArg>,
    /// Allowed tools when this skill is active.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Optional version string.
    pub version: Option<String>,
    /// Optional author.
    pub author: Option<String>,
    /// Optional tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl SkillDefinition {
    /// Validate the skill definition.
    pub fn validate(&self) -> Result<()> {
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

        if self.description.len() > 1024 {
            return Err(CortexError::InvalidInput(
                "Skill description must be at most 1024 characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Get required arguments.
    pub fn required_args(&self) -> Vec<&SkillArg> {
        self.args.iter().filter(|a| a.required).collect()
    }
}

/// A loaded skill with definition and content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedSkill {
    /// Skill definition from frontmatter.
    pub definition: SkillDefinition,
    /// Markdown content (instructions).
    pub content: String,
    /// Path to the skill file or directory.
    pub path: PathBuf,
    /// Source of the skill.
    pub source: SkillSource,
}

impl LoadedSkill {
    /// Get the skill's ID (same as name).
    pub fn id(&self) -> &str {
        &self.definition.name
    }

    /// Check if this skill allows a specific tool.
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        if self.definition.tools.is_empty() {
            true // No restriction means all tools allowed
        } else {
            self.definition
                .tools
                .iter()
                .any(|t| t.eq_ignore_ascii_case(tool_name))
        }
    }

    /// Render the skill content with provided arguments.
    pub fn render(&self, args: &HashMap<String, String>) -> Result<String> {
        let mut content = self.content.clone();

        // Substitute arguments in the content using {{arg_name}} syntax
        for arg in &self.definition.args {
            let placeholder = format!("{{{{{}}}}}", arg.name);
            let value = args
                .get(&arg.name)
                .cloned()
                .or_else(|| arg.default.clone())
                .unwrap_or_default();
            content = content.replace(&placeholder, &value);
        }

        Ok(content)
    }

    /// Validate that all required arguments are provided.
    pub fn validate_args(&self, args: &HashMap<String, String>) -> Result<()> {
        for arg in self.definition.required_args() {
            if !args.contains_key(&arg.name) && arg.default.is_none() {
                return Err(CortexError::InvalidInput(format!(
                    "Missing required argument: {} - {}",
                    arg.name, arg.description
                )));
            }
        }
        Ok(())
    }
}

/// Source of a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Personal skill from ~/.cortex/skills/
    Personal,
    /// Project skill from .cortex/skills/
    Project,
    /// Current directory SKILL.md
    Local,
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Personal => write!(f, "personal"),
            Self::Project => write!(f, "project"),
            Self::Local => write!(f, "local"),
        }
    }
}

/// Skill loader that discovers and loads skills.
pub struct SkillLoader {
    /// Personal skills directory (~/.cortex/skills/).
    personal_dir: PathBuf,
    /// Project skills directories (in priority order).
    /// Supports: .agents/, .agent/, .cortex/skills/
    project_dirs: Vec<PathBuf>,
    /// Current working directory.
    cwd: PathBuf,
}

impl SkillLoader {
    /// Create a new skill loader.
    ///
    /// Skill search paths (in priority order):
    /// 1. `./SKILL.md` (local file)
    /// 2. `.agents/` (project, https://agent.md/ compatible)
    /// 3. `.agent/` (project, https://agent.md/ compatible)
    /// 4. `.cortex/skills/` (project, traditional format)
    /// 5. `~/.cortex/skills/` (personal)
    pub fn new(cwd: PathBuf) -> Self {
        let personal_dir = dirs::home_dir()
            .map(|h| h.join(".cortex").join("skills"))
            .unwrap_or_else(|| PathBuf::from("~/.cortex/skills"));

        // Support multiple project skill directories
        let project_dirs = vec![
            cwd.join(".agents"),                // https://agent.md/ format
            cwd.join(".agent"),                 // https://agent.md/ format (singular)
            cwd.join(".cortex").join("skills"), // Traditional format
        ];

        Self {
            personal_dir,
            project_dirs,
            cwd,
        }
    }

    /// Load a skill by name.
    pub async fn load(&self, name: &str) -> Result<LoadedSkill> {
        // Search order: local SKILL.md, project skills, personal skills

        // 1. Check for SKILL.md in current directory
        let local_skill = self.cwd.join("SKILL.md");
        if local_skill.exists() {
            let skill = self
                .load_skill_file(&local_skill, SkillSource::Local)
                .await?;
            if skill.definition.name == name || name == "local" {
                return Ok(skill);
            }
        }

        // 2. Check project skills (multiple directories: .agents/, .agent/, .cortex/skills/)
        for project_dir in &self.project_dirs {
            if let Some(skill) = self
                .find_skill_in_dir(project_dir, name, SkillSource::Project)
                .await?
            {
                return Ok(skill);
            }
        }

        // 3. Check personal skills
        if let Some(skill) = self
            .find_skill_in_dir(&self.personal_dir, name, SkillSource::Personal)
            .await?
        {
            return Ok(skill);
        }

        // Build list of searched paths for error message
        let project_paths: Vec<String> = self
            .project_dirs
            .iter()
            .map(|p| p.display().to_string())
            .collect();

        Err(CortexError::NotFound(format!(
            "Skill not found: {}. Searched in:\n  - {}/SKILL.md (local)\n  - {} (project)\n  - {} (personal)",
            name,
            self.cwd.display(),
            project_paths.join(", "),
            self.personal_dir.display()
        )))
    }

    /// Find a skill in a directory.
    async fn find_skill_in_dir(
        &self,
        dir: &Path,
        name: &str,
        source: SkillSource,
    ) -> Result<Option<LoadedSkill>> {
        if !dir.exists() {
            return Ok(None);
        }

        // Check for skill directory with SKILL.md
        let skill_dir = dir.join(name);
        let skill_file = skill_dir.join("SKILL.md");
        if skill_file.exists() {
            return Ok(Some(self.load_skill_file(&skill_file, source).await?));
        }

        // Check for direct SKILL.md file with matching name
        let direct_file = dir.join(format!("{}.md", name));
        if direct_file.exists() {
            return Ok(Some(self.load_skill_file(&direct_file, source).await?));
        }

        // Scan all subdirectories for matching skill
        if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    let skill_md = path.join("SKILL.md");
                    if skill_md.exists() {
                        if let Ok(skill) = self.load_skill_file(&skill_md, source).await {
                            if skill.definition.name == name {
                                return Ok(Some(skill));
                            }
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    /// Load a skill from a file.
    async fn load_skill_file(&self, path: &Path, source: SkillSource) -> Result<LoadedSkill> {
        let content = tokio::fs::read_to_string(path).await?;
        let (definition, markdown) = parse_skill_md(&content)?;

        definition.validate()?;

        Ok(LoadedSkill {
            definition,
            content: markdown,
            path: path.to_path_buf(),
            source,
        })
    }

    /// List all available skills.
    pub async fn list(&self) -> Result<Vec<LoadedSkill>> {
        let mut skills = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        // Check local SKILL.md
        let local_skill = self.cwd.join("SKILL.md");
        if local_skill.exists() {
            if let Ok(skill) = self.load_skill_file(&local_skill, SkillSource::Local).await {
                seen_names.insert(skill.definition.name.clone());
                skills.push(skill);
            }
        }

        // Scan project skills (multiple directories: .agents/, .agent/, .cortex/skills/)
        for project_dir in &self.project_dirs {
            if let Ok(project_skills) = self.scan_directory(project_dir, SkillSource::Project).await
            {
                for skill in project_skills {
                    // Skip duplicates (first occurrence wins)
                    if !seen_names.contains(&skill.definition.name) {
                        seen_names.insert(skill.definition.name.clone());
                        skills.push(skill);
                    }
                }
            }
        }

        // Scan personal skills
        if let Ok(personal_skills) = self
            .scan_directory(&self.personal_dir, SkillSource::Personal)
            .await
        {
            for skill in personal_skills {
                // Skip duplicates (project skills take precedence)
                if !seen_names.contains(&skill.definition.name) {
                    seen_names.insert(skill.definition.name.clone());
                    skills.push(skill);
                }
            }
        }

        Ok(skills)
    }

    /// Scan a directory for skills.
    async fn scan_directory(&self, dir: &Path, source: SkillSource) -> Result<Vec<LoadedSkill>> {
        let mut skills = Vec::new();

        if !dir.exists() {
            return Ok(skills);
        }

        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(_) => return Ok(skills),
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();

            if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.exists() {
                    if let Ok(skill) = self.load_skill_file(&skill_md, source).await {
                        skills.push(skill);
                    }
                }
            } else if path.extension().is_some_and(|e| e == "md") {
                if let Ok(skill) = self.load_skill_file(&path, source).await {
                    skills.push(skill);
                }
            }
        }

        Ok(skills)
    }
}

/// Parse a SKILL.md file into definition and content.
fn parse_skill_md(content: &str) -> Result<(SkillDefinition, String)> {
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

    let yaml_content = rest[..end_idx].trim();
    let markdown_content = rest[end_idx + 4..].trim();

    // Parse YAML
    let definition: SkillDefinition = serde_yaml::from_str(yaml_content)
        .map_err(|e| CortexError::InvalidInput(format!("Invalid YAML frontmatter: {e}")))?;

    Ok((definition, markdown_content.to_string()))
}

/// Registry tracking which skills are loaded in the session.
#[derive(Default)]
pub struct SkillRegistry {
    /// Currently loaded skills by name.
    loaded: RwLock<HashMap<String, LoadedSkill>>,
}

impl SkillRegistry {
    /// Create a new skill registry.
    pub fn new() -> Self {
        Self {
            loaded: RwLock::new(HashMap::new()),
        }
    }

    /// Register a loaded skill.
    pub async fn register(&self, skill: LoadedSkill) {
        let mut loaded = self.loaded.write().await;
        loaded.insert(skill.definition.name.clone(), skill);
    }

    /// Get a loaded skill.
    pub async fn get(&self, name: &str) -> Option<LoadedSkill> {
        let loaded = self.loaded.read().await;
        loaded.get(name).cloned()
    }

    /// Check if a skill is loaded.
    pub async fn is_loaded(&self, name: &str) -> bool {
        let loaded = self.loaded.read().await;
        loaded.contains_key(name)
    }

    /// List all loaded skills.
    pub async fn list_loaded(&self) -> Vec<LoadedSkill> {
        let loaded = self.loaded.read().await;
        loaded.values().cloned().collect()
    }

    /// Clear all loaded skills.
    pub async fn clear(&self) {
        let mut loaded = self.loaded.write().await;
        loaded.clear();
    }
}

/// Handler for the skill tool.
pub struct SkillHandler {
    /// Skill loader for discovering skills.
    loader: SkillLoader,
    /// Registry of loaded skills.
    registry: Arc<SkillRegistry>,
}

impl SkillHandler {
    /// Create a new skill handler.
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        Self {
            loader: SkillLoader::new(cwd),
            registry: Arc::new(SkillRegistry::new()),
        }
    }

    /// Create a handler with a specific working directory.
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self {
            loader: SkillLoader::new(cwd),
            registry: Arc::new(SkillRegistry::new()),
        }
    }

    /// Create a handler with a shared registry.
    pub fn with_registry(cwd: PathBuf, registry: Arc<SkillRegistry>) -> Self {
        Self {
            loader: SkillLoader::new(cwd),
            registry,
        }
    }

    /// Get the tool definition.
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "UseSkill",
            "Execute a specialized skill within the conversation. Skills provide domain-specific \
             capabilities like browser automation, API testing, or data processing. Only use skills \
             listed in available_skills.",
            json!({
                "type": "object",
                "properties": {
                    "skill": {
                        "type": "string",
                        "description": "Name of the skill to execute (must exist in available_skills)"
                    }
                },
                "required": ["skill"],
                "additionalProperties": false
            }),
        )
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &Arc<SkillRegistry> {
        &self.registry
    }

    /// List available skills.
    #[allow(dead_code)]
    async fn list_skills(&self) -> Result<ToolResult> {
        let skills = self.loader.list().await?;

        if skills.is_empty() {
            return Ok(ToolResult::success(
                "No skills found. Create skills in:\n  \
                 - ./SKILL.md (local)\n  \
                 - .agents/<skill-name>/SKILL.md (project, agent.md format)\n  \
                 - .agent/<skill-name>/SKILL.md (project, agent.md format)\n  \
                 - .cortex/skills/<skill-name>/SKILL.md (project)\n  \
                 - ~/.cortex/skills/<skill-name>/SKILL.md (personal)",
            ));
        }

        let mut output = String::from("Available Skills:\n\n");
        for skill in &skills {
            output.push_str(&format!(
                "📚 {} ({}) - {}\n",
                skill.definition.name, skill.source, skill.definition.description
            ));

            if !skill.definition.args.is_empty() {
                output.push_str("   Arguments:\n");
                for arg in &skill.definition.args {
                    let required = if arg.required { " (required)" } else { "" };
                    output.push_str(&format!(
                        "     - {}{}: {}\n",
                        arg.name, required, arg.description
                    ));
                }
            }

            if !skill.definition.tools.is_empty() {
                output.push_str(&format!(
                    "   Tools: {}\n",
                    skill.definition.tools.join(", ")
                ));
            }
        }

        Ok(ToolResult::success(output))
    }

    /// Load and activate a skill.
    async fn load_skill(&self, name: &str, args: HashMap<String, String>) -> Result<ToolResult> {
        // Load the skill
        let skill = self.loader.load(name).await?;

        // Validate arguments
        skill.validate_args(&args)?;

        // Render content with arguments
        let rendered_content = skill.render(&args)?;

        // Register the skill as loaded
        self.registry.register(skill.clone()).await;

        // Build the response
        let mut output = format!("Skill '{}' loaded successfully\n\n", skill.definition.name);
        output.push_str(&format!(
            "Source: {} ({})\n",
            skill.source,
            skill.path.display()
        ));
        output.push_str(&format!("Description: {}\n", skill.definition.description));

        if !skill.definition.tools.is_empty() {
            output.push_str(&format!(
                "Allowed tools: {}\n",
                skill.definition.tools.join(", ")
            ));
        }

        if !args.is_empty() {
            output.push_str("\nArguments provided:\n");
            for (key, value) in &args {
                output.push_str(&format!("   - {}: {}\n", key, value));
            }
        }

        output.push_str("\n---\n\n# Skill Instructions\n\n");
        output.push_str(&rendered_content);

        Ok(ToolResult::success(output))
    }
}

impl Default for SkillHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for SkillHandler {
    fn name(&self) -> &str {
        "UseSkill"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let skill = arguments
            .get("skill")
            .and_then(|s| s.as_str())
            .ok_or_else(|| {
                CortexError::InvalidInput("'skill' parameter is required".to_string())
            })?;

        // Load the skill with no additional arguments (spec only allows 'skill' param)
        self.load_skill(skill, HashMap::new()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_md() {
        let content = r#"---
name: test-skill
description: A test skill for unit testing
args:
  - name: target
    description: Target file or directory
    required: true
  - name: verbose
    description: Enable verbose output
    required: false
    default: "false"
tools:
  - Read
  - Grep
  - Glob
---

# Test Skill

## Instructions

Analyze {{target}} with verbose={{verbose}}.
"#;

        let (def, markdown) = parse_skill_md(content).unwrap();

        assert_eq!(def.name, "test-skill");
        assert_eq!(def.description, "A test skill for unit testing");
        assert_eq!(def.args.len(), 2);
        assert_eq!(def.args[0].name, "target");
        assert!(def.args[0].required);
        assert_eq!(def.args[1].name, "verbose");
        assert!(!def.args[1].required);
        assert_eq!(def.args[1].default, Some("false".to_string()));
        assert_eq!(def.tools, vec!["Read", "Grep", "Glob"]);
        assert!(markdown.contains("# Test Skill"));
    }

    #[test]
    fn test_skill_definition_validation() {
        let valid = SkillDefinition {
            name: "valid-skill-123".to_string(),
            description: "Test skill".to_string(),
            args: vec![],
            tools: vec![],
            version: None,
            author: None,
            tags: vec![],
        };
        assert!(valid.validate().is_ok());

        let invalid_name = SkillDefinition {
            name: "Invalid-Skill".to_string(),
            description: "Test".to_string(),
            args: vec![],
            tools: vec![],
            version: None,
            author: None,
            tags: vec![],
        };
        assert!(invalid_name.validate().is_err());
    }

    #[test]
    fn test_loaded_skill_render() {
        let skill = LoadedSkill {
            definition: SkillDefinition {
                name: "test".to_string(),
                description: "Test".to_string(),
                args: vec![
                    SkillArg {
                        name: "name".to_string(),
                        description: "Name arg".to_string(),
                        required: true,
                        default: None,
                    },
                    SkillArg {
                        name: "count".to_string(),
                        description: "Count arg".to_string(),
                        required: false,
                        default: Some("5".to_string()),
                    },
                ],
                tools: vec![],
                version: None,
                author: None,
                tags: vec![],
            },
            content: "Hello {{name}}, count is {{count}}".to_string(),
            path: PathBuf::from("/test"),
            source: SkillSource::Local,
        };

        let mut args = HashMap::new();
        args.insert("name".to_string(), "World".to_string());

        let rendered = skill.render(&args).unwrap();
        assert_eq!(rendered, "Hello World, count is 5");
    }

    #[test]
    fn test_loaded_skill_validate_args() {
        let skill = LoadedSkill {
            definition: SkillDefinition {
                name: "test".to_string(),
                description: "Test".to_string(),
                args: vec![
                    SkillArg {
                        name: "required_arg".to_string(),
                        description: "Required".to_string(),
                        required: true,
                        default: None,
                    },
                    SkillArg {
                        name: "optional_arg".to_string(),
                        description: "Optional".to_string(),
                        required: false,
                        default: Some("default".to_string()),
                    },
                ],
                tools: vec![],
                version: None,
                author: None,
                tags: vec![],
            },
            content: String::new(),
            path: PathBuf::from("/test"),
            source: SkillSource::Local,
        };

        // Missing required arg should fail
        let empty_args = HashMap::new();
        assert!(skill.validate_args(&empty_args).is_err());

        // With required arg should pass
        let mut args = HashMap::new();
        args.insert("required_arg".to_string(), "value".to_string());
        assert!(skill.validate_args(&args).is_ok());
    }

    #[test]
    fn test_loaded_skill_allows_tool() {
        let restricted = LoadedSkill {
            definition: SkillDefinition {
                name: "restricted".to_string(),
                description: "Restricted".to_string(),
                args: vec![],
                tools: vec!["Read".to_string(), "Grep".to_string()],
                version: None,
                author: None,
                tags: vec![],
            },
            content: String::new(),
            path: PathBuf::from("/test"),
            source: SkillSource::Local,
        };

        assert!(restricted.allows_tool("Read"));
        assert!(restricted.allows_tool("read")); // Case-insensitive
        assert!(restricted.allows_tool("Grep"));
        assert!(!restricted.allows_tool("Execute"));

        let unrestricted = LoadedSkill {
            definition: SkillDefinition {
                name: "unrestricted".to_string(),
                description: "Unrestricted".to_string(),
                args: vec![],
                tools: vec![], // Empty means all allowed
                version: None,
                author: None,
                tags: vec![],
            },
            content: String::new(),
            path: PathBuf::from("/test"),
            source: SkillSource::Local,
        };

        assert!(unrestricted.allows_tool("Execute"));
        assert!(unrestricted.allows_tool("Write"));
    }
}
