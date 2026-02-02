//! Hook Configuration.
//!
//! Configuration types for git hooks including pattern checks.

use serde::{Deserialize, Serialize};

/// Configuration for a git hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Whether the hook is enabled.
    pub enabled: bool,
    /// Enable AI-powered review.
    pub ai_review: bool,
    /// Block commit/push on issues found.
    pub block_on_issues: bool,
    /// AI model to use for review.
    pub model: Option<String>,
    /// Custom prompt for AI review.
    pub custom_prompt: Option<String>,
    /// Run linters before commit.
    pub run_linters: bool,
    /// Check for secrets.
    pub check_secrets: bool,
    /// Check for TODO/FIXME comments.
    pub check_todos: bool,
    /// Validate conventional commits format.
    pub conventional_commits: bool,
    /// Required commit message sections.
    pub required_sections: Vec<String>,
    /// Minimum commit message length.
    pub min_message_length: usize,
    /// Maximum commit message length for first line.
    pub max_subject_length: usize,
    /// Custom patterns to check for.
    pub custom_patterns: Vec<PatternCheck>,
    /// Timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ai_review: false,
            block_on_issues: false,
            model: None,
            custom_prompt: None,
            run_linters: true,
            check_secrets: true,
            check_todos: false,
            conventional_commits: true,
            required_sections: Vec::new(),
            min_message_length: 10,
            max_subject_length: 72,
            custom_patterns: Vec::new(),
            timeout_secs: 60,
        }
    }
}

impl HookConfig {
    /// Create a new config with AI review enabled.
    pub fn with_ai_review(mut self, enabled: bool) -> Self {
        self.ai_review = enabled;
        self
    }

    /// Set blocking mode.
    pub fn with_blocking(mut self, block: bool) -> Self {
        self.block_on_issues = block;
        self
    }

    /// Set the AI model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Enable/disable secret checking.
    pub fn with_secrets_check(mut self, enabled: bool) -> Self {
        self.check_secrets = enabled;
        self
    }

    /// Enable/disable TODO checking.
    pub fn with_todo_check(mut self, enabled: bool) -> Self {
        self.check_todos = enabled;
        self
    }

    /// Set conventional commits requirement.
    pub fn with_conventional_commits(mut self, enabled: bool) -> Self {
        self.conventional_commits = enabled;
        self
    }

    /// Add a custom pattern check.
    pub fn with_pattern(mut self, pattern: PatternCheck) -> Self {
        self.custom_patterns.push(pattern);
        self
    }
}

/// Custom pattern to check in code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternCheck {
    /// Pattern name.
    pub name: String,
    /// Regex pattern.
    pub pattern: String,
    /// Error message if found.
    pub message: String,
    /// Whether to block on match.
    pub blocking: bool,
    /// File patterns to check (glob).
    pub file_patterns: Vec<String>,
}

impl PatternCheck {
    /// Create a new pattern check.
    pub fn new(
        name: impl Into<String>,
        pattern: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            pattern: pattern.into(),
            message: message.into(),
            blocking: true,
            file_patterns: Vec::new(),
        }
    }

    /// Set blocking behavior.
    pub fn with_blocking(mut self, blocking: bool) -> Self {
        self.blocking = blocking;
        self
    }

    /// Add file patterns.
    pub fn with_file_patterns(mut self, patterns: Vec<String>) -> Self {
        self.file_patterns = patterns;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_config_builder() {
        let config = HookConfig::default()
            .with_ai_review(true)
            .with_secrets_check(true)
            .with_blocking(true)
            .with_conventional_commits(true);

        assert!(config.ai_review);
        assert!(config.check_secrets);
        assert!(config.block_on_issues);
        assert!(config.conventional_commits);
    }

    #[test]
    fn test_pattern_check() {
        let pattern = PatternCheck::new("console.log", r"console\.log", "Remove debug logging")
            .with_blocking(true)
            .with_file_patterns(vec!["*.js".to_string(), "*.ts".to_string()]);

        assert!(pattern.blocking);
        assert_eq!(pattern.file_patterns.len(), 2);
    }
}
