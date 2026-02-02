//! System prompt builder for constructing dynamic prompts.
//!
//! This module provides a fluent API for building system prompts
//! with sections, variables, and templates.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::sections::{PromptSection, SectionPriority};

/// Builder for constructing system prompts.
///
/// The builder assembles prompts from:
/// - A base template with variable substitution
/// - Ordered sections with priorities
/// - Dynamic content based on context
#[derive(Debug, Clone, Default)]
pub struct SystemPromptBuilder {
    /// Base template text.
    base: Option<String>,
    /// Sections to include (ordered by insertion, sorted by priority on build).
    sections: IndexMap<String, PromptSection>,
    /// Variables for template substitution.
    variables: HashMap<String, String>,
    /// Prefix to add before everything.
    prefix: Option<String>,
    /// Suffix to add after everything.
    suffix: Option<String>,
    /// Section separator.
    section_separator: String,
}

impl SystemPromptBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            section_separator: "\n\n".to_string(),
            ..Default::default()
        }
    }

    /// Create a builder with a base template.
    pub fn with_base(base: impl Into<String>) -> Self {
        Self {
            base: Some(base.into()),
            section_separator: "\n\n".to_string(),
            ..Default::default()
        }
    }

    /// Set the base template.
    pub fn base(mut self, base: impl Into<String>) -> Self {
        self.base = Some(base.into());
        self
    }

    /// Add a section.
    pub fn section(mut self, section: PromptSection) -> Self {
        self.sections.insert(section.name.clone(), section);
        self
    }

    /// Add a section with name and content.
    pub fn add_section(mut self, name: impl Into<String>, content: impl Into<String>) -> Self {
        let section = PromptSection::new(name, content);
        self.sections.insert(section.name.clone(), section);
        self
    }

    /// Add a section with priority.
    pub fn add_section_with_priority(
        mut self,
        name: impl Into<String>,
        content: impl Into<String>,
        priority: SectionPriority,
    ) -> Self {
        let section = PromptSection::new(name, content).with_priority(priority);
        self.sections.insert(section.name.clone(), section);
        self
    }

    /// Add a high-priority section (appears early).
    pub fn add_high_priority_section(
        self,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        self.add_section_with_priority(name, content, SectionPriority::High)
    }

    /// Add a low-priority section (appears late).
    pub fn add_low_priority_section(
        self,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        self.add_section_with_priority(name, content, SectionPriority::Low)
    }

    /// Set a variable for template substitution.
    ///
    /// Variables can be referenced in templates using `{{variable_name}}` syntax.
    pub fn variable(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(key.into(), value.into());
        self
    }

    /// Set multiple variables at once.
    pub fn variables(mut self, vars: HashMap<String, String>) -> Self {
        self.variables.extend(vars);
        self
    }

    /// Set a prefix that appears before everything.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Set a suffix that appears after everything.
    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    /// Set the section separator (default: "\n\n").
    pub fn section_separator(mut self, sep: impl Into<String>) -> Self {
        self.section_separator = sep.into();
        self
    }

    /// Remove a section by name.
    pub fn remove_section(mut self, name: &str) -> Self {
        self.sections.shift_remove(name);
        self
    }

    /// Check if a section exists.
    pub fn has_section(&self, name: &str) -> bool {
        self.sections.contains_key(name)
    }

    /// Get a section by name.
    pub fn get_section(&self, name: &str) -> Option<&PromptSection> {
        self.sections.get(name)
    }

    /// Build the final prompt string.
    pub fn build(&self) -> String {
        let mut parts = Vec::new();

        // Add prefix if present
        if let Some(ref prefix) = self.prefix {
            parts.push(self.substitute_variables(prefix));
        }

        // Add base template if present
        if let Some(ref base) = self.base {
            parts.push(self.substitute_variables(base));
        }

        // Sort sections by priority and add them
        let mut sections: Vec<_> = self.sections.values().collect();
        sections.sort_by_key(|s| std::cmp::Reverse(s.priority));

        for section in sections {
            if section.enabled {
                let content = self.substitute_variables(&section.render());
                parts.push(content);
            }
        }

        // Add suffix if present
        if let Some(ref suffix) = self.suffix {
            parts.push(self.substitute_variables(suffix));
        }

        parts.join(&self.section_separator)
    }

    /// Build and return estimated token count.
    pub fn build_with_token_estimate(&self) -> (String, u32) {
        let prompt = self.build();
        let tokens = estimate_tokens(&prompt);
        (prompt, tokens)
    }

    /// Substitute variables in text.
    ///
    /// Supports both `{{var}}` and `${var}` syntax.
    fn substitute_variables(&self, text: &str) -> String {
        let mut result = text.to_string();

        for (key, value) in &self.variables {
            // Handle {{var}} syntax
            let pattern1 = format!("{{{{{}}}}}", key);
            result = result.replace(&pattern1, value);

            // Handle ${var} syntax
            let pattern2 = format!("${{{}}}", key);
            result = result.replace(&pattern2, value);
        }

        result
    }
}

/// Estimate token count for a string.
///
/// Uses a simple approximation of ~4 characters per token.
fn estimate_tokens(text: &str) -> u32 {
    (text.len() as f64 / 4.0).ceil() as u32
}

/// Convenience function to build a simple prompt.
pub fn build_simple_prompt(base: &str, sections: &[(&str, &str)]) -> String {
    let mut builder = SystemPromptBuilder::with_base(base);

    for (name, content) in sections {
        builder = builder.add_section(*name, *content);
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_builder() {
        let prompt = SystemPromptBuilder::new()
            .base("You are a helpful assistant.")
            .build();

        assert_eq!(prompt, "You are a helpful assistant.");
    }

    #[test]
    fn test_with_sections() {
        let prompt = SystemPromptBuilder::with_base("Base prompt")
            .add_section("Rules", "Follow these rules")
            .add_section("Context", "Current context")
            .build();

        assert!(prompt.contains("Base prompt"));
        assert!(prompt.contains("## Rules"));
        assert!(prompt.contains("## Context"));
    }

    #[test]
    fn test_variable_substitution() {
        let prompt = SystemPromptBuilder::with_base("Hello {{name}}!")
            .variable("name", "World")
            .build();

        assert_eq!(prompt, "Hello World!");
    }

    #[test]
    fn test_dollar_brace_syntax() {
        let prompt = SystemPromptBuilder::with_base("Working in ${cwd}")
            .variable("cwd", "/project")
            .build();

        assert_eq!(prompt, "Working in /project");
    }

    #[test]
    fn test_section_priority() {
        let prompt = SystemPromptBuilder::new()
            .add_section_with_priority("Low", "Low priority", SectionPriority::Low)
            .add_section_with_priority("High", "High priority", SectionPriority::High)
            .add_section_with_priority("Normal", "Normal priority", SectionPriority::Normal)
            .build();

        // High should appear before Normal, Normal before Low
        let high_pos = prompt.find("High priority").unwrap();
        let normal_pos = prompt.find("Normal priority").unwrap();
        let low_pos = prompt.find("Low priority").unwrap();

        assert!(high_pos < normal_pos);
        assert!(normal_pos < low_pos);
    }

    #[test]
    fn test_prefix_suffix() {
        let prompt = SystemPromptBuilder::with_base("Main content")
            .prefix("PREFIX")
            .suffix("SUFFIX")
            .build();

        assert!(prompt.starts_with("PREFIX"));
        assert!(prompt.ends_with("SUFFIX"));
    }

    #[test]
    fn test_remove_section() {
        let prompt = SystemPromptBuilder::new()
            .add_section("Keep", "Keep this")
            .add_section("Remove", "Remove this")
            .remove_section("Remove")
            .build();

        assert!(prompt.contains("Keep this"));
        assert!(!prompt.contains("Remove this"));
    }

    #[test]
    fn test_token_estimate() {
        let (prompt, tokens) =
            SystemPromptBuilder::with_base("This is a test prompt.").build_with_token_estimate();

        assert!(tokens > 0);
        assert!(!prompt.is_empty());
    }

    #[test]
    fn test_build_simple_prompt() {
        let prompt =
            build_simple_prompt("Base", &[("Rules", "Rule 1"), ("Context", "Context info")]);

        assert!(prompt.contains("Base"));
        assert!(prompt.contains("Rule 1"));
        assert!(prompt.contains("Context info"));
    }
}
