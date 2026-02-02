//! Skill input validation.
//!
//! Provides strict validation for skill inputs to prevent crashes
//! and ensure data integrity.

use crate::error::{SkillError, SkillResult};
use crate::skill::Skill;

/// Validation result with optional warnings.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether validation passed.
    pub is_valid: bool,
    /// Error messages if validation failed.
    pub errors: Vec<String>,
    /// Warning messages (non-fatal issues).
    pub warnings: Vec<String>,
}

impl ValidationResult {
    /// Creates a successful validation result.
    pub fn valid() -> Self {
        Self {
            is_valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Creates a failed validation result with an error.
    pub fn invalid(error: impl Into<String>) -> Self {
        Self {
            is_valid: false,
            errors: vec![error.into()],
            warnings: Vec::new(),
        }
    }

    /// Adds a warning to the result.
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Adds an error to the result.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.errors.push(error.into());
        self.is_valid = false;
        self
    }

    /// Converts to a Result, returning an error if validation failed.
    pub fn into_result(self) -> SkillResult<Vec<String>> {
        if self.is_valid {
            Ok(self.warnings)
        } else {
            Err(SkillError::Validation(self.errors.join("; ")))
        }
    }
}

/// Possible validation errors.
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// Skill name is empty or invalid.
    InvalidName(String),
    /// Description is empty.
    EmptyDescription,
    /// Prompt is empty.
    EmptyPrompt,
    /// Invalid model name.
    InvalidModel(String),
    /// Invalid timeout value.
    InvalidTimeout(u64),
    /// Conflicting tool configuration.
    ConflictingTools(String),
    /// Invalid tag.
    InvalidTag(String),
    /// Tool name is invalid.
    InvalidToolName(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::InvalidName(name) => {
                write!(
                    f,
                    "Invalid skill name: '{}'. Names must be non-empty and contain only alphanumeric characters, hyphens, and underscores.",
                    name
                )
            }
            ValidationError::EmptyDescription => {
                write!(f, "Skill description cannot be empty.")
            }
            ValidationError::EmptyPrompt => {
                write!(f, "Skill prompt cannot be empty.")
            }
            ValidationError::InvalidModel(model) => {
                write!(
                    f,
                    "Invalid model name: '{}'. Model names should not contain special characters.",
                    model
                )
            }
            ValidationError::InvalidTimeout(timeout) => {
                write!(
                    f,
                    "Invalid timeout value: {}. Timeout must be between 1 and 86400 seconds.",
                    timeout
                )
            }
            ValidationError::ConflictingTools(tool) => {
                write!(
                    f,
                    "Tool '{}' is in both allowed_tools and denied_tools.",
                    tool
                )
            }
            ValidationError::InvalidTag(tag) => {
                write!(
                    f,
                    "Invalid tag: '{}'. Tags should be non-empty and not contain special characters.",
                    tag
                )
            }
            ValidationError::InvalidToolName(tool) => {
                write!(
                    f,
                    "Invalid tool name: '{}'. Tool names should be alphanumeric or contain wildcards.",
                    tool
                )
            }
        }
    }
}

/// Skill validator for strict input validation.
pub struct SkillValidator;

impl SkillValidator {
    /// Validates a skill configuration.
    ///
    /// Performs comprehensive validation including:
    /// - Name format validation
    /// - Description presence
    /// - Prompt presence
    /// - Model name validation
    /// - Timeout bounds checking
    /// - Tool configuration consistency
    /// - Tag format validation
    pub fn validate(skill: &Skill) -> ValidationResult {
        let mut result = ValidationResult::valid();

        // Validate name
        if let Err(e) = Self::validate_name(&skill.name) {
            result = result.with_error(e.to_string());
        }

        // Validate description
        if skill.description.trim().is_empty() {
            result = result.with_error(ValidationError::EmptyDescription.to_string());
        }

        // Validate prompt
        if skill.prompt.trim().is_empty() {
            result = result.with_error(ValidationError::EmptyPrompt.to_string());
        }

        // Validate model (if specified)
        if let Some(ref model) = skill.config.model
            && let Err(e) = Self::validate_model(model)
        {
            result = result.with_error(e.to_string());
        }

        // Validate timeout (if specified)
        if let Some(timeout) = skill.config.timeout
            && let Err(e) = Self::validate_timeout(timeout)
        {
            result = result.with_error(e.to_string());
        }

        // Validate tool configuration consistency
        for allowed in &skill.config.allowed_tools {
            if skill
                .config
                .denied_tools
                .iter()
                .any(|d| d.eq_ignore_ascii_case(allowed))
            {
                result = result
                    .with_error(ValidationError::ConflictingTools(allowed.clone()).to_string());
            }
        }

        // Validate tool names
        for tool in &skill.config.allowed_tools {
            if let Err(e) = Self::validate_tool_name(tool) {
                result = result.with_error(e.to_string());
            }
        }
        for tool in &skill.config.denied_tools {
            if let Err(e) = Self::validate_tool_name(tool) {
                result = result.with_error(e.to_string());
            }
        }

        // Validate tags
        for tag in &skill.metadata.tags {
            if let Err(e) = Self::validate_tag(tag) {
                result = result.with_warning(e.to_string());
            }
        }

        // Add warnings for potential issues
        if skill.config.allowed_tools.is_empty() && skill.config.denied_tools.is_empty() {
            result =
                result.with_warning("No tool restrictions configured. All tools will be allowed.");
        }

        if skill.config.timeout.is_none() {
            result = result.with_warning("No timeout configured. Skill may run indefinitely.");
        }

        if skill.prompt.len() > 50000 {
            result = result.with_warning(
                "Prompt is very long (>50KB). Consider breaking it into smaller pieces.",
            );
        }

        result
    }

    /// Validates a skill name.
    pub fn validate_name(name: &str) -> Result<(), ValidationError> {
        if name.trim().is_empty() {
            return Err(ValidationError::InvalidName(name.to_string()));
        }

        // Allow alphanumeric, hyphens, underscores, and spaces
        let valid = name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ');

        if !valid {
            return Err(ValidationError::InvalidName(name.to_string()));
        }

        // Name shouldn't be too long
        if name.len() > 100 {
            return Err(ValidationError::InvalidName(format!(
                "{} (too long, max 100 chars)",
                name
            )));
        }

        Ok(())
    }

    /// Validates a model name.
    pub fn validate_model(model: &str) -> Result<(), ValidationError> {
        if model.trim().is_empty() {
            return Err(ValidationError::InvalidModel(model.to_string()));
        }

        // Model names should be alphanumeric with hyphens and dots
        let valid = model
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/');

        if !valid {
            return Err(ValidationError::InvalidModel(model.to_string()));
        }

        Ok(())
    }

    /// Validates a timeout value.
    pub fn validate_timeout(timeout: u64) -> Result<(), ValidationError> {
        // Minimum 1 second, maximum 24 hours
        if !(1..=86400).contains(&timeout) {
            return Err(ValidationError::InvalidTimeout(timeout));
        }
        Ok(())
    }

    /// Validates a tool name.
    pub fn validate_tool_name(tool: &str) -> Result<(), ValidationError> {
        if tool.trim().is_empty() {
            return Err(ValidationError::InvalidToolName(tool.to_string()));
        }

        // Tool names should be alphanumeric with wildcards allowed
        let valid = tool
            .chars()
            .all(|c| c.is_alphanumeric() || c == '*' || c == '_' || c == '-');

        if !valid {
            return Err(ValidationError::InvalidToolName(tool.to_string()));
        }

        Ok(())
    }

    /// Validates a tag.
    pub fn validate_tag(tag: &str) -> Result<(), ValidationError> {
        if tag.trim().is_empty() {
            return Err(ValidationError::InvalidTag(tag.to_string()));
        }

        // Tags should be simple alphanumeric with hyphens
        let valid = tag
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_');

        if !valid {
            return Err(ValidationError::InvalidTag(tag.to_string()));
        }

        Ok(())
    }

    /// Validates skill invocation arguments.
    ///
    /// Checks that arguments don't contain shell injection patterns.
    pub fn validate_args(args: &[String]) -> ValidationResult {
        let mut result = ValidationResult::valid();

        for arg in args {
            // Check for potential shell injection
            let dangerous_patterns = ["$(", "`", "&&", "||", ";", "|", ">", "<", "\n", "\r"];
            for pattern in &dangerous_patterns {
                if arg.contains(pattern) {
                    result = result.with_warning(format!(
                        "Argument contains potentially dangerous pattern '{}': {}",
                        pattern,
                        arg.chars().take(50).collect::<String>()
                    ));
                }
            }

            // Check for very long arguments
            if arg.len() > 10000 {
                result = result.with_warning(format!(
                    "Argument is very long ({} chars). Consider passing via file.",
                    arg.len()
                ));
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::{SkillConfig, SkillMetadata};
    use std::path::PathBuf;

    fn make_valid_skill() -> Skill {
        Skill::new(
            "test-skill".to_string(),
            "Test Skill".to_string(),
            "A test skill".to_string(),
            "1.0.0".to_string(),
            "You are a test assistant.".to_string(),
            SkillConfig {
                timeout: Some(60),
                ..Default::default()
            },
            SkillMetadata::default(),
            PathBuf::from("/test"),
        )
    }

    #[test]
    fn test_validate_valid_skill() {
        let skill = make_valid_skill();
        let result = SkillValidator::validate(&skill);
        assert!(result.is_valid);
    }

    #[test]
    fn test_validate_empty_name() {
        let mut skill = make_valid_skill();
        skill.name = "".to_string();
        let result = SkillValidator::validate(&skill);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.contains("name")));
    }

    #[test]
    fn test_validate_empty_description() {
        let mut skill = make_valid_skill();
        skill.description = "".to_string();
        let result = SkillValidator::validate(&skill);
        assert!(!result.is_valid);
    }

    #[test]
    fn test_validate_empty_prompt() {
        let mut skill = make_valid_skill();
        skill.prompt = "".to_string();
        let result = SkillValidator::validate(&skill);
        assert!(!result.is_valid);
    }

    #[test]
    fn test_validate_conflicting_tools() {
        let mut skill = make_valid_skill();
        skill.config.allowed_tools = vec!["Read".to_string()];
        skill.config.denied_tools = vec!["Read".to_string()];
        let result = SkillValidator::validate(&skill);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| e.contains("Read")));
    }

    #[test]
    fn test_validate_invalid_timeout() {
        let mut skill = make_valid_skill();
        skill.config.timeout = Some(100000);
        let result = SkillValidator::validate(&skill);
        assert!(!result.is_valid);
    }

    #[test]
    fn test_validate_warnings() {
        let mut skill = make_valid_skill();
        skill.config.timeout = None;
        let result = SkillValidator::validate(&skill);
        assert!(result.is_valid);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_validate_name_special_chars() {
        assert!(SkillValidator::validate_name("valid-name").is_ok());
        assert!(SkillValidator::validate_name("valid_name").is_ok());
        assert!(SkillValidator::validate_name("Valid Name").is_ok());
        assert!(SkillValidator::validate_name("name@invalid").is_err());
        assert!(SkillValidator::validate_name("name!invalid").is_err());
    }

    #[test]
    fn test_validate_tool_name() {
        assert!(SkillValidator::validate_tool_name("Read").is_ok());
        assert!(SkillValidator::validate_tool_name("file*").is_ok());
        assert!(SkillValidator::validate_tool_name("").is_err());
        assert!(SkillValidator::validate_tool_name("tool@invalid").is_err());
    }

    #[test]
    fn test_validate_args() {
        let safe_args = vec!["arg1".to_string(), "arg2".to_string()];
        let result = SkillValidator::validate_args(&safe_args);
        assert!(result.is_valid);
        assert!(result.warnings.is_empty());

        let dangerous_args = vec!["$(rm -rf /)".to_string()];
        let result = SkillValidator::validate_args(&dangerous_args);
        assert!(result.is_valid); // Warnings, not errors
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_validation_result_methods() {
        let result = ValidationResult::valid()
            .with_warning("warning1")
            .with_warning("warning2");
        assert!(result.is_valid);
        assert_eq!(result.warnings.len(), 2);

        let result = ValidationResult::valid().with_error("error1");
        assert!(!result.is_valid);
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn test_into_result() {
        let valid = ValidationResult::valid().with_warning("warn");
        let result = valid.into_result();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec!["warn"]);

        let invalid = ValidationResult::invalid("error");
        let result = invalid.into_result();
        assert!(result.is_err());
    }
}
