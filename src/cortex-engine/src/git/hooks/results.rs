//! Hook Execution Results.
//!
//! Types for representing hook execution results and issues.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::config::HookConfig;
use super::types::GitHook;

/// Result of hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookExecutionResult {
    /// Hook that was executed.
    pub hook: GitHook,
    /// Whether the hook passed.
    pub passed: bool,
    /// Exit code (0 = success).
    pub exit_code: i32,
    /// Issues found during execution.
    pub issues: Vec<HookIssue>,
    /// Warnings (non-blocking).
    pub warnings: Vec<String>,
    /// Execution duration.
    pub duration_ms: u64,
    /// Output from hook execution.
    pub output: String,
    /// Generated commit message (for prepare-commit-msg).
    pub generated_message: Option<String>,
    /// AI review summary (if AI review enabled).
    pub ai_review: Option<String>,
}

impl HookExecutionResult {
    /// Create a new successful result.
    pub fn success(hook: GitHook, duration_ms: u64) -> Self {
        Self {
            hook,
            passed: true,
            exit_code: 0,
            issues: Vec::new(),
            warnings: Vec::new(),
            duration_ms,
            output: String::new(),
            generated_message: None,
            ai_review: None,
        }
    }

    /// Create a new failed result.
    pub fn failure(
        hook: GitHook,
        exit_code: i32,
        issues: Vec<HookIssue>,
        duration_ms: u64,
    ) -> Self {
        Self {
            hook,
            passed: false,
            exit_code,
            issues,
            warnings: Vec::new(),
            duration_ms,
            output: String::new(),
            generated_message: None,
            ai_review: None,
        }
    }

    /// Add output to the result.
    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        self.output = output.into();
        self
    }

    /// Add a warning.
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Add generated message.
    pub fn with_generated_message(mut self, message: impl Into<String>) -> Self {
        self.generated_message = Some(message.into());
        self
    }

    /// Add AI review.
    pub fn with_ai_review(mut self, review: impl Into<String>) -> Self {
        self.ai_review = Some(review.into());
        self
    }

    /// Check if there are any blocking issues.
    pub fn has_blocking_issues(&self) -> bool {
        self.issues
            .iter()
            .any(|i| i.severity == IssueSeverity::Error)
    }
}

/// Issue found during hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookIssue {
    /// Issue category.
    pub category: IssueCategory,
    /// Severity level.
    pub severity: IssueSeverity,
    /// Issue message.
    pub message: String,
    /// File where issue was found.
    pub file: Option<PathBuf>,
    /// Line number.
    pub line: Option<usize>,
    /// Suggestion for fixing.
    pub suggestion: Option<String>,
}

impl HookIssue {
    /// Create a new issue.
    pub fn new(
        category: IssueCategory,
        severity: IssueSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            category,
            severity,
            message: message.into(),
            file: None,
            line: None,
            suggestion: None,
        }
    }

    /// Add file location.
    pub fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Add line number.
    pub fn with_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }

    /// Add suggestion.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Create a secret detection issue.
    pub fn secret(file: impl Into<PathBuf>, line: usize, pattern_name: &str) -> Self {
        Self::new(
            IssueCategory::Security,
            IssueSeverity::Error,
            format!("Potential secret detected: {}", pattern_name),
        )
        .with_file(file)
        .with_line(line)
        .with_suggestion("Remove the secret and use environment variables instead")
    }

    /// Create a TODO/FIXME issue.
    pub fn todo(file: impl Into<PathBuf>, line: usize, content: &str) -> Self {
        Self::new(
            IssueCategory::CodeQuality,
            IssueSeverity::Warning,
            format!("TODO/FIXME found: {}", content),
        )
        .with_file(file)
        .with_line(line)
    }
}

/// Category of hook issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCategory {
    /// Security issues (secrets, vulnerabilities).
    Security,
    /// Code quality issues.
    CodeQuality,
    /// Formatting issues.
    Formatting,
    /// Commit message issues.
    CommitMessage,
    /// Test failures.
    Tests,
    /// Lint errors.
    Lint,
    /// Custom pattern match.
    CustomPattern,
}

impl std::fmt::Display for IssueCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Security => write!(f, "Security"),
            Self::CodeQuality => write!(f, "Code Quality"),
            Self::Formatting => write!(f, "Formatting"),
            Self::CommitMessage => write!(f, "Commit Message"),
            Self::Tests => write!(f, "Tests"),
            Self::Lint => write!(f, "Lint"),
            Self::CustomPattern => write!(f, "Custom Pattern"),
        }
    }
}

/// Severity level of an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    /// Error - blocks commit/push.
    Error,
    /// Warning - reported but doesn't block.
    Warning,
    /// Info - informational only.
    Info,
}

impl std::fmt::Display for IssueSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => write!(f, "ERROR"),
            Self::Warning => write!(f, "WARNING"),
            Self::Info => write!(f, "INFO"),
        }
    }
}

/// Status of an installed hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookStatus {
    /// Hook type.
    pub hook: GitHook,
    /// Whether the hook is installed.
    pub installed: bool,
    /// Whether it's a Cortex hook.
    pub is_cortex: bool,
    /// Hook configuration (if Cortex hook).
    pub config: Option<HookConfig>,
    /// Path to hook file.
    pub path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_issue_severity() {
        let issue = HookIssue::new(IssueCategory::Security, IssueSeverity::Error, "Test issue");

        assert_eq!(issue.category, IssueCategory::Security);
        assert_eq!(issue.severity, IssueSeverity::Error);
    }

    #[test]
    fn test_hook_result() {
        let result = HookExecutionResult::success(GitHook::PreCommit, 100)
            .with_output("Success")
            .with_warning("Minor issue");

        assert!(result.passed);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.warnings.len(), 1);
    }
}
