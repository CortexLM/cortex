//! Agent definitions and types.

use crate::permission::PermissionConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agent operation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AgentMode {
    /// Primary agent (user-facing).
    #[default]
    Primary,
    /// Sub-agent (invoked by other agents).
    Subagent,
    /// Available as both primary and sub-agent.
    All,
}

/// Agent information and configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Agent identifier.
    pub name: String,
    /// Display name.
    pub display_name: Option<String>,
    /// Description of when to use this agent.
    pub description: Option<String>,
    /// Agent mode.
    pub mode: AgentMode,
    /// Whether this is a built-in agent.
    pub native: bool,
    /// Whether agent is hidden from UI.
    pub hidden: bool,
    /// Custom system prompt.
    pub prompt: Option<String>,
    /// Temperature for generation.
    pub temperature: Option<f32>,
    /// Top-P for generation.
    pub top_p: Option<f32>,
    /// Color for UI (hex).
    pub color: Option<String>,
    /// Permission configuration.
    pub permission: PermissionConfig,
    /// Model override (provider/model).
    pub model: Option<String>,
    /// Tools configuration (tool_name -> enabled).
    pub tools: HashMap<String, bool>,
    /// Additional options.
    pub options: HashMap<String, serde_json::Value>,
    /// Maximum agentic steps.
    pub max_steps: Option<usize>,
    /// Maximum output tokens for this agent.
    pub max_tokens: Option<u32>,
    /// Whether this agent should use a small/lightweight model.
    #[serde(default)]
    pub use_small_model: bool,
}

impl AgentInfo {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: None,
            description: None,
            mode: AgentMode::Primary,
            native: false,
            hidden: false,
            prompt: None,
            temperature: None,
            top_p: None,
            color: None,
            permission: PermissionConfig::default(),
            model: None,
            tools: HashMap::new(),
            options: HashMap::new(),
            max_steps: None,
            max_tokens: None,
            use_small_model: false,
        }
    }

    pub fn native(mut self) -> Self {
        self.native = true;
        self
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn with_mode(mut self, mode: AgentMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    pub fn with_permission(mut self, permission: PermissionConfig) -> Self {
        self.permission = permission;
        self
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    pub fn enable_tool(mut self, tool: impl Into<String>) -> Self {
        self.tools.insert(tool.into(), true);
        self
    }

    pub fn disable_tool(mut self, tool: impl Into<String>) -> Self {
        self.tools.insert(tool.into(), false);
        self
    }

    pub fn is_tool_enabled(&self, tool: &str) -> bool {
        self.tools.get(tool).copied().unwrap_or(true)
    }

    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = Some(max_steps);
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_small_model(mut self) -> Self {
        self.use_small_model = true;
        self
    }
}

/// A running agent instance.
pub struct Agent {
    pub info: AgentInfo,
    pub session_id: Option<String>,
}

// ============================================================================
// Built-in Agent Factory Functions
// ============================================================================

/// Create the "general" subagent for complex searches and multi-step tasks.
///
/// This agent is designed to be invoked via @general syntax or Task tool.
/// It has full access but cannot spawn further subagents (prevents recursion).
pub fn create_general_agent() -> AgentInfo {
    use crate::permission::PermissionConfig;
    use std::collections::HashMap;

    let mut tools = HashMap::new();
    // Enable todowrite for subagents - required for planning and progress tracking
    tools.insert("todowrite".to_string(), true);
    tools.insert("todoread".to_string(), true);
    // Disable task to prevent recursive subagent spawning
    tools.insert("task".to_string(), false);

    AgentInfo {
        name: "general".to_string(),
        display_name: Some("General".to_string()),
        description: Some(
            "General-purpose agent for complex searches and multi-step tasks. Can run in parallel."
                .to_string(),
        ),
        mode: AgentMode::Subagent,
        native: true,
        hidden: true, // Not shown in main list, invoked via @general
        prompt: Some(GENERAL_AGENT_PROMPT.to_string()),
        temperature: Some(0.7),
        top_p: None,
        color: Some("#8b5cf6".to_string()), // Violet
        permission: PermissionConfig::full_access(),
        model: None, // Uses default model
        tools,
        options: HashMap::new(),
        max_steps: Some(20), // Max 20 steps
        max_tokens: None,
        use_small_model: false,
    }
}

/// System prompt for the general agent.
pub const GENERAL_AGENT_PROMPT: &str = r#"You are a general-purpose agent specialized in complex research and multi-step tasks.

Your capabilities:
- Search and explore codebases thoroughly
- Execute shell commands to gather information
- Read and analyze multiple files
- Perform web searches when needed
- Synthesize information from multiple sources

Guidelines:
- Be thorough but efficient
- Focus on completing the task given to you
- Return clear, actionable results
- If you need more context, use available tools to gather it
- Do not modify files or make changes unless explicitly asked

## MANDATORY: Planning Phase (CRITICAL)

Before ANY action, you MUST create a detailed plan using the TodoWrite tool. This is non-negotiable.

### Planning Format
Use TodoWrite to create your plan with this EXACT format:
```
1. [pending] <TASK_DESCRIPTION>
2. [pending] <TASK_DESCRIPTION>
3. [pending] <TASK_DESCRIPTION>
...
```

### Progress Updates (MANDATORY)
As you work, you MUST update your todo list after EACH task:
- When starting a task: change `[pending]` to `[in_progress]`
- When completing a task: change `[in_progress]` to `[completed]`
- Example: `1. [completed] Analyze the codebase structure`

### Real-time Visibility Rules
1. ALWAYS call TodoWrite BEFORE your first action
2. ALWAYS update TodoWrite when a task status changes
3. Keep only ONE task as `[in_progress]` at a time
4. Mark tasks `[completed]` immediately when done

This allows the orchestrator to monitor your progress in real-time.

## MANDATORY: Final Summary

When you have completed ALL tasks, your final message MUST be a comprehensive summary with this structure:

```
## Summary for Orchestrator

### Tasks Completed
- [List each task you completed with brief outcome]

### Key Findings/Changes
- [Main discoveries or modifications made]

### Files Modified (if any)
- [List of files with type of change]

### Recommendations (if applicable)
- [Any follow-up actions or suggestions]

### Status: COMPLETED
```

This summary will be sent to the orchestrator to coordinate with other agents.
"#;

/// Create the "explore" subagent for fast codebase exploration.
pub fn create_explore_agent() -> AgentInfo {
    use crate::permission::PermissionConfig;
    use std::collections::HashMap;

    let mut tools = HashMap::new();
    tools.insert("edit".to_string(), false);
    tools.insert("write".to_string(), false);
    // Enable todowrite for subagents - required for planning and progress tracking
    tools.insert("todoread".to_string(), true);
    tools.insert("todowrite".to_string(), true);

    AgentInfo {
        name: "explore".to_string(),
        display_name: Some("Explore".to_string()),
        description: Some(
            "Fast agent specialized for exploring codebases. Use for finding files by patterns, searching code, or answering questions about the codebase."
                .to_string(),
        ),
        mode: AgentMode::Subagent,
        native: true,
        hidden: false,
        prompt: Some(EXPLORE_AGENT_PROMPT.to_string()),
        temperature: Some(0.3),
        top_p: None,
        color: Some("#f59e0b".to_string()), // Amber
        permission: PermissionConfig::read_only(),
        model: None,
        tools,
        options: HashMap::new(),
        max_steps: Some(15),
        max_tokens: None,
        use_small_model: false,
    }
}

/// System prompt for the explore agent.
pub const EXPLORE_AGENT_PROMPT: &str = r#"You are a fast, focused agent specialized in exploring codebases. Your goal is to quickly find relevant information.

When exploring:
1. Use glob patterns to find files by name/path
2. Use grep to search for specific code patterns
3. Use read to examine file contents
4. Be thorough but efficient - check multiple likely locations

Thoroughness levels:
- "quick": Basic search, check obvious locations
- "medium": Moderate exploration, check common patterns
- "very thorough": Comprehensive analysis across multiple locations

## MANDATORY: Planning Phase (CRITICAL)

Before ANY action, you MUST create a detailed plan using the TodoWrite tool. This is non-negotiable.

### Planning Format
Use TodoWrite to create your plan with this EXACT format:
```
1. [pending] <TASK_DESCRIPTION>
2. [pending] <TASK_DESCRIPTION>
3. [pending] <TASK_DESCRIPTION>
...
```

### Progress Updates (MANDATORY)
As you work, you MUST update your todo list after EACH task:
- When starting a task: change `[pending]` to `[in_progress]`
- When completing a task: change `[in_progress]` to `[completed]`
- Example: `1. [completed] Search for configuration files`

### Real-time Visibility Rules
1. ALWAYS call TodoWrite BEFORE your first action
2. ALWAYS update TodoWrite when a task status changes
3. Keep only ONE task as `[in_progress]` at a time
4. Mark tasks `[completed]` immediately when done

This allows the orchestrator to monitor your progress in real-time.

## MANDATORY: Final Summary

When you have completed ALL tasks, your final message MUST be a comprehensive summary with this structure:

```
## Summary for Orchestrator

### Tasks Completed
- [List each task you completed with brief outcome]

### Key Findings/Changes
- [Main discoveries or modifications made]

### Files Found/Analyzed
- [List relevant files discovered]

### Recommendations (if applicable)
- [Any follow-up actions or suggestions]

### Status: COMPLETED
```

This summary will be sent to the orchestrator to coordinate with other agents.
"#;

/// Create the "research" subagent for thorough investigation.
pub fn create_research_agent() -> AgentInfo {
    use crate::permission::PermissionConfig;
    use std::collections::HashMap;

    let mut tools = HashMap::new();
    tools.insert("edit".to_string(), false);
    tools.insert("write".to_string(), false);
    // Enable todowrite for subagents - required for planning and progress tracking
    tools.insert("todoread".to_string(), true);
    tools.insert("todowrite".to_string(), true);
    tools.insert("task".to_string(), false);

    AgentInfo {
        name: "research".to_string(),
        display_name: Some("Research".to_string()),
        description: Some(
            "Research agent for thorough investigation. Read-only, focuses on analysis and information gathering."
                .to_string(),
        ),
        mode: AgentMode::Subagent,
        native: true,
        hidden: true,
        prompt: Some(RESEARCH_AGENT_PROMPT.to_string()),
        temperature: Some(0.5),
        top_p: None,
        color: Some("#06b6d4".to_string()), // Cyan
        permission: PermissionConfig::read_only(),
        model: None,
        tools,
        options: HashMap::new(),
        max_steps: Some(15),
        max_tokens: None,
        use_small_model: false,
    }
}

/// System prompt for the research agent.
pub const RESEARCH_AGENT_PROMPT: &str = r#"You are a research agent focused on thorough investigation and analysis.

Capabilities:
- Deep code analysis and understanding
- Pattern recognition across codebases
- Documentation review and synthesis
- Dependency analysis

Guidelines:
1. Read extensively before drawing conclusions
2. Look for patterns and relationships
3. Document your findings clearly
4. Consider multiple perspectives
5. Do NOT modify any files - read-only investigation

## MANDATORY: Planning Phase (CRITICAL)

Before ANY action, you MUST create a detailed plan using the TodoWrite tool. This is non-negotiable.

### Planning Format
Use TodoWrite to create your plan with this EXACT format:
```
1. [pending] <TASK_DESCRIPTION>
2. [pending] <TASK_DESCRIPTION>
3. [pending] <TASK_DESCRIPTION>
...
```

### Progress Updates (MANDATORY)
As you work, you MUST update your todo list after EACH task:
- When starting a task: change `[pending]` to `[in_progress]`
- When completing a task: change `[in_progress]` to `[completed]`
- Example: `1. [completed] Analyze the authentication module`

### Real-time Visibility Rules
1. ALWAYS call TodoWrite BEFORE your first action
2. ALWAYS update TodoWrite when a task status changes
3. Keep only ONE task as `[in_progress]` at a time
4. Mark tasks `[completed]` immediately when done

This allows the orchestrator to monitor your progress in real-time.

## MANDATORY: Final Summary

When you have completed ALL tasks, your final message MUST be a comprehensive summary with this structure:

```
## Summary for Orchestrator

### Tasks Completed
- [List each task you completed with brief outcome]

### Key Findings
- [Main discoveries with evidence]

### Analysis Results
- Executive summary of findings
- Detailed patterns identified
- References to specific files and lines

### Recommendations (if applicable)
- [Any follow-up actions or suggestions]

### Status: COMPLETED
```

This summary will be sent to the orchestrator to coordinate with other agents.
"#;

impl Agent {
    pub fn new(info: AgentInfo) -> Self {
        Self {
            info,
            session_id: None,
        }
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn name(&self) -> &str {
        &self.info.name
    }

    pub fn display_name(&self) -> &str {
        self.info.display_name.as_deref().unwrap_or(&self.info.name)
    }

    pub fn can_edit(&self) -> bool {
        self.info.permission.edit.is_allowed()
    }

    pub fn can_execute(&self, command: &str) -> bool {
        self.info.permission.can_execute_bash(command)
    }
}
