//! Agent generation prompts for Cortex CLI.
//!
//! This module contains the prompt used to generate new agent configurations
//! using LLM-powered natural language understanding.

/// System prompt for generating agent configurations.
///
/// This prompt guides the LLM to create well-structured agent definitions
/// based on natural language descriptions. It includes:
///
/// - Information about Cortex agents and their configuration
/// - Available tools and their descriptions
/// - Agent modes (primary, subagent, all)
/// - Examples of well-designed agents
/// - Guidelines for creating effective agents
/// - Output format specification (JSON)
pub const AGENT_GENERATION_PROMPT: &str = r#"You are an expert at creating AI agent configurations for the Cortex CLI tool. Your task is to generate a complete agent definition based on the user's natural language description.

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

/// List of known tools for validation during agent generation.
pub const KNOWN_TOOLS: &[&str] = &[
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

/// Normalize a tool name to its canonical form.
///
/// Returns the canonical tool name if found, None otherwise.
pub fn normalize_tool_name(tool: &str) -> Option<&'static str> {
    KNOWN_TOOLS
        .iter()
        .find(|k| k.eq_ignore_ascii_case(tool))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generation_prompt_has_key_sections() {
        assert!(AGENT_GENERATION_PROMPT.contains("About Cortex Agents"));
        assert!(AGENT_GENERATION_PROMPT.contains("Available Tools"));
        assert!(AGENT_GENERATION_PROMPT.contains("Agent Modes"));
        assert!(AGENT_GENERATION_PROMPT.contains("Guidelines"));
        assert!(AGENT_GENERATION_PROMPT.contains("Output Format"));
    }

    #[test]
    fn test_generation_prompt_has_examples() {
        assert!(AGENT_GENERATION_PROMPT.contains("Code Reviewer"));
        assert!(AGENT_GENERATION_PROMPT.contains("Documentation Writer"));
        assert!(AGENT_GENERATION_PROMPT.contains("Test Writer"));
    }

    #[test]
    fn test_known_tools() {
        assert!(KNOWN_TOOLS.contains(&"Read"));
        assert!(KNOWN_TOOLS.contains(&"Execute"));
        assert!(KNOWN_TOOLS.contains(&"WebSearch"));
    }

    #[test]
    fn test_normalize_tool_name() {
        assert_eq!(normalize_tool_name("read"), Some("Read"));
        assert_eq!(normalize_tool_name("READ"), Some("Read"));
        assert_eq!(normalize_tool_name("grep"), Some("Grep"));
        assert_eq!(normalize_tool_name("unknown"), None);
    }
}
