//! Error types for the skills system.
//!
//! Provides comprehensive error handling with clear terminal reporting.

use std::path::PathBuf;
use thiserror::Error;

/// Result type alias for skill operations.
pub type SkillResult<T> = std::result::Result<T, SkillError>;

/// Errors that can occur in the skills system.
#[derive(Error, Debug)]
pub enum SkillError {
    /// SKILL.toml file not found.
    #[error("SKILL.toml not found: {0}")]
    TomlNotFound(PathBuf),

    /// skill.md file not found.
    #[error("skill.md not found: {0}")]
    PromptNotFound(PathBuf),

    /// TOML parsing error.
    #[error("Failed to parse SKILL.toml: {0}")]
    TomlParse(#[from] toml::de::Error),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Validation error.
    #[error("Validation error: {0}")]
    Validation(String),

    /// Skill not found.
    #[error("Skill not found: {0}")]
    NotFound(String),

    /// Skill execution timeout.
    #[error("Skill '{skill}' timed out after {timeout_secs}s")]
    Timeout { skill: String, timeout_secs: u64 },

    /// Tool not allowed for this skill.
    #[error("Tool '{tool}' is not allowed for skill '{skill}'")]
    ToolNotAllowed { tool: String, skill: String },

    /// Tool explicitly denied for this skill.
    #[error("Tool '{tool}' is explicitly denied for skill '{skill}'")]
    ToolDenied { tool: String, skill: String },

    /// Invalid skill configuration.
    #[error("Invalid skill configuration: {0}")]
    InvalidConfig(String),

    /// Hot reload error.
    #[error("Hot reload error: {0}")]
    HotReload(String),

    /// Watch error.
    #[error("Watch error: {0}")]
    Watch(#[from] notify::Error),

    /// Pattern matching error.
    #[error("Pattern matching error: {0}")]
    Pattern(#[from] glob::PatternError),

    /// Skill directory not found.
    #[error("Skill directory not found: {0}")]
    DirNotFound(PathBuf),

    /// Missing required field.
    #[error("Missing required field '{field}' in SKILL.toml for skill '{skill}'")]
    MissingField { skill: String, field: String },

    /// Invalid reasoning effort value.
    #[error("Invalid reasoning effort: {0}. Must be 'low', 'medium', or 'high'")]
    InvalidReasoningEffort(String),
}

impl SkillError {
    /// Returns true if this error is recoverable (e.g., skill can be skipped).
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            SkillError::TomlNotFound(_)
                | SkillError::PromptNotFound(_)
                | SkillError::TomlParse(_)
                | SkillError::Validation(_)
        )
    }

    /// Returns a user-friendly error message for terminal display.
    pub fn user_message(&self) -> String {
        match self {
            SkillError::NotFound(name) => {
                format!(
                    "Skill '{}' not found.\n\nAvailable skills can be listed with /skills command.",
                    name
                )
            }
            SkillError::ToolNotAllowed { tool, skill } => {
                format!(
                    "Tool '{}' is not in the allowed tools list for skill '{}'.\n\n\
                     Check the skill's SKILL.toml for allowed_tools configuration.",
                    tool, skill
                )
            }
            SkillError::ToolDenied { tool, skill } => {
                format!(
                    "Tool '{}' is explicitly denied for skill '{}'.\n\n\
                     This tool is in the denied_tools list in SKILL.toml.",
                    tool, skill
                )
            }
            SkillError::Timeout {
                skill,
                timeout_secs,
            } => {
                format!(
                    "Skill '{}' exceeded its timeout of {} seconds.\n\n\
                     Consider increasing the timeout in SKILL.toml or optimizing the skill.",
                    skill, timeout_secs
                )
            }
            SkillError::InvalidConfig(msg) => {
                format!(
                    "Invalid skill configuration: {}\n\nPlease check the SKILL.toml file.",
                    msg
                )
            }
            _ => self.to_string(),
        }
    }

    /// Returns the exit code for CLI error reporting.
    pub fn exit_code(&self) -> i32 {
        match self {
            SkillError::NotFound(_) => 2,
            SkillError::Validation(_) => 3,
            SkillError::Timeout { .. } => 4,
            SkillError::ToolNotAllowed { .. } | SkillError::ToolDenied { .. } => 5,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = SkillError::NotFound("test-skill".to_string());
        assert!(err.to_string().contains("test-skill"));
    }

    #[test]
    fn test_error_is_recoverable() {
        assert!(SkillError::TomlNotFound(PathBuf::from("/test")).is_recoverable());
        assert!(SkillError::Validation("test".to_string()).is_recoverable());
        assert!(!SkillError::NotFound("test".to_string()).is_recoverable());
    }

    #[test]
    fn test_user_message() {
        let err = SkillError::NotFound("code-review".to_string());
        let msg = err.user_message();
        assert!(msg.contains("code-review"));
        assert!(msg.contains("/skills"));
    }

    #[test]
    fn test_exit_codes() {
        assert_eq!(SkillError::NotFound("x".to_string()).exit_code(), 2);
        assert_eq!(SkillError::Validation("x".to_string()).exit_code(), 3);
        assert_eq!(
            SkillError::Timeout {
                skill: "x".to_string(),
                timeout_secs: 10
            }
            .exit_code(),
            4
        );
    }
}
