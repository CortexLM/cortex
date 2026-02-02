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
//! - [`tasks`]: Task-related prompts (summarization, compaction, titles)
//! - [`review`]: Code review prompts
//! - [`tools`]: Tool-specific prompts (subagent executor, mentions)
//! - [`generation`]: Agent generation prompts
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
//! ```

pub mod agents;
pub mod core;
pub mod generation;
pub mod review;
pub mod tasks;
pub mod tools;

// Re-export commonly used prompts for convenience
pub use agents::{
    EXPLORE_AGENT_PROMPT, GENERAL_AGENT_PROMPT, RESEARCH_AGENT_PROMPT, SUMMARY_AGENT_PROMPT,
    TITLE_AGENT_PROMPT,
};
pub use core::{CORTEX_MAIN_PROMPT, TUI_SYSTEM_PROMPT_TEMPLATE};
pub use tasks::{COMPACTION_PROMPT, SUMMARIZATION_PROMPT};
