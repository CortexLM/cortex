//! Subagent types and configuration.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Type of subagent to spawn.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentType {
    /// General-purpose coding agent with full tool access.
    Code,
    /// Research agent focused on reading and analysis (no file modifications).
    Research,
    /// Refactoring agent for code improvements.
    Refactor,
    /// Testing agent for writing and running tests.
    Test,
    /// Documentation agent for writing docs.
    Documentation,
    /// Security audit agent.
    Security,
    /// Architecture planning agent.
    Architect,
    /// Code review agent.
    Reviewer,
    /// Custom agent from registry (by name).
    /// The string is the agent name as defined in the AgentRegistry.
    Custom(String),
}

impl SubagentType {
    /// Get the display name for this subagent type.
    pub fn name(&self) -> &str {
        match self {
            Self::Code => "code",
            Self::Research => "research",
            Self::Refactor => "refactor",
            Self::Test => "test",
            Self::Documentation => "documentation",
            Self::Security => "security",
            Self::Architect => "architect",
            Self::Reviewer => "reviewer",
            Self::Custom(name) => name.as_str(),
        }
    }

    /// Parse from string.
    ///
    /// Built-in types are matched by name. Any unrecognized name is treated as
    /// a custom agent name from the AgentRegistry.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "code" | "coding" => Self::Code,
            "research" | "investigate" => Self::Research,
            "refactor" | "refactoring" => Self::Refactor,
            "test" | "testing" => Self::Test,
            "doc" | "docs" | "documentation" => Self::Documentation,
            "security" | "audit" => Self::Security,
            "architect" | "architecture" | "design" => Self::Architect,
            "review" | "reviewer" | "code-review" => Self::Reviewer,
            // Any other name is treated as a custom agent from the registry
            other => Self::Custom(other.to_string()),
        }
    }

    /// Check if this is a custom agent type.
    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Get the custom agent name if this is a custom type.
    pub fn custom_name(&self) -> Option<&str> {
        match self {
            Self::Custom(name) => Some(name),
            _ => None,
        }
    }

    /// Get the base system prompt for this subagent type (without task details).
    /// The task details should be sent as a user message, not in the system prompt.
    /// Tasks are conversational - passed as user messages rather than system configuration.
    pub fn base_system_prompt(&self) -> String {
        // Common planning instructions that all subagents must follow
        let planning_instructions = r#"

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

This summary will be sent to the orchestrator to coordinate with other agents."#;

        let base_prompt = match self {
            Self::Code => String::from(
                "You are a specialized coding subagent that implements functionality autonomously.\n\n\
                ## Your Capabilities\n\
                - Write clean, well-documented code\n\
                - Follow existing project conventions\n\
                - Test your changes when possible",
            ),
            Self::Research => String::from(
                "You are a research subagent specialized in investigating and gathering information.\n\n\
                ## Your Capabilities\n\
                - Read and understand code thoroughly\n\
                - Use grep, glob, and read tools to explore codebases\n\
                - Identify patterns and relationships\n\n\
                ## Important\n\
                - Do NOT modify any files",
            ),
            Self::Refactor => String::from(
                "You are a refactoring subagent that improves code quality.\n\n\
                ## Your Capabilities\n\
                - Improve code structure and readability\n\
                - Apply consistent naming conventions\n\
                - Remove code duplication\n\n\
                ## Important\n\
                - Preserve existing functionality\n\
                - Ensure tests still pass after changes",
            ),
            Self::Test => String::from(
                "You are a testing subagent that writes and runs tests.\n\n\
                ## Your Capabilities\n\
                - Write comprehensive unit tests\n\
                - Cover edge cases and error conditions\n\
                - Run tests to verify they pass\n\n\
                ## Important\n\
                - Follow existing test patterns in the project\n\
                - Report test coverage if possible",
            ),
            Self::Documentation => String::from(
                "You are a documentation subagent that writes and improves documentation.\n\n\
                ## Your Capabilities\n\
                - Write clear, concise documentation\n\
                - Include code examples where helpful\n\
                - Document public APIs and interfaces\n\n\
                ## Important\n\
                - Follow existing documentation style\n\
                - Keep documentation up to date with code",
            ),
            Self::Security => String::from(
                "You are a security audit subagent that identifies security issues.\n\n\
                ## Your Capabilities\n\
                - Look for common vulnerabilities (injection, XSS, etc.)\n\
                - Check for insecure configurations\n\
                - Review authentication and authorization\n\n\
                ## Important\n\
                - Identify sensitive data exposure\n\
                - Provide specific remediation steps",
            ),
            Self::Architect => String::from(
                "You are an architecture subagent that designs and plans software architecture.\n\n\
                ## Your Capabilities\n\
                - Analyze current architecture\n\
                - Consider scalability and maintainability\n\
                - Propose clear component boundaries\n\n\
                ## Important\n\
                - Document trade-offs and decisions\n\
                - Create implementation roadmaps",
            ),
            Self::Reviewer => String::from(
                "You are a code review subagent that reviews code for quality and correctness.\n\n\
                ## Your Capabilities\n\
                - Check for correctness and bugs\n\
                - Review code style and consistency\n\
                - Identify potential performance issues\n\n\
                ## Important\n\
                - Suggest improvements\n\
                - Prioritize feedback by severity",
            ),
            Self::Custom(_) => String::from(
                "You are a specialized subagent.\n\n\
                ## Important\n\
                - Complete the assigned task efficiently",
            ),
        };

        format!("{}{}", base_prompt, planning_instructions)
    }

    /// Get the system prompt for this subagent type (legacy, with task details embedded).
    /// Deprecated: Use base_system_prompt() and send task as user message instead.
    pub fn system_prompt(&self, task_description: &str, instructions: &str) -> String {
        format!(
            "{}\n\n## Current Task\n{}\n\n## Instructions\n{}",
            self.base_system_prompt(),
            task_description,
            instructions
        )
    }

    /// Get allowed tools for this subagent type.
    pub fn allowed_tools(&self) -> Option<Vec<&'static str>> {
        match self {
            Self::Research => Some(vec!["Read", "Grep", "Glob", "LS", "FetchUrl", "WebSearch"]),
            Self::Reviewer => Some(vec!["Read", "Grep", "Glob", "LS"]),
            Self::Security => Some(vec!["Read", "Grep", "Glob", "LS", "Execute"]),
            Self::Architect => Some(vec!["Read", "Grep", "Glob", "LS", "WebSearch"]),
            // Full tool access for these types
            Self::Code | Self::Refactor | Self::Test | Self::Documentation | Self::Custom(_) => {
                None
            }
        }
    }

    /// Get denied tools for this subagent type.
    pub fn denied_tools(&self) -> Vec<&'static str> {
        match self {
            Self::Research | Self::Reviewer | Self::Architect => {
                vec!["Create", "Edit", "ApplyPatch", "MultiEdit", "Execute"]
            }
            _ => vec![],
        }
    }

    /// Get max iterations for this subagent type.
    pub fn max_iterations(&self) -> u32 {
        match self {
            Self::Research | Self::Reviewer => 10,
            Self::Architect => 15,
            Self::Code | Self::Refactor | Self::Test => 20,
            Self::Documentation | Self::Security => 15,
            Self::Custom(_) => 20,
        }
    }
}

impl std::fmt::Display for SubagentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Configuration for spawning a subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentConfig {
    /// Type of subagent.
    pub agent_type: SubagentType,
    /// Task description (short).
    pub description: String,
    /// Detailed instructions/prompt.
    pub prompt: String,
    /// Model to use (overrides default).
    pub model: Option<String>,
    /// Temperature setting.
    pub temperature: Option<f32>,
    /// Maximum tool iterations.
    pub max_iterations: Option<u32>,
    /// Timeout for the entire task.
    pub timeout: Option<Duration>,
    /// Working directory.
    pub working_dir: PathBuf,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Parent session ID (for hierarchy tracking).
    pub parent_session_id: Option<String>,
    /// Session ID to continue (for resuming tasks).
    pub continue_session_id: Option<String>,
    /// Additional context to provide.
    pub context: Option<String>,
    /// Custom agent name (for Custom type).
    pub custom_agent_name: Option<String>,
    /// Optional session ID (if not provided, a new one will be generated).
    /// Use this to coordinate session_id between UI and executor.
    pub session_id: Option<String>,
}

impl SubagentConfig {
    /// Create a new subagent config.
    pub fn new(
        agent_type: SubagentType,
        description: impl Into<String>,
        prompt: impl Into<String>,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            agent_type,
            description: description.into(),
            prompt: prompt.into(),
            model: None,
            temperature: None,
            max_iterations: None,
            timeout: None,
            working_dir,
            env: HashMap::new(),
            parent_session_id: None,
            continue_session_id: None,
            context: None,
            custom_agent_name: None,
            session_id: None,
        }
    }

    /// Set the model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set temperature.
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set max iterations.
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Set timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set parent session ID.
    pub fn with_parent_session(mut self, session_id: impl Into<String>) -> Self {
        self.parent_session_id = Some(session_id.into());
        self
    }

    /// Set session to continue.
    pub fn with_continue_session(mut self, session_id: impl Into<String>) -> Self {
        self.continue_session_id = Some(session_id.into());
        self
    }

    /// Add context.
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Set a specific session ID (for coordination between UI and executor).
    /// If not set, a new session ID will be generated automatically.
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Get effective max iterations.
    pub fn effective_max_iterations(&self) -> u32 {
        self.max_iterations
            .unwrap_or_else(|| self.agent_type.max_iterations())
    }

    /// Get effective timeout.
    /// Returns None if no timeout is set (subagent runs until completion).
    pub fn effective_timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Build the full system prompt (legacy - includes task details).
    /// Deprecated: Use build_base_system_prompt() and build_user_message() instead.
    pub fn build_system_prompt(&self) -> String {
        let mut prompt = self
            .agent_type
            .system_prompt(&self.description, &self.prompt);

        if let Some(ref context) = self.context {
            prompt.push_str("\n\n## Additional Context\n");
            prompt.push_str(context);
        }

        prompt
    }

    /// Build the base system prompt (without task details).
    /// Task details should be sent as a user message.
    pub fn build_base_system_prompt(&self) -> String {
        self.agent_type.base_system_prompt()
    }

    /// Build the user message containing the task.
    /// Tasks are sent as user messages for conversational interaction.
    pub fn build_user_message(&self) -> String {
        let mut message = format!(
            "## Task\n{}\n\n## Instructions\n{}",
            self.description, self.prompt
        );

        if let Some(ref context) = self.context {
            message.push_str("\n\n## Additional Context\n");
            message.push_str(context);
        }

        message.push_str("\n\nPlease complete this task and provide a clear summary of your findings or actions when done.");

        message
    }
}

/// Status of a subagent execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    /// Subagent is being initialized.
    Initializing,
    /// Subagent is running.
    Running,
    /// Subagent is waiting for tool approval.
    WaitingForApproval,
    /// Subagent completed successfully.
    Completed,
    /// Subagent failed with error.
    Failed,
    /// Subagent was cancelled.
    Cancelled,
    /// Subagent timed out.
    TimedOut,
    /// Subagent is paused (can be resumed).
    Paused,
}

impl SubagentStatus {
    /// Check if the status is terminal.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }

    /// Check if the subagent can be resumed.
    pub fn can_resume(&self) -> bool {
        matches!(self, Self::Paused | Self::WaitingForApproval)
    }
}

impl std::fmt::Display for SubagentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initializing => write!(f, "initializing"),
            Self::Running => write!(f, "running"),
            Self::WaitingForApproval => write!(f, "waiting for approval"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::TimedOut => write!(f, "timed out"),
            Self::Paused => write!(f, "paused"),
        }
    }
}

/// Session information for a subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSession {
    /// Unique session ID.
    pub id: String,
    /// Parent session ID.
    pub parent_id: Option<String>,
    /// Subagent type.
    pub agent_type: SubagentType,
    /// Task description.
    pub description: String,
    /// Current status.
    pub status: SubagentStatus,
    /// Number of turns completed.
    pub turns_completed: u32,
    /// Number of tool calls made.
    pub tool_calls_made: u32,
    /// Total tokens used.
    pub tokens_used: u64,
    /// Created timestamp (ISO 8601).
    pub created_at: String,
    /// Last updated timestamp (ISO 8601).
    pub updated_at: String,
    /// Working directory.
    pub working_dir: PathBuf,
    /// Files modified during execution.
    pub files_modified: Vec<String>,
}

impl SubagentSession {
    /// Create a new session.
    pub fn new(
        id: impl Into<String>,
        parent_id: Option<String>,
        agent_type: SubagentType,
        description: impl Into<String>,
        working_dir: PathBuf,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: id.into(),
            parent_id,
            agent_type,
            description: description.into(),
            status: SubagentStatus::Initializing,
            turns_completed: 0,
            tool_calls_made: 0,
            tokens_used: 0,
            created_at: now.clone(),
            updated_at: now,
            working_dir,
            files_modified: Vec::new(),
        }
    }

    /// Update the session status.
    pub fn set_status(&mut self, status: SubagentStatus) {
        self.status = status;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Record a completed turn.
    pub fn record_turn(&mut self, tool_calls: u32, tokens: u64) {
        self.turns_completed += 1;
        self.tool_calls_made += tool_calls;
        self.tokens_used += tokens;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Record a modified file.
    pub fn record_file_modified(&mut self, path: impl Into<String>) {
        let path = path.into();
        if !self.files_modified.contains(&path) {
            self.files_modified.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subagent_type_parsing() {
        assert_eq!(SubagentType::from_str("code"), SubagentType::Code);
        assert_eq!(SubagentType::from_str("research"), SubagentType::Research);
        assert_eq!(SubagentType::from_str("REFACTOR"), SubagentType::Refactor);
        // Unknown names are now treated as custom agent names
        assert_eq!(
            SubagentType::from_str("my-custom-agent"),
            SubagentType::Custom("my-custom-agent".to_string())
        );
        assert!(SubagentType::from_str("my-custom-agent").is_custom());
        assert_eq!(
            SubagentType::from_str("my-custom-agent").custom_name(),
            Some("my-custom-agent")
        );
    }

    #[test]
    fn test_subagent_config() {
        let config = SubagentConfig::new(
            SubagentType::Research,
            "Analyze codebase",
            "Find all database queries",
            PathBuf::from("/project"),
        )
        .with_max_iterations(5)
        .with_timeout(Duration::from_secs(300));

        assert_eq!(config.effective_max_iterations(), 5);
        assert_eq!(config.effective_timeout(), Some(Duration::from_secs(300)));
    }

    #[test]
    fn test_subagent_session() {
        let mut session = SubagentSession::new(
            "session-1",
            None,
            SubagentType::Code,
            "Implement feature",
            PathBuf::from("/project"),
        );

        assert_eq!(session.status, SubagentStatus::Initializing);

        session.set_status(SubagentStatus::Running);
        assert_eq!(session.status, SubagentStatus::Running);

        session.record_turn(3, 1000);
        assert_eq!(session.turns_completed, 1);
        assert_eq!(session.tool_calls_made, 3);
    }
}
