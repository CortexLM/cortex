//! Agent validation protocol for Forge orchestration.
//!
//! This module defines protocol types for agent validation results,
//! including status, findings, and aggregated responses.
//!
//! Note: The primary types for validation are re-exported at `cortex_agents::forge`
//! from the `agents` module. This protocol module provides additional types
//! for the orchestrator, including `ForgeResponse` and `Location` types.
//!
//! # Example
//!
//! ```rust
//! use cortex_agents::forge::{
//!     ValidationStatus, ValidationResult, Finding, Severity, RuleInfo,
//! };
//!
//! // Create a validation result using the builder pattern
//! let mut result = ValidationResult::new("security-scanner");
//!
//! // Add a finding
//! result.add_finding(Finding::new("SEC001", Severity::Warning, "Potential issue detected"));
//!
//! // Check result status
//! assert!(result.is_passed()); // No errors or criticals = passed
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Severity level for validation findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational note.
    Info,
    /// Warning that should be addressed.
    Warning,
    /// Error - must be fixed before proceeding.
    Error,
    /// Critical - security or safety issue requiring immediate attention.
    Critical,
}

impl Severity {
    /// Check if this severity is an error.
    pub fn is_error(&self) -> bool {
        matches!(self, Severity::Error | Severity::Critical)
    }

    /// Check if this severity is a warning or higher.
    pub fn is_warning_or_higher(&self) -> bool {
        matches!(
            self,
            Severity::Error | Severity::Critical | Severity::Warning
        )
    }

    /// Get a human-readable label for this severity.
    pub fn label(&self) -> &'static str {
        match self {
            Severity::Critical => "CRITICAL",
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
            Severity::Info => "INFO",
        }
    }
}

impl Default for Severity {
    fn default() -> Self {
        Severity::Info
    }
}

/// Location of a finding in the codebase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// File path (relative to project root).
    pub file: String,
    /// Line number (1-indexed).
    pub line: Option<u32>,
    /// Column number (1-indexed).
    pub column: Option<u32>,
    /// End line number for multi-line findings.
    pub end_line: Option<u32>,
    /// End column number.
    pub end_column: Option<u32>,
}

impl Location {
    /// Create a new location for a file.
    pub fn file(path: impl Into<String>) -> Self {
        Self {
            file: path.into(),
            line: None,
            column: None,
            end_line: None,
            end_column: None,
        }
    }

    /// Create a location with a specific line.
    pub fn at_line(path: impl Into<String>, line: u32) -> Self {
        Self {
            file: path.into(),
            line: Some(line),
            column: None,
            end_line: None,
            end_column: None,
        }
    }

    /// Create a location with line and column.
    pub fn at_position(path: impl Into<String>, line: u32, column: u32) -> Self {
        Self {
            file: path.into(),
            line: Some(line),
            column: Some(column),
            end_line: None,
            end_column: None,
        }
    }

    /// Set the end position for a range.
    pub fn with_end(mut self, end_line: u32, end_column: Option<u32>) -> Self {
        self.end_line = Some(end_line);
        self.end_column = end_column;
        self
    }

    /// Format location as a string (e.g., "src/main.rs:10:5").
    pub fn display(&self) -> String {
        let mut result = self.file.clone();
        if let Some(line) = self.line {
            result.push_str(&format!(":{line}"));
            if let Some(col) = self.column {
                result.push_str(&format!(":{col}"));
            }
        }
        result
    }
}

/// A validation finding from an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Severity of the finding.
    pub severity: Severity,
    /// Human-readable message describing the issue.
    pub message: String,
    /// Location in the codebase (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    /// Suggested fix or action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// Rule that triggered this finding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
}

impl Finding {
    /// Create a new critical finding.
    pub fn critical(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Critical,
            message: message.into(),
            location: None,
            suggestion: None,
            rule_id: None,
        }
    }

    /// Create a new error finding.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            location: None,
            suggestion: None,
            rule_id: None,
        }
    }

    /// Create a new warning finding.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            location: None,
            suggestion: None,
            rule_id: None,
        }
    }

    /// Create a new info finding.
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            message: message.into(),
            location: None,
            suggestion: None,
            rule_id: None,
        }
    }

    /// Set the location for this finding.
    pub fn with_location(mut self, location: Location) -> Self {
        self.location = Some(location);
        self
    }

    /// Set a suggestion for this finding.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Set the rule ID for this finding.
    pub fn with_rule(mut self, rule_id: impl Into<String>) -> Self {
        self.rule_id = Some(rule_id.into());
        self
    }
}

/// Information about a rule that was applied during validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleInfo {
    /// Unique identifier for the rule.
    pub id: String,
    /// Human-readable name of the rule.
    pub name: String,
    /// Whether the rule was enabled during validation.
    pub enabled: bool,
}

impl RuleInfo {
    /// Create a new enabled rule info.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            enabled: true,
        }
    }

    /// Create a disabled rule info.
    pub fn disabled(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            enabled: false,
        }
    }
}

/// Overall validation status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ValidationStatus {
    /// All validations passed.
    #[default]
    Pass,
    /// At least one warning but no errors.
    Warning,
    /// At least one error.
    Fail,
}

impl ValidationStatus {
    /// Check if this status represents a passing validation.
    pub fn is_pass(&self) -> bool {
        matches!(self, ValidationStatus::Pass)
    }

    /// Check if this status represents a failure.
    pub fn is_fail(&self) -> bool {
        matches!(self, ValidationStatus::Fail)
    }

    /// Combine two statuses, taking the worst result.
    pub fn combine(self, other: ValidationStatus) -> ValidationStatus {
        match (self, other) {
            (ValidationStatus::Fail, _) | (_, ValidationStatus::Fail) => ValidationStatus::Fail,
            (ValidationStatus::Warning, _) | (_, ValidationStatus::Warning) => {
                ValidationStatus::Warning
            }
            _ => ValidationStatus::Pass,
        }
    }

    /// Get a human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            ValidationStatus::Pass => "PASS",
            ValidationStatus::Warning => "WARNING",
            ValidationStatus::Fail => "FAIL",
        }
    }
}

/// Result of a single agent's validation run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Overall status of the validation.
    pub status: ValidationStatus,
    /// ID of the agent that produced this result.
    pub agent_id: String,
    /// Rules that were applied during validation.
    pub rules_applied: Vec<RuleInfo>,
    /// Findings from the validation.
    pub findings: Vec<Finding>,
    /// Timestamp when validation completed.
    pub timestamp: DateTime<Utc>,
}

impl ValidationResult {
    /// Create a passing validation result.
    pub fn pass(agent_id: impl Into<String>) -> Self {
        Self {
            status: ValidationStatus::Pass,
            agent_id: agent_id.into(),
            rules_applied: Vec::new(),
            findings: Vec::new(),
            timestamp: Utc::now(),
        }
    }

    /// Create a failing validation result with findings.
    pub fn fail(agent_id: impl Into<String>, findings: Vec<Finding>) -> Self {
        Self {
            status: ValidationStatus::Fail,
            agent_id: agent_id.into(),
            rules_applied: Vec::new(),
            findings,
            timestamp: Utc::now(),
        }
    }

    /// Create a validation result from findings (auto-determines status).
    pub fn from_findings(agent_id: impl Into<String>, findings: Vec<Finding>) -> Self {
        let status = Self::compute_status(&findings);
        Self {
            status,
            agent_id: agent_id.into(),
            rules_applied: Vec::new(),
            findings,
            timestamp: Utc::now(),
        }
    }

    /// Compute the validation status based on findings.
    fn compute_status(findings: &[Finding]) -> ValidationStatus {
        let has_critical_or_error = findings.iter().any(|f| f.severity.is_error());
        let has_warning = findings.iter().any(|f| f.severity == Severity::Warning);

        if has_critical_or_error {
            ValidationStatus::Fail
        } else if has_warning {
            ValidationStatus::Warning
        } else {
            ValidationStatus::Pass
        }
    }

    /// Add rules that were applied.
    pub fn with_rules(mut self, rules: Vec<RuleInfo>) -> Self {
        self.rules_applied = rules;
        self
    }

    /// Add a single finding.
    pub fn add_finding(&mut self, finding: Finding) {
        self.findings.push(finding);
        self.status = Self::compute_status(&self.findings);
    }

    /// Check if this result is a success (pass or warning).
    pub fn is_success(&self) -> bool {
        !self.status.is_fail()
    }

    /// Get the count of findings by severity.
    pub fn finding_counts(&self) -> (usize, usize, usize) {
        let errors = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count();
        let warnings = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count();
        let infos = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Info)
            .count();
        (errors, warnings, infos)
    }
}

/// Aggregated response from the Forge orchestration system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeResponse {
    /// Overall status (worst of all agent results).
    pub status: ValidationStatus,
    /// Results from each agent.
    pub results: Vec<ValidationResult>,
    /// Total execution time in milliseconds.
    pub execution_time_ms: u64,
    /// Timestamp when orchestration completed.
    pub completed_at: DateTime<Utc>,
    /// Any errors that occurred during orchestration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

impl ForgeResponse {
    /// Create a new response from agent results.
    pub fn new(results: Vec<ValidationResult>, execution_time_ms: u64) -> Self {
        let status = results
            .iter()
            .fold(ValidationStatus::Pass, |acc, r| acc.combine(r.status));

        Self {
            status,
            results,
            execution_time_ms,
            completed_at: Utc::now(),
            errors: Vec::new(),
        }
    }

    /// Create a response with orchestration errors.
    pub fn with_errors(mut self, errors: Vec<String>) -> Self {
        self.errors = errors;
        if !self.errors.is_empty() && self.status != ValidationStatus::Fail {
            self.status = ValidationStatus::Fail;
        }
        self
    }

    /// Check if the overall validation passed.
    pub fn is_success(&self) -> bool {
        self.status.is_pass() && self.errors.is_empty()
    }

    /// Get total number of findings across all agents.
    pub fn total_findings(&self) -> usize {
        self.results.iter().map(|r| r.findings.len()).sum()
    }

    /// Get aggregated finding counts (errors, warnings, infos).
    pub fn total_finding_counts(&self) -> (usize, usize, usize) {
        self.results.iter().fold((0, 0, 0), |(e, w, i), r| {
            let (re, rw, ri) = r.finding_counts();
            (e + re, w + rw, i + ri)
        })
    }

    /// Convert to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical.is_error());
        assert!(Severity::Error.is_error());
        assert!(Severity::Critical.is_warning_or_higher());
        assert!(Severity::Error.is_warning_or_higher());
        assert!(Severity::Warning.is_warning_or_higher());
        assert!(!Severity::Info.is_warning_or_higher());
    }

    #[test]
    fn test_location_display() {
        let loc = Location::file("src/main.rs");
        assert_eq!(loc.display(), "src/main.rs");

        let loc = Location::at_line("src/main.rs", 10);
        assert_eq!(loc.display(), "src/main.rs:10");

        let loc = Location::at_position("src/main.rs", 10, 5);
        assert_eq!(loc.display(), "src/main.rs:10:5");
    }

    #[test]
    fn test_finding_builders() {
        let finding = Finding::error("Something went wrong")
            .with_location(Location::at_line("src/lib.rs", 42))
            .with_suggestion("Fix the issue")
            .with_rule("E001");

        assert_eq!(finding.severity, Severity::Error);
        assert_eq!(finding.message, "Something went wrong");
        assert!(finding.location.is_some());
        assert_eq!(finding.suggestion.as_deref(), Some("Fix the issue"));
        assert_eq!(finding.rule_id.as_deref(), Some("E001"));
    }

    #[test]
    fn test_validation_status_combine() {
        assert_eq!(
            ValidationStatus::Pass.combine(ValidationStatus::Pass),
            ValidationStatus::Pass
        );
        assert_eq!(
            ValidationStatus::Pass.combine(ValidationStatus::Warning),
            ValidationStatus::Warning
        );
        assert_eq!(
            ValidationStatus::Warning.combine(ValidationStatus::Fail),
            ValidationStatus::Fail
        );
        assert_eq!(
            ValidationStatus::Fail.combine(ValidationStatus::Pass),
            ValidationStatus::Fail
        );
    }

    #[test]
    fn test_validation_result_from_findings() {
        let findings = vec![
            Finding::error("Error 1"),
            Finding::warning("Warning 1"),
            Finding::info("Info 1"),
        ];

        let result = ValidationResult::from_findings("test-agent", findings);
        assert_eq!(result.status, ValidationStatus::Fail);
        assert_eq!(result.finding_counts(), (1, 1, 1));
    }

    #[test]
    fn test_validation_result_auto_status() {
        let mut result = ValidationResult::pass("agent");
        result.add_finding(Finding::warning("A warning"));
        assert_eq!(result.status, ValidationStatus::Warning);

        result.add_finding(Finding::error("An error"));
        assert_eq!(result.status, ValidationStatus::Fail);
    }

    #[test]
    fn test_forge_response_aggregation() {
        let results = vec![
            ValidationResult::pass("agent-1"),
            ValidationResult::from_findings("agent-2", vec![Finding::warning("warn")]),
        ];

        let response = ForgeResponse::new(results, 100);
        assert_eq!(response.status, ValidationStatus::Warning);
        assert_eq!(response.total_findings(), 1);
        assert!(response.errors.is_empty());
    }

    #[test]
    fn test_forge_response_json_roundtrip() {
        let response = ForgeResponse::new(vec![ValidationResult::pass("test")], 50);

        let json = response.to_json().expect("should serialize");
        let parsed = ForgeResponse::from_json(&json).expect("should deserialize");

        assert_eq!(parsed.status, response.status);
        assert_eq!(parsed.execution_time_ms, response.execution_time_ms);
    }
}
