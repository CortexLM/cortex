//! System prompt management and templating.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// System prompt configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemPrompt {
    /// Base prompt text.
    pub base: Option<String>,
    /// Sections to include.
    pub sections: Vec<PromptSection>,
    /// Variables for templating.
    pub variables: HashMap<String, String>,
    /// Enable code execution context.
    pub code_execution: bool,
    /// Enable file operation context.
    pub file_operations: bool,
    /// Enable web search context.
    pub web_search: bool,
    /// Custom instructions.
    pub custom_instructions: Option<String>,
    /// Persona/role.
    pub persona: Option<String>,
    /// Token count estimate.
    token_count: u32,
}

impl SystemPrompt {
    /// Create a new system prompt.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with base text.
    pub fn with_base(base: impl Into<String>) -> Self {
        let base = base.into();
        let token_count = estimate_tokens(&base);
        Self {
            base: Some(base),
            token_count,
            ..Self::default()
        }
    }

    /// Set base prompt.
    pub fn set_base(&mut self, base: impl Into<String>) {
        let base = base.into();
        self.token_count = estimate_tokens(&base);
        self.base = Some(base);
        self.recalculate_tokens();
    }

    /// Add a section.
    pub fn add_section(&mut self, section: PromptSection) {
        self.sections.push(section);
        self.recalculate_tokens();
    }

    /// Remove a section by name.
    pub fn remove_section(&mut self, name: &str) {
        self.sections.retain(|s| s.name != name);
        self.recalculate_tokens();
    }

    /// Set a variable.
    pub fn set_variable(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.variables.insert(key.into(), value.into());
        self.recalculate_tokens();
    }

    /// Set persona.
    pub fn set_persona(&mut self, persona: impl Into<String>) {
        self.persona = Some(persona.into());
        self.recalculate_tokens();
    }

    /// Set custom instructions.
    pub fn set_custom_instructions(&mut self, instructions: impl Into<String>) {
        self.custom_instructions = Some(instructions.into());
        self.recalculate_tokens();
    }

    /// Enable code execution context.
    pub fn enable_code_execution(&mut self) {
        self.code_execution = true;
        self.recalculate_tokens();
    }

    /// Enable file operations context.
    pub fn enable_file_operations(&mut self) {
        self.file_operations = true;
        self.recalculate_tokens();
    }

    /// Enable web search context.
    pub fn enable_web_search(&mut self) {
        self.web_search = true;
        self.recalculate_tokens();
    }

    /// Get token count.
    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    /// Render the full system prompt.
    pub fn render(&self) -> Option<String> {
        let mut parts = Vec::new();

        // Persona
        if let Some(persona) = &self.persona {
            parts.push(persona.clone());
        }

        // Base prompt
        if let Some(base) = &self.base {
            let rendered = self.render_template(base);
            parts.push(rendered);
        }

        // Sections
        for section in &self.sections {
            if section.enabled {
                let content = self.render_template(&section.content);
                if !section.name.is_empty() {
                    parts.push(format!("## {}\n{}", section.name, content));
                } else {
                    parts.push(content);
                }
            }
        }

        // Capability contexts
        if self.code_execution {
            parts.push(CODE_EXECUTION_CONTEXT.to_string());
        }
        if self.file_operations {
            parts.push(FILE_OPERATIONS_CONTEXT.to_string());
        }
        if self.web_search {
            parts.push(WEB_SEARCH_CONTEXT.to_string());
        }

        // Custom instructions
        if let Some(instructions) = &self.custom_instructions {
            parts.push(format!("## Custom Instructions\n{instructions}"));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    /// Render template with variables.
    fn render_template(&self, template: &str) -> String {
        let mut result = template.to_string();
        for (key, value) in &self.variables {
            result = result.replace(&format!("{{{{{key}}}}}"), value);
            result = result.replace(&format!("${{{key}}}"), value);
        }
        result
    }

    /// Recalculate token count.
    fn recalculate_tokens(&mut self) {
        if let Some(rendered) = self.render() {
            self.token_count = estimate_tokens(&rendered);
        } else {
            self.token_count = 0;
        }
    }
}

/// A section of the system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSection {
    /// Section name.
    pub name: String,
    /// Section content.
    pub content: String,
    /// Whether this section is enabled.
    pub enabled: bool,
    /// Priority (higher = earlier in prompt).
    pub priority: u32,
}

impl PromptSection {
    /// Create a new section.
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
            enabled: true,
            priority: 0,
        }
    }

    /// Create with priority.
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// Set enabled state.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Builder for system prompts.
#[derive(Debug, Default)]
pub struct SystemPromptBuilder {
    prompt: SystemPrompt,
}

impl SystemPromptBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set base prompt.
    pub fn base(mut self, base: impl Into<String>) -> Self {
        self.prompt.base = Some(base.into());
        self
    }

    /// Set persona.
    pub fn persona(mut self, persona: impl Into<String>) -> Self {
        self.prompt.persona = Some(persona.into());
        self
    }

    /// Add a section.
    pub fn section(mut self, name: impl Into<String>, content: impl Into<String>) -> Self {
        self.prompt.sections.push(PromptSection::new(name, content));
        self
    }

    /// Add a variable.
    pub fn variable(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.prompt.variables.insert(key.into(), value.into());
        self
    }

    /// Set custom instructions.
    pub fn custom_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.prompt.custom_instructions = Some(instructions.into());
        self
    }

    /// Enable code execution.
    pub fn code_execution(mut self) -> Self {
        self.prompt.code_execution = true;
        self
    }

    /// Enable file operations.
    pub fn file_operations(mut self) -> Self {
        self.prompt.file_operations = true;
        self
    }

    /// Enable web search.
    pub fn web_search(mut self) -> Self {
        self.prompt.web_search = true;
        self
    }

    /// Build the system prompt.
    pub fn build(mut self) -> SystemPrompt {
        self.prompt.recalculate_tokens();
        self.prompt
    }
}

/// Predefined system prompts.
pub mod presets {
    use super::*;

    /// Default coding assistant prompt.
    pub fn coding_assistant() -> SystemPrompt {
        SystemPromptBuilder::new()
            .persona("You are Cortex, an expert AI coding assistant.")
            .base(CODING_ASSISTANT_BASE)
            .code_execution()
            .file_operations()
            .build()
    }

    /// Research assistant prompt.
    pub fn research_assistant() -> SystemPrompt {
        SystemPromptBuilder::new()
            .persona("You are a helpful research assistant with access to web search.")
            .base("Help the user find and analyze information. Cite sources when possible.")
            .web_search()
            .build()
    }

    /// Code review prompt.
    pub fn code_reviewer() -> SystemPrompt {
        SystemPromptBuilder::new()
            .persona("You are an expert code reviewer.")
            .base(CODE_REVIEWER_BASE)
            .file_operations()
            .build()
    }

    /// Minimal assistant prompt.
    pub fn minimal() -> SystemPrompt {
        SystemPromptBuilder::new()
            .base("You are a helpful assistant. Be concise.")
            .build()
    }
}

// Context strings
const CODE_EXECUTION_CONTEXT: &str = r#"## Code Execution
You have access to execute shell commands and code. Use this capability responsibly:
- Always explain what commands will do before executing
- Prefer non-destructive operations
- Ask for confirmation before making significant changes
- Handle errors gracefully"#;

const FILE_OPERATIONS_CONTEXT: &str = r#"## File Operations
You can read, write, and modify files. Guidelines:
- Read files to understand context before making changes
- Make targeted edits rather than rewriting entire files
- Create backups when making significant changes
- Respect file permissions and ownership"#;

const WEB_SEARCH_CONTEXT: &str = r#"## Web Search
You can search the web for information. Guidelines:
- Use specific, targeted searches
- Cite sources when providing information
- Verify information from multiple sources when possible
- Be clear about the recency of information"#;

const CODING_ASSISTANT_BASE: &str = r#"You are an expert software engineer who helps users with coding tasks.

## Capabilities
- Write, review, and debug code
- Execute shell commands to test and verify changes
- Read and modify files in the project
- Search for patterns and understand codebases

## Guidelines
- Write clean, maintainable code
- Follow project conventions and style
- Explain your reasoning and approach
- Test changes when possible
- Be concise but thorough"#;

const CODE_REVIEWER_BASE: &str = r#"Review code for:
- Correctness and bugs
- Performance issues
- Security vulnerabilities
- Code style and maintainability
- Test coverage

Provide specific, actionable feedback with examples."#;

/// Estimate token count.
fn estimate_tokens(text: &str) -> u32 {
    (text.len() as u32 / 4) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_prompt_builder() {
        let prompt = SystemPromptBuilder::new()
            .persona("You are helpful")
            .base("Help the user.")
            .variable("name", "Alice")
            .code_execution()
            .build();

        let rendered = prompt.render().unwrap();
        assert!(rendered.contains("You are helpful"));
        assert!(rendered.contains("Help the user"));
        assert!(rendered.contains("Code Execution"));
    }

    #[test]
    fn test_variable_substitution() {
        let prompt = SystemPromptBuilder::new()
            .base("Hello {{name}}, you are in ${project}!")
            .variable("name", "Alice")
            .variable("project", "TestProject")
            .build();

        let rendered = prompt.render().unwrap();
        assert!(rendered.contains("Hello Alice"));
        assert!(rendered.contains("TestProject"));
    }

    #[test]
    fn test_sections() {
        let mut prompt = SystemPrompt::new();
        prompt.add_section(PromptSection::new("Rules", "Follow these rules"));
        prompt.add_section(PromptSection::new("Context", "Current context"));

        let rendered = prompt.render().unwrap();
        assert!(rendered.contains("## Rules"));
        assert!(rendered.contains("## Context"));
    }

    #[test]
    fn test_presets() {
        let coding = presets::coding_assistant();
        let rendered = coding.render().unwrap();
        assert!(rendered.contains("Cortex"));
        assert!(rendered.contains("Code Execution"));
    }
}
