//! Validation agents for Forge system.
//!
//! This module provides validation agent implementations for code review,
//! security analysis, and quality enforcement in the Forge orchestration system.
//!
//! # Architecture
//!
//! Validation agents follow a trait-based pattern that allows:
//! - Independent validation rules with clear dependencies
//! - Parallel execution of independent agents
//! - Aggregation of results from multiple agents
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::forge::agents::{ValidationAgent, ValidationContext, SecurityAgent};
//!
//! let agent = SecurityAgent::new();
//! let ctx = ValidationContext::new("/path/to/project");
//! let result = agent.validate(&ctx).await?;
//! ```

pub mod aggregator;
pub mod dynamic;
pub mod quality;
pub mod security;
pub mod utils;

pub use aggregator::AggregatorAgent;
pub use dynamic::{create_agent_from_toml, create_agent_from_toml_str, DynamicAgent};
pub use quality::QualityAgent;
pub use security::SecurityAgent;
pub use utils::{
    collect_files_recursive, collect_rust_files, collect_rust_files_recursive,
    collect_source_files, is_test_path, truncate_line, MAX_FILES_LIMIT,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during validation agent operations.
#[derive(Error, Debug)]
pub enum AgentError {
    /// IO error when reading files.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Validation execution error.
    #[error("Validation error: {0}")]
    Validation(String),

    /// Missing dependency - agent requires another agent's results.
    #[error("Missing dependency: agent '{0}' requires results from '{1}'")]
    MissingDependency(String, String),

    /// Serialization/deserialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

// ============================================================================
// Core Types
// ============================================================================

/// Severity level for validation findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational note - no action required.
    Info,
    /// Warning - should be addressed but not blocking.
    Warning,
    /// Error - must be fixed before proceeding.
    Error,
    /// Critical - security or safety issue requiring immediate attention.
    Critical,
}

impl Severity {
    /// Returns display name for the severity level.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Warning => "Warning",
            Self::Error => "Error",
            Self::Critical => "Critical",
        }
    }

    /// Returns emoji/icon for the severity level.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Info => "ℹ️",
            Self::Warning => "⚠️",
            Self::Error => "❌",
            Self::Critical => "🚨",
        }
    }
}

impl Default for Severity {
    fn default() -> Self {
        Self::Info
    }
}

/// Status of a validation run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    /// Validation passed with no issues.
    Passed,
    /// Validation passed with warnings.
    PassedWithWarnings,
    /// Validation failed.
    Failed,
    /// Validation was skipped (e.g., dependency not met).
    Skipped,
}

impl Default for ValidationStatus {
    fn default() -> Self {
        Self::Passed
    }
}

/// Information about a validation rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleInfo {
    /// Unique rule identifier.
    pub id: String,
    /// Human-readable rule name.
    pub name: String,
    /// Detailed description of what the rule checks.
    pub description: String,
    /// Default severity when rule is violated.
    pub default_severity: Severity,
    /// Whether the rule is enabled by default.
    pub enabled_by_default: bool,
}

impl RuleInfo {
    /// Create a new rule info.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            default_severity: Severity::Warning,
            enabled_by_default: true,
        }
    }

    /// Set the default severity.
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.default_severity = severity;
        self
    }

    /// Set whether enabled by default.
    pub fn enabled_by_default(mut self, enabled: bool) -> Self {
        self.enabled_by_default = enabled;
        self
    }
}

/// A single finding from a validation rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// The rule that generated this finding.
    pub rule_id: String,
    /// Severity of this finding.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// File path where the issue was found (if applicable).
    pub file: Option<PathBuf>,
    /// Line number where the issue was found (if applicable).
    pub line: Option<u32>,
    /// Column number where the issue was found (if applicable).
    pub column: Option<u32>,
    /// Code snippet showing the issue (if applicable).
    pub snippet: Option<String>,
    /// Suggested fix (if available).
    pub suggestion: Option<String>,
}

impl Finding {
    /// Create a new finding.
    pub fn new(rule_id: impl Into<String>, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            rule_id: rule_id.into(),
            severity,
            message: message.into(),
            file: None,
            line: None,
            column: None,
            snippet: None,
            suggestion: None,
        }
    }

    /// Set the file location.
    pub fn at_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Set the line number.
    pub fn at_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    /// Set the column number.
    pub fn at_column(mut self, column: u32) -> Self {
        self.column = Some(column);
        self
    }

    /// Set a code snippet.
    pub fn with_snippet(mut self, snippet: impl Into<String>) -> Self {
        self.snippet = Some(snippet.into());
        self
    }

    /// Set a suggested fix.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Format finding as a human-readable string.
    pub fn format(&self) -> String {
        let mut result = format!(
            "{} [{}] {}",
            self.severity.icon(),
            self.rule_id,
            self.message
        );

        if let Some(ref file) = self.file {
            result.push_str(&format!("\n  at {}", file.display()));
            if let Some(line) = self.line {
                result.push_str(&format!(":{}", line));
                if let Some(col) = self.column {
                    result.push_str(&format!(":{}", col));
                }
            }
        }

        if let Some(ref snippet) = self.snippet {
            result.push_str(&format!("\n  > {}", snippet));
        }

        if let Some(ref suggestion) = self.suggestion {
            result.push_str(&format!("\n  💡 {}", suggestion));
        }

        result
    }
}

/// Result of a validation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// ID of the agent that produced this result.
    pub agent_id: String,
    /// Overall validation status.
    pub status: ValidationStatus,
    /// All findings from the validation.
    pub findings: Vec<Finding>,
    /// Duration of the validation in milliseconds.
    pub duration_ms: u64,
    /// Timestamp when validation completed.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Summary message.
    pub summary: String,
}

impl ValidationResult {
    /// Create a new validation result.
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            status: ValidationStatus::Passed,
            findings: Vec::new(),
            duration_ms: 0,
            timestamp: chrono::Utc::now(),
            summary: String::new(),
        }
    }

    /// Add a finding and update status accordingly.
    pub fn add_finding(&mut self, finding: Finding) {
        // Update status based on finding severity
        match finding.severity {
            Severity::Critical | Severity::Error => {
                self.status = ValidationStatus::Failed;
            }
            Severity::Warning => {
                if self.status == ValidationStatus::Passed {
                    self.status = ValidationStatus::PassedWithWarnings;
                }
            }
            Severity::Info => {
                // Info doesn't change status
            }
        }
        self.findings.push(finding);
    }

    /// Set the duration.
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    /// Set the summary.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Count findings by severity.
    pub fn count_by_severity(&self) -> HashMap<Severity, usize> {
        let mut counts = HashMap::new();
        for finding in &self.findings {
            *counts.entry(finding.severity).or_insert(0) += 1;
        }
        counts
    }

    /// Check if validation passed (no errors or criticals).
    pub fn is_passed(&self) -> bool {
        matches!(
            self.status,
            ValidationStatus::Passed | ValidationStatus::PassedWithWarnings
        )
    }

    /// Get all critical findings.
    pub fn critical_findings(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Critical)
            .collect()
    }

    /// Get all error findings.
    pub fn error_findings(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect()
    }
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self::new("unknown")
    }
}

// ============================================================================
// Agent Configuration
// ============================================================================

/// Configuration for a validation agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent-specific settings.
    #[serde(default)]
    pub settings: HashMap<String, serde_json::Value>,

    /// Enabled rules (if None, all default rules are enabled).
    #[serde(default)]
    pub enabled_rules: Option<Vec<String>>,

    /// Disabled rules.
    #[serde(default)]
    pub disabled_rules: Vec<String>,

    /// Rule-specific severity overrides.
    #[serde(default)]
    pub severity_overrides: HashMap<String, Severity>,

    /// Paths to exclude from validation.
    #[serde(default)]
    pub exclude_paths: Vec<String>,

    /// File patterns to include (glob).
    #[serde(default)]
    pub include_patterns: Vec<String>,
}

impl AgentConfig {
    /// Create a new config with defaults.
    pub fn new() -> Self {
        Self {
            settings: HashMap::new(),
            enabled_rules: None,
            disabled_rules: Vec::new(),
            severity_overrides: HashMap::new(),
            exclude_paths: vec![
                "target/**".to_string(),
                "node_modules/**".to_string(),
                ".git/**".to_string(),
                "vendor/**".to_string(),
            ],
            include_patterns: vec!["**/*.rs".to_string()],
        }
    }

    /// Check if a rule is enabled.
    pub fn is_rule_enabled(&self, rule_id: &str, default_enabled: bool) -> bool {
        // Check if explicitly disabled
        if self.disabled_rules.contains(&rule_id.to_string()) {
            return false;
        }

        // Check if explicitly enabled
        if let Some(ref enabled) = self.enabled_rules {
            return enabled.contains(&rule_id.to_string());
        }

        // Use default
        default_enabled
    }

    /// Get severity for a rule (with override support).
    pub fn get_severity(&self, rule_id: &str, default_severity: Severity) -> Severity {
        self.severity_overrides
            .get(rule_id)
            .copied()
            .unwrap_or(default_severity)
    }

    /// Get a setting value.
    pub fn get_setting<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.settings
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Set a setting value.
    pub fn set_setting(&mut self, key: impl Into<String>, value: impl Serialize) {
        if let Ok(v) = serde_json::to_value(value) {
            self.settings.insert(key.into(), v);
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Validation Context
// ============================================================================

/// Context provided to validation agents during execution.
#[derive(Debug, Clone)]
pub struct ValidationContext {
    /// Root path of the project being validated.
    pub project_path: PathBuf,

    /// Agent-specific configuration.
    pub config: AgentConfig,

    /// Results from previously completed agents (for dependencies).
    pub previous_results: HashMap<String, ValidationResult>,
}

impl ValidationContext {
    /// Create a new validation context.
    pub fn new(project_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: project_path.into(),
            config: AgentConfig::new(),
            previous_results: HashMap::new(),
        }
    }

    /// Create with a specific configuration.
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Add a previous result (for dependent agents).
    pub fn with_previous_result(mut self, result: ValidationResult) -> Self {
        self.previous_results
            .insert(result.agent_id.clone(), result);
        self
    }

    /// Get a previous result by agent ID.
    pub fn get_previous_result(&self, agent_id: &str) -> Option<&ValidationResult> {
        self.previous_results.get(agent_id)
    }

    /// Check if a file path should be excluded.
    pub fn should_exclude(&self, path: &std::path::Path) -> bool {
        let path_str = path.to_string_lossy();
        for pattern in &self.config.exclude_paths {
            if glob_match(pattern, &path_str) {
                return true;
            }
        }
        false
    }

    /// Check if a file path matches include patterns.
    pub fn matches_include(&self, path: &std::path::Path) -> bool {
        if self.config.include_patterns.is_empty() {
            return true;
        }
        let path_str = path.to_string_lossy();
        for pattern in &self.config.include_patterns {
            if glob_match(pattern, &path_str) {
                return true;
            }
        }
        false
    }
}

// ============================================================================
// ValidationAgent Trait
// ============================================================================

/// Trait for validation agents.
///
/// Validation agents perform specific checks on a codebase and produce
/// findings with associated severities. Agents can declare dependencies
/// on other agents, allowing for ordered execution and access to previous
/// results.
#[async_trait]
pub trait ValidationAgent: Send + Sync {
    /// Returns the unique identifier for this agent.
    fn id(&self) -> &str;

    /// Returns the human-readable name of this agent.
    fn name(&self) -> &str;

    /// Returns the IDs of agents this agent depends on.
    ///
    /// The orchestrator will ensure all dependencies have completed
    /// before running this agent, and their results will be available
    /// in `ValidationContext::previous_results`.
    fn dependencies(&self) -> Vec<String> {
        Vec::new()
    }

    /// Returns information about all rules this agent can check.
    fn rules(&self) -> Vec<RuleInfo>;

    /// Performs validation and returns results.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The validation context containing project path, config, and previous results.
    ///
    /// # Returns
    ///
    /// A `ValidationResult` containing all findings from this agent.
    async fn validate(&self, ctx: &ValidationContext) -> Result<ValidationResult, AgentError>;
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Simple glob pattern matching.
///
/// Supports:
/// - `*` matches any sequence of characters except `/`
/// - `**` matches any sequence of characters including `/`
/// - `?` matches any single character
pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();

    glob_match_parts(&pattern_parts, &path_parts)
}

pub(crate) fn glob_match_parts(pattern: &[&str], path: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }

    let first_pattern = pattern[0];

    if first_pattern == "**" {
        // Try matching ** against zero or more path components
        for i in 0..=path.len() {
            if glob_match_parts(&pattern[1..], &path[i..]) {
                return true;
            }
        }
        return false;
    }

    if path.is_empty() {
        return false;
    }

    // Match first component
    if !glob_match_component(first_pattern, path[0]) {
        return false;
    }

    glob_match_parts(&pattern[1..], &path[1..])
}

pub(crate) fn glob_match_component(pattern: &str, component: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let mut p_chars = pattern.chars().peekable();
    let mut c_chars = component.chars().peekable();

    while let Some(p) = p_chars.next() {
        match p {
            '*' => {
                // * matches any sequence
                if p_chars.peek().is_none() {
                    return true;
                }
                // Try matching rest of pattern against each suffix
                let remaining_pattern: String = p_chars.collect();
                let remaining_component: String = c_chars.collect();
                for i in 0..=remaining_component.len() {
                    if glob_match_component(&remaining_pattern, &remaining_component[i..]) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                // ? matches any single character
                if c_chars.next().is_none() {
                    return false;
                }
            }
            _ => {
                // Literal match
                match c_chars.next() {
                    Some(c) if c == p => {}
                    _ => return false,
                }
            }
        }
    }

    c_chars.peek().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical > Severity::Error);
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn test_finding_builder() {
        let finding = Finding::new("test-rule", Severity::Warning, "Test message")
            .at_file("/path/to/file.rs")
            .at_line(42)
            .with_suggestion("Fix this");

        assert_eq!(finding.rule_id, "test-rule");
        assert_eq!(finding.severity, Severity::Warning);
        assert_eq!(finding.file, Some(PathBuf::from("/path/to/file.rs")));
        assert_eq!(finding.line, Some(42));
        assert_eq!(finding.suggestion, Some("Fix this".to_string()));
    }

    #[test]
    fn test_validation_result_status_update() {
        let mut result = ValidationResult::new("test");
        assert!(result.is_passed());

        result.add_finding(Finding::new("rule1", Severity::Warning, "Warning"));
        assert_eq!(result.status, ValidationStatus::PassedWithWarnings);
        assert!(result.is_passed());

        result.add_finding(Finding::new("rule2", Severity::Error, "Error"));
        assert_eq!(result.status, ValidationStatus::Failed);
        assert!(!result.is_passed());
    }

    #[test]
    fn test_agent_config_rule_enabled() {
        let mut config = AgentConfig::new();
        assert!(config.is_rule_enabled("any_rule", true));
        assert!(!config.is_rule_enabled("any_rule", false));

        config.disabled_rules.push("disabled_rule".to_string());
        assert!(!config.is_rule_enabled("disabled_rule", true));
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.rs", "file.rs"));
        assert!(!glob_match("*.rs", "file.txt"));
        assert!(glob_match("**/*.rs", "src/lib.rs"));
        assert!(glob_match("**/*.rs", "src/nested/deep/file.rs"));
        assert!(glob_match("target/**", "target/debug/build"));
        assert!(glob_match("file?.rs", "file1.rs"));
        assert!(!glob_match("file?.rs", "file12.rs"));
    }

    #[test]
    fn test_context_exclusions() {
        let ctx = ValidationContext::new("/project");
        assert!(ctx.should_exclude(std::path::Path::new("target/debug/build")));
        assert!(ctx.should_exclude(std::path::Path::new("node_modules/package")));
        assert!(!ctx.should_exclude(std::path::Path::new("src/lib.rs")));
    }
}
