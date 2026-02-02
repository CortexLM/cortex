//! Dynamic validation agent for Forge system.
//!
//! This module provides a configurable agent that loads its rules from TOML files,
//! allowing users to define custom validation agents without writing Rust code.

use async_trait::async_trait;
use regex::Regex;
use std::path::Path;
use std::time::Instant;
use tokio::fs;
use tokio::io::AsyncBufReadExt;

use super::{
    glob_match,
    utils::{collect_source_files, is_test_path, truncate_line},
    AgentError, Finding, RuleInfo, Severity, ValidationAgent, ValidationContext, ValidationResult,
};
use crate::forge::config::{AgentRulesFile, DynamicRuleDefinition};

// ============================================================================
// DynamicAgent
// ============================================================================

/// A validation agent that loads its rules dynamically from TOML configuration.
///
/// This allows users to define custom validation agents without writing Rust code.
/// Rules are loaded from `.cortex/forge/agents/<agent_id>/rules.toml`.
#[derive(Debug, Clone)]
pub struct DynamicAgent {
    /// Agent identifier.
    id: String,
    /// Human-readable name.
    name: String,
    /// Description of what this agent does.
    description: String,
    /// Agents this agent depends on.
    dependencies: Vec<String>,
    /// Rule definitions loaded from TOML.
    rules: std::collections::HashMap<String, DynamicRuleDefinition>,
    /// Default severity for rules that don't specify one.
    default_severity: Severity,
}

impl DynamicAgent {
    /// Create a new dynamic agent from an AgentRulesFile configuration.
    pub fn from_rules_file(rules_file: AgentRulesFile) -> Self {
        Self {
            id: rules_file.agent.id.clone(),
            name: rules_file.agent.name.clone(),
            description: rules_file.agent.description.clone(),
            dependencies: Vec::new(),
            rules: rules_file.rules,
            default_severity: Severity::Warning,
        }
    }

    /// Create a new dynamic agent with the specified ID.
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            description: String::new(),
            dependencies: Vec::new(),
            rules: std::collections::HashMap::new(),
            default_severity: Severity::Warning,
        }
    }

    /// Set the agent name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the agent description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Add a dependency on another agent.
    pub fn depends_on(mut self, agent_id: impl Into<String>) -> Self {
        self.dependencies.push(agent_id.into());
        self
    }

    /// Set multiple dependencies.
    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    /// Add a rule definition.
    pub fn with_rule(mut self, rule_id: impl Into<String>, rule: DynamicRuleDefinition) -> Self {
        self.rules.insert(rule_id.into(), rule);
        self
    }

    /// Set the default severity for rules without explicit severity.
    pub fn with_default_severity(mut self, severity: Severity) -> Self {
        self.default_severity = severity;
        self
    }

    /// Load an agent from a TOML file path.
    pub async fn load_from_file(path: &Path) -> Result<Self, AgentError> {
        let content = fs::read_to_string(path).await.map_err(AgentError::Io)?;
        let rules_file: AgentRulesFile = toml::from_str(&content)
            .map_err(|e| AgentError::Config(format!("Failed to parse rules file: {}", e)))?;
        Ok(Self::from_rules_file(rules_file))
    }

    /// Parse severity string to Severity enum.
    fn parse_severity(&self, severity_str: Option<&str>) -> Severity {
        match severity_str.map(|s| s.to_lowercase()).as_deref() {
            Some("critical") => Severity::Critical,
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warning,
            Some("info") => Severity::Info,
            _ => self.default_severity,
        }
    }

    /// Run pattern-based rules on source files.
    async fn run_pattern_rules(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let files = collect_source_files(&ctx.project_path, ctx).await?;

        for (rule_id, rule_def) in &self.rules {
            if !rule_def.enabled || rule_def.patterns.is_empty() {
                continue;
            }

            // Check if rule is enabled in agent config
            if !ctx.config.is_rule_enabled(rule_id, true) {
                continue;
            }

            let severity = self.parse_severity(rule_def.severity.as_deref());
            let patterns: Vec<Regex> = rule_def
                .patterns
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect();

            if patterns.is_empty() {
                continue;
            }

            for file_path in &files {
                // Check exclude patterns
                if should_exclude_file(file_path, &rule_def.exclude_patterns) {
                    continue;
                }

                // Check allowed files (if specified, file must match)
                if !rule_def.allowed_files.is_empty()
                    && !is_file_allowed(file_path, &rule_def.allowed_files)
                {
                    continue;
                }

                // Check test file handling
                if rule_def.allow_in_tests && is_test_path(file_path) {
                    continue;
                }

                if let Err(e) = self
                    .scan_file_for_patterns(
                        file_path,
                        rule_id,
                        &rule_def.description,
                        severity,
                        &patterns,
                        findings,
                    )
                    .await
                {
                    tracing::warn!(
                        "Failed to scan file {} for rule {}: {}",
                        file_path.display(),
                        rule_id,
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Scan a single file for pattern matches.
    async fn scan_file_for_patterns(
        &self,
        file_path: &Path,
        rule_id: &str,
        description: &str,
        severity: Severity,
        patterns: &[Regex],
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let file = fs::File::open(file_path).await?;
        let reader = tokio::io::BufReader::new(file);
        let mut lines = reader.lines();
        let mut line_num: u32 = 0;

        while let Some(line) = lines.next_line().await? {
            line_num += 1;

            for pattern in patterns {
                if pattern.is_match(&line) {
                    let message = if description.is_empty() {
                        format!("Pattern match found for rule '{}'", rule_id)
                    } else {
                        description.to_string()
                    };

                    findings.push(
                        Finding::new(rule_id, severity, message)
                            .at_file(file_path)
                            .at_line(line_num)
                            .with_snippet(truncate_line(&line, 80)),
                    );
                    break; // One finding per line per rule
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl ValidationAgent for DynamicAgent {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn dependencies(&self) -> Vec<String> {
        self.dependencies.clone()
    }

    fn rules(&self) -> Vec<RuleInfo> {
        self.rules
            .iter()
            .map(|(id, def)| {
                let severity = self.parse_severity(def.severity.as_deref());
                RuleInfo::new(id, &def.description, &def.description)
                    .with_severity(severity)
                    .enabled_by_default(def.enabled)
            })
            .collect()
    }

    async fn validate(&self, ctx: &ValidationContext) -> Result<ValidationResult, AgentError> {
        let start = Instant::now();
        let mut result = ValidationResult::new(&self.id);
        let mut findings = Vec::new();

        // Run pattern-based rules
        self.run_pattern_rules(ctx, &mut findings).await?;

        // Add all findings to result
        for finding in findings {
            result.add_finding(finding);
        }

        // Set duration and summary
        let duration_ms = start.elapsed().as_millis() as u64;
        let counts = result.count_by_severity();

        let summary = format!(
            "{} scan complete: {} critical, {} errors, {} warnings, {} info",
            self.name,
            counts.get(&Severity::Critical).unwrap_or(&0),
            counts.get(&Severity::Error).unwrap_or(&0),
            counts.get(&Severity::Warning).unwrap_or(&0),
            counts.get(&Severity::Info).unwrap_or(&0)
        );

        Ok(result.with_duration(duration_ms).with_summary(summary))
    }
}

// ============================================================================
// Factory Functions
// ============================================================================

/// Create a dynamic agent from a TOML configuration file.
pub async fn create_agent_from_toml(path: &Path) -> Result<DynamicAgent, AgentError> {
    DynamicAgent::load_from_file(path).await
}

/// Create a dynamic agent from TOML content string.
pub fn create_agent_from_toml_str(content: &str) -> Result<DynamicAgent, AgentError> {
    let rules_file: AgentRulesFile = toml::from_str(content)
        .map_err(|e| AgentError::Config(format!("Failed to parse TOML: {}", e)))?;
    Ok(DynamicAgent::from_rules_file(rules_file))
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Check if a file should be excluded based on glob patterns.
fn should_exclude_file(file_path: &Path, exclude_patterns: &[String]) -> bool {
    let path_str = file_path.to_string_lossy();
    for pattern in exclude_patterns {
        if glob_match(pattern, &path_str) {
            return true;
        }
    }
    false
}

/// Check if a file is in the allowed files list.
fn is_file_allowed(file_path: &Path, allowed_files: &[String]) -> bool {
    let path_str = file_path.to_string_lossy();
    for pattern in allowed_files {
        if glob_match(pattern, &path_str) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dynamic_agent_builder() {
        let agent = DynamicAgent::new("custom")
            .with_name("Custom Agent")
            .with_description("A custom validation agent")
            .depends_on("security");

        assert_eq!(agent.id(), "custom");
        assert_eq!(agent.name(), "Custom Agent");
        assert_eq!(agent.dependencies(), vec!["security".to_string()]);
    }

    #[test]
    fn test_parse_severity() {
        let agent = DynamicAgent::new("test");

        assert_eq!(agent.parse_severity(Some("critical")), Severity::Critical);
        assert_eq!(agent.parse_severity(Some("CRITICAL")), Severity::Critical);
        assert_eq!(agent.parse_severity(Some("error")), Severity::Error);
        assert_eq!(agent.parse_severity(Some("warning")), Severity::Warning);
        assert_eq!(agent.parse_severity(Some("info")), Severity::Info);
        assert_eq!(agent.parse_severity(None), Severity::Warning); // default
    }

    #[test]
    fn test_create_agent_from_toml_str() {
        let toml = r#"
[agent]
id = "custom"
name = "Custom Validator"
description = "Validates custom rules"

[rules.no_debug]
enabled = true
severity = "warning"
description = "Detect debug statements"
patterns = ["println!", "dbg!"]
"#;

        let agent = create_agent_from_toml_str(toml).expect("should parse TOML");
        assert_eq!(agent.id(), "custom");
        assert_eq!(agent.name(), "Custom Validator");
        assert_eq!(agent.rules.len(), 1);
        assert!(agent.rules.contains_key("no_debug"));
    }

    #[tokio::test]
    async fn test_dynamic_agent_validate() {
        let agent = DynamicAgent::new("test").with_name("Test Agent");

        // Use a temp directory that exists
        let temp_dir = std::env::temp_dir();
        let ctx = ValidationContext::new(temp_dir);
        let result = agent.validate(&ctx).await.expect("should validate");

        assert_eq!(result.agent_id, "test");
        assert!(result.is_passed());
    }
}
