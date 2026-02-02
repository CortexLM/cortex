//! Custom Agent (Subagent) system for Cortex CLI.
//!
//! Custom agents are reusable, configurable subagents defined in Markdown files
//! with YAML frontmatter. They can be invoked via slash commands or @mentions.
//!
//! # Custom Agent File Format
//!
//! ```markdown
//! ---
//! name: code-reviewer
//! description: Reviews diffs for correctness and security risks
//! model: inherit
//! reasoning_effort: high
//! tools: read-only
//! ---
//!
//! You are the team's senior code reviewer...
//! ```
//!
//! # Tool Categories
//!
//! - `read-only`: Read, LS, Grep, Glob
//! - `edit`: Create, Edit, ApplyPatch
//! - `execute`: Execute (shell commands)
//! - `web`: WebSearch, FetchUrl
//! - `all`: All available tools
//!
//! # Usage
//!
//! ```rust,ignore
//! use cortex_agents::custom::{CustomAgentLoader, CustomAgentRegistry};
//!
//! let loader = CustomAgentLoader::new().with_default_paths(Some(&project_root));
//! let agents = loader.load_all().await?;
//!
//! let mut registry = CustomAgentRegistry::new();
//! for agent in agents {
//!     registry.register(agent);
//! }
//!
//! if let Some(info) = registry.to_agent_info("code-reviewer") {
//!     // Use the agent info...
//! }
//! ```

pub mod config;
pub mod loader;
pub mod registry;

pub use config::{CustomAgentConfig, CustomAgentError, ReasoningEffort, ToolCategory, ToolsConfig};
pub use loader::CustomAgentLoader;
pub use registry::CustomAgentRegistry;
