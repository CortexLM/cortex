//! Centralized system prompts for Cortex CLI.
//!
//! This module provides a single source of truth for all system prompts
//! used throughout the Cortex CLI. By centralizing prompts here, we:
//!
//! - Maintain consistency across different components
//! - Make it easy to review and update prompts
//! - Enable better testing of prompt behavior
//! - Provide a clear overview of the AI's instruction harness
//!
//! # Organization
//!
//! Prompts are organized by category:
//!
//! - [`core`]: Core agent prompts (Cortex main prompt, TUI prompt)
//! - [`agents`]: Built-in agent prompts (explore, general, research, etc.)
//! - [`builtin_skills`]: On-demand skills for context-aware loading
//! - [`tasks`]: Task-related prompts (summarization, compaction, titles)
//! - [`review`]: Code review prompts
//! - [`tools`]: Tool-specific prompts (subagent executor, mentions)
//! - [`generation`]: Agent generation prompts
//! - [`top_agent`]: Top-agent style prompts (autonomous, backup-first philosophy)
//!
//! # Usage
//!
//! ```rust
//! use cortex_prompt_harness::prompts;
//!
//! // Get the main Cortex system prompt
//! let prompt = prompts::core::CORTEX_MAIN_PROMPT;
//!
//! // Get agent-specific prompts
//! let explore = prompts::agents::EXPLORE_AGENT_PROMPT;
//! let research = prompts::agents::RESEARCH_AGENT_PROMPT;
//!
//! // Get task prompts
//! let summary = prompts::tasks::SUMMARIZATION_PROMPT;
//!
//! // Use top-agent style prompts
//! let top_agent = prompts::top_agent::TOP_AGENT_SYSTEM_PROMPT;
//! let coding = prompts::top_agent::TopAgentPresets::coding_assistant();
//! ```

pub mod agents;
pub mod base_agent;
pub mod builtin_skills;
pub mod core;
pub mod generation;
pub mod review;
pub mod tasks;
pub mod tools;
pub mod top_agent;

// Re-export commonly used prompts for convenience
pub use agents::{
    EXPLORE_AGENT_PROMPT, GENERAL_AGENT_PROMPT, RESEARCH_AGENT_PROMPT, SUMMARY_AGENT_PROMPT,
    TITLE_AGENT_PROMPT,
};
pub use base_agent::{
    AVAILABLE_SKILLS, CORTEX_BASE_PROMPT, CORTEX_BASE_PROMPT_WITH_SKILLS_PRELOADED,
    format_skill_loading_prompt, get_recommended_skills,
};
pub use builtin_skills::{
    BUILTIN_SKILL_NAMES, SKILL_CODE_QUALITY, SKILL_DEBUGGING, SKILL_FILE_OPERATIONS, SKILL_GIT,
    SKILL_PLANNING, SKILL_SECURITY, builtin_skill_count, get_builtin_skill, is_builtin_skill,
    list_builtin_skills,
};
pub use core::{
    CORTEX_MAIN_PROMPT, CortexPromptBuilder, SECTION_ANTI_PATTERNS, SECTION_CODE_DISCIPLINE,
    SECTION_COGNITIVE_ARCHITECTURE, SECTION_FAILURE_PROTOCOL, SECTION_HEADER, SECTION_NAMES,
    SECTION_OUTPUT_FORMAT, SECTION_PRIME_DIRECTIVES, SECTION_QUALITY_CHECKPOINTS,
    SECTION_RESPONSE_PATTERNS, SECTION_TOOLKIT, TUI_SYSTEM_PROMPT_TEMPLATE,
};
pub use tasks::{COMPACTION_PROMPT, SUMMARIZATION_PROMPT};
pub use top_agent::{
    TOP_AGENT_SECTION_NAMES, TOP_AGENT_SYSTEM_PROMPT, TopAgentPresets, TopAgentPromptBuilder,
};
