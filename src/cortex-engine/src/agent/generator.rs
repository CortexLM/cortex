//! AI-powered agent generation.
//!
//! This module provides functionality to generate new agent configurations
//! using LLM-powered natural language understanding. Users can describe
//! what they want an agent to do, and the system will generate an appropriate
//! agent configuration with system prompt, tool permissions, and metadata.

use crate::client::{CompletionRequest, Message, create_client};
use crate::error::{CortexError, Result};
use serde::{Deserialize, Serialize};

/// Agent operation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Primary agent (user-facing).
    #[default]
    Primary,
    /// Sub-agent (invoked by other agents via Task tool).
    Subagent,
    /// Available as both primary and sub-agent.
    All,
}

impl std::fmt::Display for AgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentMode::Primary => write!(f, "primary"),
            AgentMode::Subagent => write!(f, "subagent"),
            AgentMode::All => write!(f, "all"),
        }
    }
}

/// A generated agent configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedAgent {
    /// Snake_case identifier for the agent.
    pub identifier: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Description of when to use this agent (shown in Task tool).
    pub when_to_use: String,
    /// The generated system prompt.
    pub system_prompt: String,
    /// Recommended tools for this agent.
    pub tools: Vec<String>,
    /// Agent operation mode.
    pub mode: AgentMode,
    /// Suggested tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Temperature setting recommendation.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Whether the agent can delegate to sub-agents.
    #[serde(default = "default_true")]
    pub can_delegate: bool,
}

fn default_true() -> bool {
    true
}

/// The prompt used to generate agent configurations.
pub const GENERATE_PROMPT: &str = r#"You are an expert at creating AI agent configurations for the Cortex CLI tool. Your task is to generate a complete agent definition based on the user's natural language description.

## About Cortex Agents

Cortex Agents are specialized AI assistants that can be configured with:
- Custom system prompts that define their behavior and expertise
- Tool permissions (which tools they can use)
- Operation mode (primary user-facing, subagent for delegation, or both)
- Temperature and other generation parameters

## Available Tools

Agents can use these tools:
- **Read**: Read file contents
- **Create**: Create new files
- **Edit**: Edit existing files (find and replace)
- **MultiEdit**: Make multiple edits to a file
- **LS**: List directory contents
- **Grep**: Search for patterns in files (regex supported)
- **Glob**: Find files by glob pattern
- **Execute**: Run shell commands
- **FetchUrl**: Fetch content from URLs
- **WebSearch**: Search the web
- **TodoWrite**: Manage task lists
- **TodoRead**: Read task lists
- **Task**: Delegate work to sub-agents
- **ApplyPatch**: Apply unified diff patches
- **CodeSearch**: Semantic code search
- **ViewImage**: View and analyze images
- **LspDiagnostics**: Get language server diagnostics
- **LspHover**: Get hover information
- **LspSymbols**: List symbols in a file

## Agent Modes

- **primary**: User-facing agent, appears in the main agent list
- **subagent**: Only invoked by other agents via the Task tool
- **all**: Can be used both as primary agent and as a subagent

## Examples of Well-Designed Agents

### Example 1: Code Reviewer
```json
{
  "identifier": "code_reviewer",
  "display_name": "Code Reviewer",
  "when_to_use": "Review code for quality, bugs, security issues, and best practices. Use for PR reviews, code audits, and quality assessments.",
  "system_prompt": "You are an expert code reviewer...",
  "tools": ["Read", "Grep", "Glob", "LS"],
  "mode": "subagent",
  "tags": ["review", "quality"],
  "temperature": 0.2,
  "can_delegate": false
}
```

### Example 2: Documentation Writer
```json
{
  "identifier": "doc_writer",
  "display_name": "Documentation Writer",
  "when_to_use": "Write and update documentation including READMEs, API docs, and user guides. Use when documentation needs to be created or improved.",
  "system_prompt": "You are a technical documentation specialist...",
  "tools": ["Read", "Create", "Edit", "Grep", "Glob", "LS"],
  "mode": "all",
  "tags": ["documentation", "writing"],
  "temperature": 0.5,
  "can_delegate": true
}
```

### Example 3: Test Writer
```json
{
  "identifier": "test_writer",
  "display_name": "Test Writer",
  "when_to_use": "Write unit tests, integration tests, and test fixtures. Use when test coverage needs to be improved.",
  "system_prompt": "You are an expert at writing comprehensive tests...",
  "tools": ["Read", "Create", "Edit", "Grep", "Glob", "LS", "Execute"],
  "mode": "subagent",
  "tags": ["testing", "quality"],
  "temperature": 0.3,
  "can_delegate": false
}
```

## Guidelines for Creating Agents

1. **Identifier**: Use snake_case, keep it short and descriptive (e.g., `rust_expert`, `api_designer`)

2. **Display Name**: Human-readable, title case (e.g., "Rust Expert", "API Designer")

3. **When to Use**: Write a clear 1-2 sentence description that helps other agents (or users) understand when to invoke this agent. Be specific about the use cases.

4. **System Prompt**: Write a comprehensive prompt that:
   - Defines the agent's role and expertise
   - Lists specific capabilities and knowledge areas
   - Provides guidelines and best practices
   - Specifies output format expectations
   - Includes relevant constraints

5. **Tools**: Only include tools the agent needs. Read-only agents should not have Edit/Create/Execute.

6. **Mode**: 
   - Use "primary" for agents users interact with directly
   - Use "subagent" for specialized workers invoked via Task
   - Use "all" for versatile agents

7. **Temperature**: 
   - Lower (0.1-0.3) for analytical, code review, precise tasks
   - Medium (0.4-0.6) for balanced creative/analytical work
   - Higher (0.7-0.9) for creative writing, brainstorming

8. **Tags**: Include 2-4 relevant tags for discoverability

## Output Format

You MUST respond with a valid JSON object matching this schema:

```json
{
  "identifier": "string (snake_case)",
  "display_name": "string",
  "when_to_use": "string (1-2 sentences)",
  "system_prompt": "string (comprehensive prompt)",
  "tools": ["array", "of", "tool", "names"],
  "mode": "primary|subagent|all",
  "tags": ["array", "of", "tags"],
  "temperature": 0.0-1.0,
  "can_delegate": true|false
}
```

Generate a complete agent configuration based on the user's description. Be creative with the system prompt but precise with the metadata."#;

/// Agent generator that uses LLM to create agent configurations.
pub struct AgentGenerator {
    /// Model to use for generation.
    model: String,
    /// Optional backend URL.
    backend_url: Option<String>,
}

impl AgentGenerator {
    /// Create a new agent generator.
    pub fn new() -> Self {
        Self {
            model: "gpt-4o".to_string(),
            backend_url: None,
        }
    }

    /// Set the model to use.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the backend URL.
    pub fn with_backend_url(mut self, url: impl Into<String>) -> Self {
        self.backend_url = Some(url.into());
        self
    }

    /// Generate an agent configuration from a natural language description.
    pub async fn generate(&self, description: &str) -> Result<GeneratedAgent> {
        let client = create_client("cortex", &self.model, "", self.backend_url.as_deref())?;

        let request = CompletionRequest {
            messages: vec![
                Message::system(GENERATE_PROMPT),
                Message::user(format!(
                    "Create an agent for the following purpose:\n\n{}",
                    description
                )),
            ],
            model: self.model.clone(),
            max_tokens: Some(4096),
            temperature: Some(0.7),
            seed: None,
            tools: vec![],
            stream: false,
        };

        let response = client.complete_sync(request).await?;

        // Extract the response content
        let content = response
            .message
            .as_ref()
            .and_then(|m| m.content.as_text())
            .ok_or_else(|| CortexError::Internal("No response from model".into()))?;

        // Parse the JSON response
        let generated = parse_generated_agent(content)?;

        Ok(generated)
    }
}

impl Default for AgentGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse the generated agent from LLM response.
fn parse_generated_agent(content: &str) -> Result<GeneratedAgent> {
    // Try to parse directly
    if let Ok(agent) = serde_json::from_str::<GeneratedAgent>(content) {
        return validate_agent(agent);
    }

    // Try to extract JSON from code block
    if let Some(json_start) = content.find("```json") {
        let json_content = &content[json_start + 7..];
        if let Some(json_end) = json_content.find("```") {
            let json_str = json_content[..json_end].trim();
            if let Ok(agent) = serde_json::from_str::<GeneratedAgent>(json_str) {
                return validate_agent(agent);
            }
        }
    }

    // Try to find JSON object in content
    if let Some(start) = content.find('{') {
        let sub = &content[start..];
        let mut brace_count = 0;
        let mut end_idx = 0;

        for (i, c) in sub.chars().enumerate() {
            if c == '{' {
                brace_count += 1;
            } else if c == '}' {
                brace_count -= 1;
                if brace_count == 0 {
                    end_idx = i + 1;
                    break;
                }
            }
        }

        if end_idx > 0 {
            let json_str = &sub[..end_idx];
            if let Ok(agent) = serde_json::from_str::<GeneratedAgent>(json_str) {
                return validate_agent(agent);
            }
        }
    }

    Err(CortexError::Internal(format!(
        "Failed to parse agent configuration from response:\n{}",
        content
    )))
}

/// Validate and normalize the generated agent.
fn validate_agent(mut agent: GeneratedAgent) -> Result<GeneratedAgent> {
    // Normalize identifier to snake_case
    agent.identifier = agent
        .identifier
        .to_lowercase()
        .replace([' ', '-'], "_")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();

    if agent.identifier.is_empty() {
        return Err(CortexError::InvalidInput(
            "Agent identifier cannot be empty".into(),
        ));
    }

    if agent.identifier.len() > 64 {
        agent.identifier = agent.identifier[..64].to_string();
    }

    // Validate display name
    if agent.display_name.is_empty() {
        agent.display_name = agent
            .identifier
            .split('_')
            .map(|s| {
                let mut c = s.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().chain(c).collect(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }

    // Validate when_to_use
    if agent.when_to_use.is_empty() {
        agent.when_to_use = format!(
            "A specialized agent for {}.",
            agent.display_name.to_lowercase()
        );
    }

    // Validate system prompt
    if agent.system_prompt.is_empty() {
        return Err(CortexError::InvalidInput(
            "System prompt cannot be empty".into(),
        ));
    }

    // Validate temperature
    if let Some(temp) = agent.temperature {
        if !(0.0..=2.0).contains(&temp) {
            agent.temperature = Some(temp.clamp(0.0, 2.0));
        }
    }

    // Validate tools - filter to known tools
    let known_tools = [
        "Read",
        "Create",
        "Edit",
        "MultiEdit",
        "LS",
        "Grep",
        "Glob",
        "Execute",
        "FetchUrl",
        "WebSearch",
        "TodoWrite",
        "TodoRead",
        "Task",
        "ApplyPatch",
        "CodeSearch",
        "ViewImage",
        "LspDiagnostics",
        "LspHover",
        "LspSymbols",
    ];

    agent
        .tools
        .retain(|t| known_tools.iter().any(|k| k.eq_ignore_ascii_case(t)));

    // Normalize tool names to proper casing
    agent.tools = agent
        .tools
        .iter()
        .filter_map(|t| {
            known_tools
                .iter()
                .find(|k| k.eq_ignore_ascii_case(t))
                .map(|k| k.to_string())
        })
        .collect();

    Ok(agent)
}

/// Convert GeneratedAgent to markdown format with YAML frontmatter.
impl GeneratedAgent {
    /// Convert to markdown format suitable for saving as an agent file.
    pub fn to_markdown(&self) -> String {
        let mut frontmatter = format!(
            r#"---
name: {}
description: "{}"
mode: {}
"#,
            self.identifier,
            self.when_to_use.replace('"', "\\\""),
            self.mode
        );

        if let Some(temp) = self.temperature {
            frontmatter.push_str(&format!("temperature: {}\n", temp));
        }

        if !self.display_name.is_empty() {
            frontmatter.push_str(&format!("display_name: \"{}\"\n", self.display_name));
        }

        frontmatter.push_str(&format!("can_delegate: {}\n", self.can_delegate));

        if !self.tools.is_empty() {
            frontmatter.push_str("allowed_tools:\n");
            for tool in &self.tools {
                frontmatter.push_str(&format!("  - {}\n", tool));
            }
        }

        if !self.tags.is_empty() {
            frontmatter.push_str("tags:\n");
            for tag in &self.tags {
                frontmatter.push_str(&format!("  - {}\n", tag));
            }
        }

        frontmatter.push_str("---\n\n");

        format!("{}{}\n", frontmatter, self.system_prompt)
    }

    /// Get the suggested filename.
    pub fn filename(&self) -> String {
        format!("{}.md", self.identifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_generated_agent() {
        let json = r#"{
            "identifier": "rust_expert",
            "display_name": "Rust Expert",
            "when_to_use": "Help with Rust programming tasks",
            "system_prompt": "You are a Rust expert.",
            "tools": ["Read", "Grep", "Edit"],
            "mode": "subagent",
            "tags": ["rust", "programming"],
            "temperature": 0.3,
            "can_delegate": false
        }"#;

        let agent = parse_generated_agent(json).unwrap();
        assert_eq!(agent.identifier, "rust_expert");
        assert_eq!(agent.display_name, "Rust Expert");
        assert_eq!(agent.tools.len(), 3);
    }

    #[test]
    fn test_parse_json_in_code_block() {
        let content = r#"Here's the agent configuration:

```json
{
    "identifier": "test_agent",
    "display_name": "Test Agent",
    "when_to_use": "Testing purposes",
    "system_prompt": "You are a test agent.",
    "tools": ["Read"],
    "mode": "primary",
    "tags": [],
    "temperature": 0.5,
    "can_delegate": true
}
```

This agent will help with testing."#;

        let agent = parse_generated_agent(content).unwrap();
        assert_eq!(agent.identifier, "test_agent");
    }

    #[test]
    fn test_validate_agent_normalizes_identifier() {
        let agent = GeneratedAgent {
            identifier: "My Test-Agent".to_string(),
            display_name: "My Test Agent".to_string(),
            when_to_use: "Testing".to_string(),
            system_prompt: "Test prompt".to_string(),
            tools: vec!["read".to_string(), "GREP".to_string()],
            mode: AgentMode::Primary,
            tags: vec![],
            temperature: Some(0.5),
            can_delegate: true,
        };

        let validated = validate_agent(agent).unwrap();
        assert_eq!(validated.identifier, "my_test_agent");
        assert!(validated.tools.contains(&"Read".to_string()));
        assert!(validated.tools.contains(&"Grep".to_string()));
    }

    #[test]
    fn test_to_markdown() {
        let agent = GeneratedAgent {
            identifier: "code_reviewer".to_string(),
            display_name: "Code Reviewer".to_string(),
            when_to_use: "Review code for quality".to_string(),
            system_prompt: "You are an expert code reviewer.".to_string(),
            tools: vec!["Read".to_string(), "Grep".to_string()],
            mode: AgentMode::Subagent,
            tags: vec!["review".to_string()],
            temperature: Some(0.2),
            can_delegate: false,
        };

        let md = agent.to_markdown();
        assert!(md.starts_with("---"));
        assert!(md.contains("name: code_reviewer"));
        assert!(md.contains("mode: subagent"));
        assert!(md.contains("temperature: 0.2"));
        assert!(md.contains("You are an expert code reviewer."));
    }

    #[test]
    fn test_agent_mode_display() {
        assert_eq!(AgentMode::Primary.to_string(), "primary");
        assert_eq!(AgentMode::Subagent.to_string(), "subagent");
        assert_eq!(AgentMode::All.to_string(), "all");
    }
}
