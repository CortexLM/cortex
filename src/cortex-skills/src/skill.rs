//! Skill definition and configuration types.
//!
//! This module defines the core skill data structures.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Represents a loaded skill.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Unique skill identifier (derived from name).
    pub id: String,
    /// Human-readable skill name.
    pub name: String,
    /// Description of what the skill does.
    pub description: String,
    /// Skill version.
    pub version: String,
    /// The prompt template for the skill.
    pub prompt: String,
    /// Skill configuration.
    pub config: SkillConfig,
    /// Skill metadata.
    pub metadata: SkillMetadata,
    /// When the skill was loaded.
    pub loaded_at: DateTime<Utc>,
    /// Source path of the skill.
    pub source_path: PathBuf,
}

impl Skill {
    /// Creates a new skill with the given parameters.
    pub fn new(
        id: String,
        name: String,
        description: String,
        version: String,
        prompt: String,
        config: SkillConfig,
        metadata: SkillMetadata,
        source_path: PathBuf,
    ) -> Self {
        Self {
            id,
            name,
            description,
            version,
            prompt,
            config,
            metadata,
            loaded_at: Utc::now(),
            source_path,
        }
    }

    /// Returns the skill's display name with icon.
    pub fn display_name(&self) -> String {
        match &self.metadata.icon {
            Some(icon) => format!("{} {}", icon, self.name),
            None => self.name.clone(),
        }
    }

    /// Returns true if this skill is auto-allowed (doesn't require permission).
    pub fn is_auto_allowed(&self) -> bool {
        self.config.auto_allowed
    }

    /// Returns the model to use for this skill, or None for default.
    pub fn model(&self) -> Option<&str> {
        self.config.model.as_deref()
    }

    /// Returns the timeout in seconds, if configured.
    pub fn timeout_secs(&self) -> Option<u64> {
        self.config.timeout
    }

    /// Checks if a tool is allowed for this skill.
    ///
    /// Returns `Ok(())` if allowed, or an error describing why not.
    pub fn check_tool(&self, tool: &str) -> Result<(), crate::SkillError> {
        // Check if explicitly denied
        if self
            .config
            .denied_tools
            .iter()
            .any(|t| t.eq_ignore_ascii_case(tool))
        {
            return Err(crate::SkillError::ToolDenied {
                tool: tool.to_string(),
                skill: self.name.clone(),
            });
        }

        // If allowed_tools is empty, everything not denied is allowed
        if self.config.allowed_tools.is_empty() {
            return Ok(());
        }

        // Check if in allowed list
        if self
            .config
            .allowed_tools
            .iter()
            .any(|t| t.eq_ignore_ascii_case(tool))
        {
            return Ok(());
        }

        // Check for wildcard pattern matching
        for pattern in &self.config.allowed_tools {
            if pattern.contains('*')
                && let Ok(glob) = glob::Pattern::new(&pattern.to_lowercase())
                && glob.matches(&tool.to_lowercase())
            {
                return Ok(());
            }
        }

        Err(crate::SkillError::ToolNotAllowed {
            tool: tool.to_string(),
            skill: self.name.clone(),
        })
    }

    /// Returns the reasoning effort level for this skill.
    pub fn reasoning_effort(&self) -> Option<ReasoningEffort> {
        self.config.reasoning_effort
    }
}

impl Default for Skill {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            version: "0.1.0".to_string(),
            prompt: String::new(),
            config: SkillConfig::default(),
            metadata: SkillMetadata::default(),
            loaded_at: Utc::now(),
            source_path: PathBuf::new(),
        }
    }
}

/// Skill configuration options.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillConfig {
    /// Specific model to use for this skill (optional).
    #[serde(default)]
    pub model: Option<String>,

    /// Tools that are allowed for this skill.
    /// Empty means all tools are allowed (except denied ones).
    #[serde(default)]
    pub allowed_tools: Vec<String>,

    /// Tools that are explicitly denied for this skill.
    #[serde(default)]
    pub denied_tools: Vec<String>,

    /// Whether this skill is auto-allowed without permission prompt.
    #[serde(default)]
    pub auto_allowed: bool,

    /// Reasoning effort level.
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,

    /// Timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Skill metadata for display and discovery.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Author of the skill.
    #[serde(default)]
    pub author: Option<String>,

    /// Icon emoji or character.
    #[serde(default)]
    pub icon: Option<String>,

    /// Tags for categorization and search.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Homepage or documentation URL.
    #[serde(default)]
    pub homepage: Option<String>,
}

/// Reasoning effort level for skill execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// Low effort - quick responses.
    Low,
    /// Medium effort - balanced.
    Medium,
    /// High effort - deep reasoning.
    High,
}

impl ReasoningEffort {
    /// Returns the string representation for API calls.
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = crate::SkillError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(ReasoningEffort::Low),
            "medium" => Ok(ReasoningEffort::Medium),
            "high" => Ok(ReasoningEffort::High),
            _ => Err(crate::SkillError::InvalidReasoningEffort(s.to_string())),
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_default() {
        let skill = Skill::default();
        assert!(skill.id.is_empty());
        assert!(skill.name.is_empty());
        assert_eq!(skill.version, "0.1.0");
    }

    #[test]
    fn test_skill_display_name() {
        let mut skill = Skill::default();
        skill.name = "code-review".to_string();

        // Without icon
        assert_eq!(skill.display_name(), "code-review");

        // With icon
        skill.metadata.icon = Some("🔍".to_string());
        assert_eq!(skill.display_name(), "🔍 code-review");
    }

    #[test]
    fn test_check_tool_allowed() {
        let mut skill = Skill::default();
        skill.name = "test-skill".to_string();
        skill.config.allowed_tools = vec!["Read".to_string(), "Grep".to_string()];

        assert!(skill.check_tool("Read").is_ok());
        assert!(skill.check_tool("read").is_ok()); // Case insensitive
        assert!(skill.check_tool("Grep").is_ok());
        assert!(skill.check_tool("Execute").is_err());
    }

    #[test]
    fn test_check_tool_denied() {
        let mut skill = Skill::default();
        skill.name = "test-skill".to_string();
        skill.config.denied_tools = vec!["Execute".to_string()];

        assert!(skill.check_tool("Read").is_ok());
        assert!(skill.check_tool("Execute").is_err());
        assert!(skill.check_tool("execute").is_err()); // Case insensitive
    }

    #[test]
    fn test_check_tool_empty_allowed() {
        let mut skill = Skill::default();
        skill.name = "test-skill".to_string();
        // Empty allowed_tools means all are allowed except denied

        assert!(skill.check_tool("Read").is_ok());
        assert!(skill.check_tool("Execute").is_ok());
        assert!(skill.check_tool("Whatever").is_ok());
    }

    #[test]
    fn test_check_tool_wildcard() {
        let mut skill = Skill::default();
        skill.name = "test-skill".to_string();
        skill.config.allowed_tools = vec!["file*".to_string()];

        assert!(skill.check_tool("fileread").is_ok());
        assert!(skill.check_tool("FileWrite").is_ok());
        assert!(skill.check_tool("execute").is_err());
    }

    #[test]
    fn test_reasoning_effort_from_str() {
        assert_eq!(
            "low".parse::<ReasoningEffort>().unwrap(),
            ReasoningEffort::Low
        );
        assert_eq!(
            "MEDIUM".parse::<ReasoningEffort>().unwrap(),
            ReasoningEffort::Medium
        );
        assert_eq!(
            "High".parse::<ReasoningEffort>().unwrap(),
            ReasoningEffort::High
        );
        assert!("invalid".parse::<ReasoningEffort>().is_err());
    }

    #[test]
    fn test_reasoning_effort_as_str() {
        assert_eq!(ReasoningEffort::Low.as_str(), "low");
        assert_eq!(ReasoningEffort::Medium.as_str(), "medium");
        assert_eq!(ReasoningEffort::High.as_str(), "high");
    }

    #[test]
    fn test_skill_is_auto_allowed() {
        let mut skill = Skill::default();
        assert!(!skill.is_auto_allowed());

        skill.config.auto_allowed = true;
        assert!(skill.is_auto_allowed());
    }
}
