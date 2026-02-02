//! Sub-agents system for Cortex CLI.
//!
//! Sub-agents are specialized AI agents that can be invoked for specific tasks.
//! They can have their own system prompts, tool restrictions, and configuration.
//!
//! Agents can be defined in:
//! - OS-specific agents directory (Windows: %APPDATA%\Cortex\agents, macOS: ~/Library/Application Support/Cortex/agents, Linux: ~/.local/share/cortex/agents)
//! - Plugin agents directories
//! - Project .cortex/agents/ directory
//! - Personal ~/.cortex/agents/ directory
//!
//! Agent definition format (AGENT.md or agent.json):
//! - name: Agent identifier
//! - description: When to use this agent
//! - model: Optional model override
//! - temperature: Optional temperature override
//! - allowed-tools: Tool restrictions
//! - system-prompt: Custom system prompt or path to prompt file
//! - enabled: Whether agent is available in Task tool (default: true)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

// ============================================================================
// OS-Specific Agents Directory
// ============================================================================

/// Get the OS-specific agents directory for custom agents created via IDE/desktop.
///
/// Returns:
/// - Windows: `%APPDATA%\Cortex\agents` (e.g., `C:\Users\<user>\AppData\Roaming\Cortex\agents`)
/// - macOS: `~/Library/Application Support/Cortex/agents`
/// - Linux: `~/.local/share/cortex/agents`
pub fn get_os_agents_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir().map(|p| p.join("Cortex").join("agents"))
    }
    #[cfg(target_os = "macos")]
    {
        dirs::data_dir().map(|p| p.join("Cortex").join("agents"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        dirs::data_local_dir().map(|p| p.join("Cortex").join("agents"))
    }
}

/// Get the OS-specific Cortex data directory (parent of agents dir).
///
/// Returns:
/// - Windows: `%APPDATA%\Cortex`
/// - macOS: `~/Library/Application Support/Cortex`
/// - Linux: `~/.local/share/cortex`
pub fn get_os_cortex_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir().map(|p| p.join("Cortex"))
    }
    #[cfg(target_os = "macos")]
    {
        dirs::data_dir().map(|p| p.join("Cortex"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        dirs::data_local_dir().map(|p| p.join("Cortex"))
    }
}

/// Agent metadata from definition file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    /// Agent name (unique identifier).
    pub name: String,
    /// Description of what this agent does.
    pub description: String,
    /// Model to use (overrides default).
    pub model: Option<String>,
    /// Temperature setting.
    pub temperature: Option<f32>,
    /// Maximum tokens for response.
    pub max_tokens: Option<u32>,
    /// Allowed tools (None means all tools).
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Denied tools.
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// Custom system prompt.
    pub system_prompt: Option<String>,
    /// Path to system prompt file.
    pub prompt_file: Option<String>,
    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether agent can spawn sub-agents.
    #[serde(default = "default_can_delegate")]
    pub can_delegate: bool,
    /// Maximum number of turns.
    pub max_turns: Option<u32>,
    /// Whether agent is enabled (available in Task tool).
    /// Agents with enabled=false are not registered with the Task tool.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_can_delegate() -> bool {
    true
}

fn default_enabled() -> bool {
    true
}

impl AgentMetadata {
    /// Create a new agent metadata.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            model: None,
            temperature: None,
            max_tokens: None,
            allowed_tools: None,
            denied_tools: Vec::new(),
            system_prompt: None,
            prompt_file: None,
            tags: Vec::new(),
            can_delegate: true,
            max_turns: None,
            enabled: true,
        }
    }

    /// Validate the metadata.
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(CortexError::InvalidInput(
                "Agent name cannot be empty".to_string(),
            ));
        }

        if self.name.len() > 64 {
            return Err(CortexError::InvalidInput(
                "Agent name must be at most 64 characters".to_string(),
            ));
        }

        if let Some(temp) = self.temperature
            && !(0.0..=2.0).contains(&temp)
        {
            return Err(CortexError::InvalidInput(
                "Temperature must be between 0.0 and 2.0".to_string(),
            ));
        }

        Ok(())
    }
}

/// Source of an agent definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    /// Built-in agent.
    Builtin,
    /// Personal agent from ~/.cortex/agents/.
    Personal,
    /// Project agent from .cortex/agents/.
    Project,
    /// Plugin-provided agent.
    Plugin,
}

impl std::fmt::Display for AgentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin => write!(f, "builtin"),
            Self::Personal => write!(f, "personal"),
            Self::Project => write!(f, "project"),
            Self::Plugin => write!(f, "plugin"),
        }
    }
}

/// A loaded agent definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    /// Agent metadata.
    pub metadata: AgentMetadata,
    /// Full system prompt (resolved from prompt_file if needed).
    pub system_prompt: String,
    /// Path to agent definition.
    pub path: PathBuf,
    /// Source of the agent.
    pub source: AgentSource,
}

impl Agent {
    /// Get the agent's ID.
    pub fn id(&self) -> &str {
        &self.metadata.name
    }

    /// Check if agent can use a specific tool.
    pub fn can_use_tool(&self, tool_name: &str) -> bool {
        // Check denied list first
        if self.metadata.denied_tools.iter().any(|t| t == tool_name) {
            return false;
        }

        // Check allowed list
        match &self.metadata.allowed_tools {
            Some(allowed) => allowed.iter().any(|t| t == tool_name || t == "*"),
            None => true, // No restriction
        }
    }

    /// Get the effective model for this agent.
    pub fn effective_model(&self, default: &str) -> String {
        self.metadata
            .model
            .clone()
            .unwrap_or_else(|| default.to_string())
    }

    /// Get the effective temperature.
    pub fn effective_temperature(&self, default: f32) -> f32 {
        self.metadata.temperature.unwrap_or(default)
    }
}

/// Agent registry for managing agent definitions.
pub struct AgentRegistry {
    /// Registered agents by name.
    agents: RwLock<HashMap<String, Agent>>,
    /// Personal agents directory.
    personal_dir: PathBuf,
    /// Project agents directory.
    project_dir: Option<PathBuf>,
    /// Plugin agent directories.
    plugin_dirs: Vec<PathBuf>,
}

impl AgentRegistry {
    /// Create a new agent registry.
    pub fn new(cortex_home: &Path, project_root: Option<&Path>) -> Self {
        let personal_dir = cortex_home.join("agents");
        let project_dir = project_root.map(|p| p.join(".cortex").join("agents"));

        Self {
            agents: RwLock::new(HashMap::new()),
            personal_dir,
            project_dir,
            plugin_dirs: Vec::new(),
        }
    }

    /// Add a plugin agents directory.
    pub fn add_plugin_dir(&mut self, dir: PathBuf) {
        self.plugin_dirs.push(dir);
    }

    /// Scan and load all agents.
    pub async fn scan(&self) -> Result<Vec<Agent>> {
        let mut all_agents = Vec::new();

        // Load built-in agents
        all_agents.extend(self.load_builtin_agents());

        // Scan OS-specific agents directory (custom agents from IDE/desktop)
        // This takes priority over other sources for custom agents
        if let Some(os_agents_dir) = get_os_agents_dir() {
            if os_agents_dir.exists() {
                tracing::debug!(path = %os_agents_dir.display(), "Scanning OS agents directory");
                match self.scan_directory(&os_agents_dir, AgentSource::Personal) {
                    Ok(agents) => {
                        // Only include enabled agents
                        let enabled_agents: Vec<_> =
                            agents.into_iter().filter(|a| a.metadata.enabled).collect();
                        tracing::info!(
                            count = enabled_agents.len(),
                            "Loaded custom agents from OS directory"
                        );
                        all_agents.extend(enabled_agents);
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %os_agents_dir.display(),
                            error = %e,
                            "Failed to scan OS agents directory"
                        );
                    }
                }
            }
        }

        // Scan personal agents (~/.cortex/agents)
        if self.personal_dir.exists() {
            let agents = self.scan_directory(&self.personal_dir, AgentSource::Personal)?;
            // Filter out disabled agents
            let enabled_agents: Vec<_> =
                agents.into_iter().filter(|a| a.metadata.enabled).collect();
            all_agents.extend(enabled_agents);
        }

        // Scan project agents
        if let Some(ref project_dir) = self.project_dir
            && project_dir.exists()
        {
            let agents = self.scan_directory(project_dir, AgentSource::Project)?;
            // Filter out disabled agents
            let enabled_agents: Vec<_> =
                agents.into_iter().filter(|a| a.metadata.enabled).collect();
            all_agents.extend(enabled_agents);
        }

        // Scan plugin agents
        for plugin_dir in &self.plugin_dirs {
            if plugin_dir.exists() {
                let agents = self.scan_directory(plugin_dir, AgentSource::Plugin)?;
                // Filter out disabled agents
                let enabled_agents: Vec<_> =
                    agents.into_iter().filter(|a| a.metadata.enabled).collect();
                all_agents.extend(enabled_agents);
            }
        }

        // Register all agents
        let mut registry = self.agents.write().await;
        for agent in &all_agents {
            registry.insert(agent.metadata.name.clone(), agent.clone());
        }

        Ok(all_agents)
    }

    /// Load built-in agents.
    fn load_builtin_agents(&self) -> Vec<Agent> {
        vec![
            Agent {
                metadata: AgentMetadata {
                    name: "code-explorer".to_string(),
                    description: "Explore and understand codebases. Use for analyzing code structure, finding patterns, and understanding implementations.".to_string(),
                    model: None,
                    temperature: Some(0.3),
                    max_tokens: Some(4096),
                    allowed_tools: Some(vec![
                        "Read".to_string(),
                        "Grep".to_string(),
                        "Glob".to_string(),
                        "LS".to_string(),
                    ]),
                    denied_tools: Vec::new(),
                    system_prompt: None,
                    prompt_file: None,
                    tags: vec!["code".to_string(), "analysis".to_string()],
                    can_delegate: false,
                    max_turns: Some(10),
                    enabled: true,
                },
                system_prompt: CODE_EXPLORER_PROMPT.to_string(),
                path: PathBuf::new(),
                source: AgentSource::Builtin,
            },
            Agent {
                metadata: AgentMetadata {
                    name: "code-reviewer".to_string(),
                    description: "Review code for quality, bugs, and best practices. Use for PR reviews and code audits.".to_string(),
                    model: None,
                    temperature: Some(0.2),
                    max_tokens: Some(4096),
                    allowed_tools: Some(vec![
                        "Read".to_string(),
                        "Grep".to_string(),
                        "Glob".to_string(),
                    ]),
                    denied_tools: vec!["Execute".to_string()],
                    system_prompt: None,
                    prompt_file: None,
                    tags: vec!["review".to_string(), "quality".to_string()],
                    can_delegate: false,
                    max_turns: Some(5),
                    enabled: true,
                },
                system_prompt: CODE_REVIEWER_PROMPT.to_string(),
                path: PathBuf::new(),
                source: AgentSource::Builtin,
            },
            Agent {
                metadata: AgentMetadata {
                    name: "architect".to_string(),
                    description: "Design software architecture and make high-level technical decisions. Use for planning new features or refactoring.".to_string(),
                    model: None,
                    temperature: Some(0.5),
                    max_tokens: Some(8192),
                    allowed_tools: Some(vec![
                        "Read".to_string(),
                        "Grep".to_string(),
                        "Glob".to_string(),
                        "LS".to_string(),
                    ]),
                    denied_tools: vec!["Execute".to_string()],
                    system_prompt: None,
                    prompt_file: None,
                    tags: vec!["architecture".to_string(), "design".to_string()],
                    can_delegate: true,
                    max_turns: Some(15),
                    enabled: true,
                },
                system_prompt: ARCHITECT_PROMPT.to_string(),
                path: PathBuf::new(),
                source: AgentSource::Builtin,
            },
        ]
    }

    /// Scan a directory for agent definitions.
    fn scan_directory(&self, dir: &Path, source: AgentSource) -> Result<Vec<Agent>> {
        let mut agents = Vec::new();

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Check for AGENT.md or agent.json
                let agent_md = path.join("AGENT.md");
                let agent_json = path.join("agent.json");

                if agent_md.exists() {
                    match self.load_agent_md(&path, &agent_md, source) {
                        Ok(agent) => agents.push(agent),
                        Err(e) => {
                            tracing::warn!("Failed to load agent from {}: {}", path.display(), e);
                        }
                    }
                } else if agent_json.exists() {
                    match self.load_agent_json(&path, &agent_json, source) {
                        Ok(agent) => agents.push(agent),
                        Err(e) => {
                            tracing::warn!("Failed to load agent from {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }

        Ok(agents)
    }

    /// Load an agent from AGENT.md format.
    fn load_agent_md(
        &self,
        agent_dir: &Path,
        agent_md: &Path,
        source: AgentSource,
    ) -> Result<Agent> {
        let content = std::fs::read_to_string(agent_md)?;
        let (metadata, prompt) = parse_agent_md(&content)?;

        metadata.validate()?;

        // Resolve system prompt
        let system_prompt = if let Some(ref prompt_file) = metadata.prompt_file {
            let prompt_path = agent_dir.join(prompt_file);
            std::fs::read_to_string(&prompt_path)?
        } else if let Some(ref prompt) = metadata.system_prompt {
            prompt.clone()
        } else {
            prompt
        };

        Ok(Agent {
            metadata,
            system_prompt,
            path: agent_dir.to_path_buf(),
            source,
        })
    }

    /// Load an agent from JSON format.
    fn load_agent_json(
        &self,
        agent_dir: &Path,
        agent_json: &Path,
        source: AgentSource,
    ) -> Result<Agent> {
        let content = std::fs::read_to_string(agent_json)?;
        let metadata: AgentMetadata = serde_json::from_str(&content)?;

        metadata.validate()?;

        // Resolve system prompt
        let system_prompt = if let Some(ref prompt_file) = metadata.prompt_file {
            let prompt_path = agent_dir.join(prompt_file);
            std::fs::read_to_string(&prompt_path)?
        } else {
            metadata.system_prompt.clone().unwrap_or_default()
        };

        Ok(Agent {
            metadata,
            system_prompt,
            path: agent_dir.to_path_buf(),
            source,
        })
    }

    /// Get an agent by name.
    pub async fn get(&self, name: &str) -> Option<Agent> {
        self.agents.read().await.get(name).cloned()
    }

    /// List all agents.
    pub async fn list(&self) -> Vec<Agent> {
        self.agents.read().await.values().cloned().collect()
    }

    /// Find agents by tag.
    pub async fn find_by_tag(&self, tag: &str) -> Vec<Agent> {
        self.agents
            .read()
            .await
            .values()
            .filter(|a| a.metadata.tags.iter().any(|t| t == tag))
            .cloned()
            .collect()
    }

    /// Find agents relevant to a query.
    pub async fn find_relevant(&self, query: &str) -> Vec<Agent> {
        let query_lower = query.to_lowercase();

        self.agents
            .read()
            .await
            .values()
            .filter(|a| {
                let desc_lower = a.metadata.description.to_lowercase();
                let name_lower = a.metadata.name.to_lowercase();

                query_lower
                    .split_whitespace()
                    .any(|term| desc_lower.contains(term) || name_lower.contains(term))
            })
            .cloned()
            .collect()
    }

    /// Reload all agents.
    pub async fn reload(&self) -> Result<Vec<Agent>> {
        self.agents.write().await.clear();
        self.scan().await
    }

    /// Check if an agent exists by name.
    pub async fn exists(&self, name: &str) -> bool {
        self.agents.read().await.contains_key(name)
    }

    /// Get the count of registered agents.
    pub async fn count(&self) -> usize {
        self.agents.read().await.len()
    }

    /// List agent names only (lightweight).
    pub async fn list_names(&self) -> Vec<String> {
        self.agents.read().await.keys().cloned().collect()
    }

    /// Get agents by source type.
    pub async fn list_by_source(&self, source: AgentSource) -> Vec<Agent> {
        self.agents
            .read()
            .await
            .values()
            .filter(|a| a.source == source)
            .cloned()
            .collect()
    }
}

/// Parse AGENT.md format.
pub fn parse_agent_md(content: &str) -> Result<(AgentMetadata, String)> {
    let content = content.trim();

    if !content.starts_with("---") {
        return Err(CortexError::InvalidInput(
            "AGENT.md must start with YAML frontmatter (---)".to_string(),
        ));
    }

    let rest = &content[3..];
    let end_idx = rest.find("\n---").ok_or_else(|| {
        CortexError::InvalidInput("Missing closing --- for YAML frontmatter".to_string())
    })?;

    let yaml_content = &rest[..end_idx].trim();
    let markdown_content = &rest[end_idx + 4..].trim();

    let metadata: AgentMetadata = serde_yaml::from_str(yaml_content)
        .map_err(|e| CortexError::InvalidInput(format!("Invalid YAML: {e}")))?;

    Ok((metadata, markdown_content.to_string()))
}

/// Agent execution context.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// Parent session ID.
    pub session_id: String,
    /// Parent turn ID.
    pub parent_turn_id: String,
    /// Current working directory.
    pub cwd: PathBuf,
    /// Depth of delegation (0 = main agent).
    pub delegation_depth: u32,
    /// Maximum delegation depth.
    pub max_delegation_depth: u32,
    /// Task to execute.
    pub task: String,
}

impl AgentContext {
    /// Create a new agent context.
    pub fn new(
        session_id: impl Into<String>,
        cwd: impl Into<PathBuf>,
        task: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            parent_turn_id: String::new(),
            cwd: cwd.into(),
            delegation_depth: 0,
            max_delegation_depth: 3,
            task: task.into(),
        }
    }

    /// Check if further delegation is allowed.
    pub fn can_delegate(&self) -> bool {
        self.delegation_depth < self.max_delegation_depth
    }

    /// Create a child context for delegation.
    pub fn child(&self, task: impl Into<String>) -> Self {
        Self {
            session_id: self.session_id.clone(),
            parent_turn_id: self.parent_turn_id.clone(),
            cwd: self.cwd.clone(),
            delegation_depth: self.delegation_depth + 1,
            max_delegation_depth: self.max_delegation_depth,
            task: task.into(),
        }
    }
}

/// Result of an agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    /// Whether the agent succeeded.
    pub success: bool,
    /// Output/response from the agent.
    pub output: String,
    /// Any artifacts produced.
    #[serde(default)]
    pub artifacts: Vec<AgentArtifact>,
    /// Error message if failed.
    pub error: Option<String>,
    /// Token usage.
    pub tokens_used: Option<u64>,
}

/// Artifact produced by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentArtifact {
    /// Artifact type.
    pub artifact_type: String,
    /// Artifact name.
    pub name: String,
    /// Artifact content or path.
    pub content: String,
}

impl AgentResult {
    /// Create a success result.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            artifacts: Vec::new(),
            error: None,
            tokens_used: None,
        }
    }

    /// Create an error result.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            artifacts: Vec::new(),
            error: Some(message.into()),
            tokens_used: None,
        }
    }

    /// Add an artifact.
    pub fn with_artifact(
        mut self,
        artifact_type: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        self.artifacts.push(AgentArtifact {
            artifact_type: artifact_type.into(),
            name: name.into(),
            content: content.into(),
        });
        self
    }
}

// Built-in agent prompts

const CODE_EXPLORER_PROMPT: &str = r#"You are a code exploration specialist. Your role is to analyze and understand codebases.

## Capabilities
- Read and analyze source code files
- Search for patterns and implementations
- Understand project structure and architecture
- Find dependencies and relationships between components

## Guidelines
1. Start by understanding the project structure (package.json, Cargo.toml, etc.)
2. Use Grep to find specific patterns or implementations
3. Use Glob to find files by type or name pattern
4. Read files to understand implementation details
5. Provide clear, structured summaries of your findings

## Output Format
Provide findings in a clear, organized manner:
- Project structure overview
- Key components and their purposes
- Important patterns or conventions used
- Relevant code snippets with explanations
"#;

const CODE_REVIEWER_PROMPT: &str = r#"You are a code review specialist. Your role is to review code for quality, bugs, and best practices.

## Review Checklist
1. **Correctness**: Does the code do what it's supposed to do?
2. **Security**: Are there any security vulnerabilities?
3. **Performance**: Are there any performance issues?
4. **Readability**: Is the code easy to understand?
5. **Maintainability**: Is the code easy to maintain and extend?
6. **Testing**: Is the code properly tested?
7. **Documentation**: Is the code properly documented?

## Guidelines
- Focus on substantive issues, not style nitpicks
- Provide specific, actionable feedback
- Include code examples when suggesting improvements
- Prioritize issues by severity (critical, major, minor)

## Output Format
Organize feedback by category:
- Critical Issues (must fix)
- Major Issues (should fix)
- Minor Issues (nice to fix)
- Suggestions (optional improvements)
"#;

const ARCHITECT_PROMPT: &str = r#"You are a software architect. Your role is to design software systems and make high-level technical decisions.

## Responsibilities
- Design system architecture
- Define component boundaries
- Choose appropriate patterns and technologies
- Ensure scalability, maintainability, and security
- Document architectural decisions

## Guidelines
1. Understand current system state before proposing changes
2. Consider trade-offs of different approaches
3. Design for change and extensibility
4. Keep solutions as simple as possible
5. Document decisions and rationale

## Output Format
Provide architectural recommendations with:
- Current state analysis
- Proposed architecture/changes
- Component diagram (text-based)
- Trade-offs and alternatives considered
- Implementation roadmap
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_metadata_validation() {
        let meta = AgentMetadata::new("test-agent", "Test agent");
        assert!(meta.validate().is_ok());

        let meta = AgentMetadata::new("", "Empty name");
        assert!(meta.validate().is_err());

        let mut meta = AgentMetadata::new("test", "Test");
        meta.temperature = Some(3.0);
        assert!(meta.validate().is_err());
    }

    #[test]
    fn test_agent_tool_permissions() {
        let agent = Agent {
            metadata: AgentMetadata {
                name: "test".to_string(),
                description: "Test".to_string(),
                model: None,
                temperature: None,
                max_tokens: None,
                allowed_tools: Some(vec!["Read".to_string(), "Grep".to_string()]),
                denied_tools: vec!["Execute".to_string()],
                system_prompt: None,
                prompt_file: None,
                tags: Vec::new(),
                can_delegate: true,
                max_turns: None,
                enabled: true,
            },
            system_prompt: String::new(),
            path: PathBuf::new(),
            source: AgentSource::Builtin,
        };

        assert!(agent.can_use_tool("Read"));
        assert!(agent.can_use_tool("Grep"));
        assert!(!agent.can_use_tool("Write"));
        assert!(!agent.can_use_tool("Execute"));
    }

    #[test]
    fn test_parse_agent_md() {
        let content = r#"---
name: my-agent
description: A test agent
model: gpt-4
temperature: 0.5
allowed-tools:
  - Read
  - Grep
---

# My Agent

This is the system prompt for my agent.
"#;

        let (metadata, prompt) = parse_agent_md(content).unwrap();

        assert_eq!(metadata.name, "my-agent");
        assert_eq!(metadata.model, Some("gpt-4".to_string()));
        assert!(prompt.contains("system prompt"));
    }

    #[test]
    fn test_agent_context() {
        let ctx = AgentContext::new("session-1", "/project", "Analyze the codebase");
        assert!(ctx.can_delegate());

        let child = ctx.child("Review file.rs");
        assert_eq!(child.delegation_depth, 1);
        assert!(child.can_delegate());
    }
}
