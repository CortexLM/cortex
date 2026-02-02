//! Multi-agent system for Cortex CLI.
//!
//! Provides different agents with different capabilities:
//! - **build**: Full access agent for development work
//! - **plan**: Read-only agent for analysis and exploration
//! - **explore**: Fast agent for codebase exploration
//! - **general**: Sub-agent for parallel tasks
//! - **research**: Read-only investigation agent
//!
//! # Operation Modes
//!
//! Agents can operate in different modes that control their capabilities:
//! - **Build**: Full access to modify files and execute commands
//! - **Plan**: Read-only, describes changes without applying them
//! - **Spec**: Generates a detailed plan for user approval before building
//!
//! ```rust,ignore
//! use cortex_agents::spec::OperationMode;
//!
//! let mode = OperationMode::Build;
//! assert!(mode.can_write());
//!
//! let mode = OperationMode::Plan;
//! assert!(!mode.can_write());
//! ```
//!
//! # @Mention Support
//!
//! Users can invoke subagents directly using @agent syntax:
//! ```text
//! @general find all error handling patterns
//! @explore search for config files
//! @research analyze the authentication flow
//! ```
//!
//! # Agent Control System
//!
//! The `AgentControl` system provides centralized management of agent threads,
//! including spawning, status tracking, and lifecycle management.
//!
//! ```rust,ignore
//! use cortex_agents::{AgentControl, AgentInfo};
//!
//! let control = AgentControl::new();
//! let id = control.spawn_agent(AgentInfo::new("test"), None).await?;
//! let status = control.get_status(id).await;
//! ```
//!
//! # Custom Agent System
//!
//! Custom agents are reusable, configurable subagents defined in Markdown files:
//!
//! ```rust,ignore
//! use cortex_agents::custom::{CustomAgentLoader, CustomAgentRegistry};
//!
//! let loader = CustomAgentLoader::with_default_paths(Some(&project_root));
//! let agents = loader.load_all().await?;
//!
//! let registry = CustomAgentRegistry::from_agents(agents);
//! if let Some(info) = registry.to_agent_info("code-reviewer") {
//!     // Use the agent info
//! }
//! ```
//!
//! # Multi-Agent Collaboration
//!
//! The collaboration module provides tools for spawning and managing subagents:
//!
//! ```rust,ignore
//! use cortex_agents::collab::{spawn, wait, close, SpawnAgentArgs, WaitArgs};
//!
//! // Spawn a subagent
//! let result = spawn::handle(&control, SpawnAgentArgs {
//!     message: "Search for patterns".to_string(),
//!     agent_type: None,
//!     config: None,
//! }, None).await?;
//!
//! // Wait for completion
//! let wait_result = wait::handle(&control, WaitArgs {
//!     ids: vec![result.agent_id],
//!     timeout_ms: Some(60_000),
//! }).await?;
//! ```
//!
//! # Task DAG
//!
//! The task module provides DAG-based dependency management:
//!
//! ```rust
//! use cortex_agents::task::{TaskDag, Task, DagBuilder};
//!
//! let dag = DagBuilder::new()
//!     .add_task("setup", "Initialize")
//!     .add_task("build", "Build project")
//!     .depends_on("setup")
//!     .build()
//!     .unwrap();
//! ```
//!
//! # Routing
//!
//! The routing module helps decide how to dispatch tasks:
//!
//! ```rust
//! use cortex_agents::routing::{decide_routing, TaskInfo, DispatchMode};
//!
//! let tasks = vec![
//!     TaskInfo::new("Task 1").read_only(),
//!     TaskInfo::new("Task 2").read_only(),
//! ];
//! let decision = decide_routing(&tasks);
//! assert_eq!(decision.mode, DispatchMode::Parallel);
//! ```

pub mod background;
pub mod collab;
pub mod control;
pub mod custom;
pub mod forge;
pub mod mention;
pub mod permission;
pub mod prompt;
pub mod registry;
pub mod routing;
pub mod spec;
pub mod task;

// Re-export custom module as 'agent' for backward compatibility and simplicity
mod agent;

pub use agent::{
    create_explore_agent,
    // Built-in agent factories
    create_general_agent,
    create_research_agent,
    Agent,
    AgentInfo,
    AgentMode,
    EXPLORE_AGENT_PROMPT,
    // Agent prompts
    GENERAL_AGENT_PROMPT,
    RESEARCH_AGENT_PROMPT,
};
pub use control::{
    AgentControl, AgentControlError, AgentControlState, AgentLimits, AgentThread, AgentThreadId,
    AgentThreadStatus,
};
pub use mention::{
    extract_mention_and_text, find_first_valid_mention, parse_agent_mentions,
    parse_message_for_agent, starts_with_mention, AgentMention, ParsedAgentMessage,
    BUILTIN_SUBAGENTS, MENTION_SYSTEM_PROMPT,
};
pub use permission::{Permission, PermissionConfig};
pub use prompt::{build_system_prompt, build_system_prompt_with_mentions, build_tool_context};
pub use registry::{AgentRegistry, SMALL_MODEL_AGENTS};
pub use spec::{
    ApprovalDecision, ApprovalManager, ApprovalRequest, ChangeType, FileChange, ModeTransition,
    OperationMode, SpecPlan, SpecStep,
};

// Re-export collaboration types
pub use collab::{
    AgentGuard, CloseAgentArgs, CloseAgentResult, CollabError, CollabResult, PendingInput,
    SendInputArgs, SendInputResult, SpawnAgentArgs, SpawnAgentResult, SpawnConfig, WaitArgs,
    WaitResult, DEFAULT_WAIT_TIMEOUT_MS, MAX_CONCURRENT_AGENTS, MAX_THREAD_SPAWN_DEPTH,
    MAX_WAIT_TIMEOUT_MS, MIN_WAIT_TIMEOUT_MS,
};

// Re-export task types
pub use task::{
    execute_dag, DagBuilder, DagError, DagHydrator, DagResult, DagStore, ExecutionProgress,
    ExecutorConfig, ExecutorError, ExecutorResult, InMemoryDagStore, PersistenceError,
    SerializedDag, SessionHydrationError, SessionHydrationResult, SessionHydrator,
    SessionRestoreConfig, StaleTaskChecker, StaleTaskInfo, Task, TaskDag, TaskExecutionResult,
    TaskExecutor, TaskId, TaskSpec, TaskStatus, TaskStore,
};

// Re-export routing types
pub use routing::{
    can_parallelize, decide_routing, estimate_duration, DispatchMode, RoutingDecision, TaskInfo,
};

// Re-export background agent types
pub use background::{
    AgentConfig, AgentEvent, AgentMailbox, AgentMessage, AgentMessageBroker, AgentResult,
    AgentStatus, BackgroundAgent, BackgroundAgentManager, BackgroundAgentManagerError,
    MessageContent, RunningAgentInfo,
};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error("Agent not found: {0}")]
    NotFound(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("Agent configuration error: {0}")]
    ConfigError(String),
    #[error("Control error: {0}")]
    ControlError(#[from] AgentControlError),
}

pub type Result<T> = std::result::Result<T, AgentError>;
