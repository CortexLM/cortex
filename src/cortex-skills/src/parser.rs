//! SKILL.toml parser.
//!
//! Parses skill definition files into Skill structures.

use std::fs;
use std::path::Path;

use serde::Deserialize;
use tracing::{debug, warn};

use crate::error::{SkillError, SkillResult};
use crate::skill::{ReasoningEffort, Skill, SkillConfig, SkillMetadata};

/// SKILL.toml file structure for deserialization.
#[derive(Debug, Deserialize)]
pub struct SkillToml {
    /// Skill name (required).
    pub name: String,

    /// Skill description (required).
    pub description: String,

    /// Skill version.
    #[serde(default = "default_version")]
    pub version: String,

    /// Inline prompt (optional, alternative to skill.md file).
    #[serde(default)]
    pub prompt: Option<String>,

    /// Specific model to use.
    #[serde(default)]
    pub model: Option<String>,

    /// Allowed tools.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,

    /// Denied tools.
    #[serde(default)]
    pub denied_tools: Option<Vec<String>>,

    /// Auto-allowed flag.
    #[serde(default)]
    pub auto_allowed: Option<bool>,

    /// Reasoning effort level.
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,

    /// Timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,

    /// Author.
    #[serde(default)]
    pub author: Option<String>,

    /// Icon emoji.
    #[serde(default)]
    pub icon: Option<String>,

    /// Tags.
    #[serde(default)]
    pub tags: Option<Vec<String>>,

    /// Homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// Parse a SKILL.toml file and load the associated skill.md prompt.
///
/// # Arguments
///
/// * `path` - Path to the SKILL.toml file
///
/// # Returns
///
/// A fully loaded `Skill` with prompt from skill.md or inline prompt.
///
/// # Errors
///
/// Returns an error if:
/// - SKILL.toml cannot be read or parsed
/// - Neither skill.md nor inline prompt exists
/// - Required fields are missing
pub fn parse_skill_toml(path: &Path) -> SkillResult<Skill> {
    debug!("Parsing SKILL.toml at {:?}", path);

    if !path.exists() {
        return Err(SkillError::TomlNotFound(path.to_path_buf()));
    }

    let content = fs::read_to_string(path)?;
    let toml: SkillToml = toml::from_str(&content)?;

    // Validate required fields
    if toml.name.trim().is_empty() {
        return Err(SkillError::MissingField {
            skill: path.to_string_lossy().to_string(),
            field: "name".to_string(),
        });
    }

    if toml.description.trim().is_empty() {
        return Err(SkillError::MissingField {
            skill: toml.name.clone(),
            field: "description".to_string(),
        });
    }

    // Get the skill directory (parent of SKILL.toml)
    let skill_dir = path
        .parent()
        .ok_or_else(|| SkillError::InvalidConfig("Invalid SKILL.toml path".to_string()))?;

    // Load prompt from skill.md or use inline prompt
    let prompt_path = skill_dir.join("skill.md");
    let prompt = if prompt_path.exists() {
        debug!("Loading prompt from {:?}", prompt_path);
        fs::read_to_string(&prompt_path)?
    } else if let Some(inline_prompt) = &toml.prompt {
        debug!("Using inline prompt for skill '{}'", toml.name);
        inline_prompt.clone()
    } else {
        warn!(
            "No skill.md or inline prompt found for skill '{}' at {:?}",
            toml.name, skill_dir
        );
        return Err(SkillError::PromptNotFound(prompt_path));
    };

    // Generate skill ID from name (lowercase, replace spaces with hyphens)
    let id = toml
        .name
        .to_lowercase()
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();

    // Build config
    let config = SkillConfig {
        model: toml.model,
        allowed_tools: toml.allowed_tools.unwrap_or_default(),
        denied_tools: toml.denied_tools.unwrap_or_default(),
        auto_allowed: toml.auto_allowed.unwrap_or(false),
        reasoning_effort: toml.reasoning_effort,
        timeout: toml.timeout,
    };

    // Build metadata
    let metadata = SkillMetadata {
        author: toml.author,
        icon: toml.icon,
        tags: toml.tags.unwrap_or_default(),
        homepage: toml.homepage,
    };

    Ok(Skill::new(
        id,
        toml.name,
        toml.description,
        toml.version,
        prompt,
        config,
        metadata,
        skill_dir.to_path_buf(),
    ))
}

/// Parse a SKILL.toml file asynchronously.
pub async fn parse_skill_toml_async(path: &Path) -> SkillResult<Skill> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || parse_skill_toml(&path))
        .await
        .map_err(|e| SkillError::InvalidConfig(format!("Task join error: {}", e)))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_skill(dir: &Path, name: &str, description: &str, prompt: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();

        let toml_content = format!(
            r#"
name = "{}"
description = "{}"
version = "1.0.0"
model = "claude-sonnet-4"
allowed_tools = ["Read", "Grep"]
denied_tools = ["Execute"]
auto_allowed = true
reasoning_effort = "high"
timeout = 300
author = "Test Author"
icon = "🔍"
tags = ["test", "example"]
homepage = "https://example.com"
"#,
            name, description
        );

        fs::write(skill_dir.join("SKILL.toml"), toml_content).unwrap();
        fs::write(skill_dir.join("skill.md"), prompt).unwrap();
    }

    #[test]
    fn test_parse_skill_toml_full() {
        let temp = TempDir::new().unwrap();
        create_test_skill(
            temp.path(),
            "code-review",
            "Expert code reviewer",
            "You are an expert code reviewer...",
        );

        let skill = parse_skill_toml(&temp.path().join("code-review/SKILL.toml")).unwrap();

        assert_eq!(skill.id, "code-review");
        assert_eq!(skill.name, "code-review");
        assert_eq!(skill.description, "Expert code reviewer");
        assert_eq!(skill.version, "1.0.0");
        assert!(skill.prompt.contains("expert code reviewer"));
        assert_eq!(skill.config.model, Some("claude-sonnet-4".to_string()));
        assert_eq!(skill.config.allowed_tools, vec!["Read", "Grep"]);
        assert_eq!(skill.config.denied_tools, vec!["Execute"]);
        assert!(skill.config.auto_allowed);
        assert_eq!(skill.config.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(skill.config.timeout, Some(300));
        assert_eq!(skill.metadata.author, Some("Test Author".to_string()));
        assert_eq!(skill.metadata.icon, Some("🔍".to_string()));
        assert_eq!(skill.metadata.tags, vec!["test", "example"]);
        assert_eq!(
            skill.metadata.homepage,
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn test_parse_skill_toml_minimal() {
        let temp = TempDir::new().unwrap();
        let skill_dir = temp.path().join("minimal");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
name = "minimal"
description = "A minimal skill"
"#,
        )
        .unwrap();
        fs::write(skill_dir.join("skill.md"), "Minimal prompt").unwrap();

        let skill = parse_skill_toml(&skill_dir.join("SKILL.toml")).unwrap();

        assert_eq!(skill.id, "minimal");
        assert_eq!(skill.name, "minimal");
        assert_eq!(skill.version, "0.1.0");
        assert!(!skill.config.auto_allowed);
        assert!(skill.config.model.is_none());
    }

    #[test]
    fn test_parse_skill_toml_inline_prompt() {
        let temp = TempDir::new().unwrap();
        let skill_dir = temp.path().join("inline");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
name = "inline"
description = "Skill with inline prompt"
prompt = "This is an inline prompt."
"#,
        )
        .unwrap();

        let skill = parse_skill_toml(&skill_dir.join("SKILL.toml")).unwrap();
        assert_eq!(skill.prompt, "This is an inline prompt.");
    }

    #[test]
    fn test_parse_skill_toml_missing_file() {
        let result = parse_skill_toml(Path::new("/nonexistent/SKILL.toml"));
        assert!(matches!(result, Err(SkillError::TomlNotFound(_))));
    }

    #[test]
    fn test_parse_skill_toml_missing_prompt() {
        let temp = TempDir::new().unwrap();
        let skill_dir = temp.path().join("no-prompt");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
name = "no-prompt"
description = "Skill without prompt"
"#,
        )
        .unwrap();

        let result = parse_skill_toml(&skill_dir.join("SKILL.toml"));
        assert!(matches!(result, Err(SkillError::PromptNotFound(_))));
    }

    #[test]
    fn test_parse_skill_toml_missing_name() {
        let temp = TempDir::new().unwrap();
        let skill_dir = temp.path().join("missing-name");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
description = "Missing name"
prompt = "Test"
"#,
        )
        .unwrap();

        let result = parse_skill_toml(&skill_dir.join("SKILL.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_skill_toml_empty_name() {
        let temp = TempDir::new().unwrap();
        let skill_dir = temp.path().join("empty-name");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
name = ""
description = "Empty name"
prompt = "Test"
"#,
        )
        .unwrap();

        let result = parse_skill_toml(&skill_dir.join("SKILL.toml"));
        assert!(matches!(result, Err(SkillError::MissingField { .. })));
    }

    #[test]
    fn test_skill_id_generation() {
        let temp = TempDir::new().unwrap();
        let skill_dir = temp.path().join("test");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
name = "Code Review Tool"
description = "Test"
prompt = "Test"
"#,
        )
        .unwrap();

        let skill = parse_skill_toml(&skill_dir.join("SKILL.toml")).unwrap();
        assert_eq!(skill.id, "code-review-tool");
    }
}
