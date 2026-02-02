//! Agent registry for managing available agents.

use crate::{AgentInfo, AgentMode, PermissionConfig};
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Marker for agents that should use small/lightweight models.
pub const SMALL_MODEL_AGENTS: &[&str] = &["title", "summary"];

/// Registry of available agents.
pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentInfo>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// Create registry with built-in agents.
    pub async fn with_defaults() -> Self {
        let registry = Self::new();
        registry.register_builtin_agents().await;
        registry
    }

    /// Register built-in agents.
    async fn register_builtin_agents(&self) {
        // Build agent - full access for development
        self.register(
            AgentInfo::new("build")
                .native()
                .with_description("Full access agent for development work")
                .with_mode(AgentMode::Primary)
                .with_permission(PermissionConfig::full_access())
                .with_color("#22c55e"),
        )
        .await;

        // Plan agent - read-only for analysis
        self.register(AgentInfo::new("plan")
            .native()
            .with_description("Read-only agent for analysis and code exploration. Denies file edits by default and asks permission for commands.")
            .with_mode(AgentMode::Primary)
            .with_permission(PermissionConfig::read_only())
            .with_color("#3b82f6")).await;

        // Explore agent - fast codebase exploration
        self.register(AgentInfo::new("explore")
            .native()
            .with_description("Fast agent specialized for exploring codebases. Use for finding files by patterns, searching code, or answering questions about the codebase.")
            .with_mode(AgentMode::Subagent)
            .with_permission(PermissionConfig::read_only())
            .with_prompt(EXPLORE_PROMPT)
            .disable_tool("edit")
            .disable_tool("write")
            .disable_tool("todoread")
            .disable_tool("todowrite")
            .with_max_steps(15)
            .with_temperature(0.3)
            .with_color("#f59e0b")).await;

        // General agent - parallel task execution
        self.register(AgentInfo::new("general")
            .native()
            .with_display_name("General")
            .with_description("General-purpose agent for complex searches, research, and multi-step tasks. Can run in parallel.")
            .with_mode(AgentMode::Subagent)
            .with_permission(PermissionConfig::full_access())
            .with_prompt(GENERAL_PROMPT)
            .disable_tool("todoread")
            .disable_tool("todowrite")
            .disable_tool("task") // Prevent recursive subagent spawning
            .with_max_steps(20)
            .with_temperature(0.7)
            .hidden()
            .with_color("#8b5cf6")).await;

        // Research agent - thorough investigation (similar to general but read-only)
        self.register(AgentInfo::new("research")
            .native()
            .with_display_name("Research")
            .with_description("Research agent for thorough investigation. Read-only, focuses on analysis and information gathering.")
            .with_mode(AgentMode::Subagent)
            .with_permission(PermissionConfig::read_only())
            .with_prompt(RESEARCH_PROMPT)
            .disable_tool("edit")
            .disable_tool("write")
            .disable_tool("todoread")
            .disable_tool("todowrite")
            .disable_tool("task")
            .with_max_steps(15)
            .with_temperature(0.5)
            .hidden()
            .with_color("#06b6d4")).await;

        // Title agent - generates session titles (uses small model)
        self.register(
            AgentInfo::new("title")
                .native()
                .with_description("Generates concise titles for sessions (uses small model)")
                .with_mode(AgentMode::Primary)
                .hidden()
                .with_prompt(TITLE_PROMPT)
                .with_temperature(0.7)
                .with_max_tokens(50),
        )
        .await;

        // Summary agent - generates summaries (uses small model)
        self.register(
            AgentInfo::new("summary")
                .native()
                .with_description("Generates summaries for compaction (uses small model)")
                .with_mode(AgentMode::Primary)
                .hidden()
                .with_prompt(SUMMARY_PROMPT)
                .with_temperature(0.3)
                .with_max_tokens(500),
        )
        .await;
    }

    /// Register an agent.
    pub async fn register(&self, agent: AgentInfo) {
        self.agents.write().await.insert(agent.name.clone(), agent);
    }

    /// Get an agent by name.
    pub async fn get(&self, name: &str) -> Option<AgentInfo> {
        self.agents.read().await.get(name).cloned()
    }

    /// List all agents.
    pub async fn list(&self) -> Vec<AgentInfo> {
        self.agents.read().await.values().cloned().collect()
    }

    /// List primary agents (user-facing).
    pub async fn list_primary(&self) -> Vec<AgentInfo> {
        self.agents
            .read()
            .await
            .values()
            .filter(|a| !a.hidden && matches!(a.mode, AgentMode::Primary | AgentMode::All))
            .cloned()
            .collect()
    }

    /// List sub-agents.
    pub async fn list_subagents(&self) -> Vec<AgentInfo> {
        self.agents
            .read()
            .await
            .values()
            .filter(|a| matches!(a.mode, AgentMode::Subagent | AgentMode::All))
            .cloned()
            .collect()
    }

    /// Unregister an agent.
    pub async fn unregister(&self, name: &str) -> Option<AgentInfo> {
        self.agents.write().await.remove(name)
    }

    /// Check if an agent exists.
    pub async fn exists(&self, name: &str) -> bool {
        self.agents.read().await.contains_key(name)
    }

    /// Get agent names.
    pub async fn names(&self) -> Vec<String> {
        self.agents.read().await.keys().cloned().collect()
    }

    /// Get agent by name if it's a valid subagent.
    pub async fn get_subagent(&self, name: &str) -> Option<AgentInfo> {
        let agent = self.agents.read().await.get(name).cloned()?;
        if matches!(agent.mode, AgentMode::Subagent | AgentMode::All) {
            Some(agent)
        } else {
            None
        }
    }

    /// Check if an agent name is a valid subagent.
    pub async fn is_subagent(&self, name: &str) -> bool {
        if let Some(agent) = self.agents.read().await.get(name) {
            matches!(agent.mode, AgentMode::Subagent | AgentMode::All)
        } else {
            false
        }
    }

    /// Check if an agent should use a small/lightweight model.
    pub fn should_use_small_model(name: &str) -> bool {
        SMALL_MODEL_AGENTS.contains(&name)
    }

    /// Get the title agent info.
    pub async fn get_title_agent(&self) -> Option<AgentInfo> {
        self.get("title").await
    }

    /// Get the summary agent info.
    pub async fn get_summary_agent(&self) -> Option<AgentInfo> {
        self.get("summary").await
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Built-in prompts
const EXPLORE_PROMPT: &str = r#"You are a fast, focused agent specialized in exploring codebases. Your goal is to quickly find relevant information.

## Capabilities
- Search files by patterns (glob)
- Search content by regex (grep)
- Read and analyze file contents
- Navigate directory structures

## Guidelines
1. Start with broad searches, then narrow down
2. Use glob patterns to find files by name/path
3. Use grep to search for specific code patterns
4. Use read to examine file contents
5. Be thorough but efficient - check multiple likely locations
6. Report findings with file paths and line numbers

## Thoroughness Levels
- "quick": Basic search, check obvious locations only
- "medium": Moderate exploration, check common patterns  
- "very thorough": Comprehensive analysis across multiple locations

## Output Format
Provide structured findings:
- List of relevant files found
- Key code snippets with context
- Summary of patterns discovered

Report your findings concisely but completely."#;

const GENERAL_PROMPT: &str = r#"You are a general-purpose agent specialized in complex research and multi-step tasks.

## Capabilities
- Search and explore codebases thoroughly
- Execute shell commands to gather information
- Read and analyze multiple files
- Perform web searches when needed
- Synthesize information from multiple sources

## Guidelines
- Be thorough but efficient
- Break down complex problems into smaller steps
- Focus on completing the task given to you
- Return clear, actionable results
- If you need more context, use available tools to gather it
- Do not modify files or make changes unless explicitly asked
- You can work in parallel with other agents on different aspects of a problem

## When Done
Provide a concise summary of your findings including:
1. What you found
2. Relevant file locations
3. Key insights or recommendations"#;

const RESEARCH_PROMPT: &str = r#"You are a research agent focused on thorough investigation and analysis.

## Capabilities  
- Deep code analysis and understanding
- Pattern recognition across codebases
- Documentation review and synthesis
- Dependency analysis

## Guidelines
1. Read extensively before drawing conclusions
2. Look for patterns and relationships
3. Document your findings clearly
4. Consider multiple perspectives
5. Do NOT modify any files - read-only investigation

## Output Format
Provide structured analysis:
- Executive summary
- Detailed findings with evidence
- Recommendations (if applicable)
- References to specific files and lines"#;

const TITLE_PROMPT: &str = r#"Generate a concise, descriptive title (3-7 words) for this conversation based on the user's request. 
Do not use quotes or special characters. Just output the title text directly."#;

const SUMMARY_PROMPT: &str = r#"Summarize the key points of this conversation in a concise manner:
1. What was the user's main request/goal?
2. What actions were taken?
3. What was the outcome?

Keep the summary under 200 words."#;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_registry() {
        let registry = AgentRegistry::with_defaults().await;

        let build = registry.get("build").await;
        assert!(build.is_some());
        assert!(build.unwrap().permission.edit.is_allowed());

        let plan = registry.get("plan").await;
        assert!(plan.is_some());
        assert!(plan.unwrap().permission.edit.is_denied());
    }

    #[tokio::test]
    async fn test_list_primary() {
        let registry = AgentRegistry::with_defaults().await;
        let primary = registry.list_primary().await;

        // build and plan should be listed
        assert!(primary.iter().any(|a| a.name == "build"));
        assert!(primary.iter().any(|a| a.name == "plan"));

        // hidden agents should not be listed
        assert!(!primary.iter().any(|a| a.name == "title"));
    }

    #[tokio::test]
    async fn test_general_agent() {
        let registry = AgentRegistry::with_defaults().await;

        let general = registry.get("general").await;
        assert!(general.is_some());
        let general = general.unwrap();

        assert_eq!(general.mode, AgentMode::Subagent);
        assert!(general.hidden);
        assert!(general.permission.edit.is_allowed());
        assert!(!general.is_tool_enabled("task")); // Recursive spawning disabled
        assert!(!general.is_tool_enabled("todoread"));
        assert!(!general.is_tool_enabled("todowrite"));
        assert_eq!(general.max_steps, Some(20));
    }

    #[tokio::test]
    async fn test_research_agent() {
        let registry = AgentRegistry::with_defaults().await;

        let research = registry.get("research").await;
        assert!(research.is_some());
        let research = research.unwrap();

        assert_eq!(research.mode, AgentMode::Subagent);
        assert!(research.hidden);
        assert!(research.permission.edit.is_denied()); // Read-only
    }

    #[tokio::test]
    async fn test_list_subagents() {
        let registry = AgentRegistry::with_defaults().await;
        let subagents = registry.list_subagents().await;

        // Should include explore, general, research
        assert!(subagents.iter().any(|a| a.name == "explore"));
        assert!(subagents.iter().any(|a| a.name == "general"));
        assert!(subagents.iter().any(|a| a.name == "research"));

        // Should not include primary-only agents
        assert!(!subagents.iter().any(|a| a.name == "build"));
        assert!(!subagents.iter().any(|a| a.name == "plan"));
    }

    #[tokio::test]
    async fn test_get_subagent() {
        let registry = AgentRegistry::with_defaults().await;

        // Should return subagent
        let general = registry.get_subagent("general").await;
        assert!(general.is_some());

        // Should not return primary agent
        let build = registry.get_subagent("build").await;
        assert!(build.is_none());

        // Should not return non-existent agent
        let nonexistent = registry.get_subagent("nonexistent").await;
        assert!(nonexistent.is_none());
    }

    #[tokio::test]
    async fn test_is_subagent() {
        let registry = AgentRegistry::with_defaults().await;

        assert!(registry.is_subagent("general").await);
        assert!(registry.is_subagent("explore").await);
        assert!(registry.is_subagent("research").await);
        assert!(!registry.is_subagent("build").await);
        assert!(!registry.is_subagent("plan").await);
        assert!(!registry.is_subagent("nonexistent").await);
    }
}
