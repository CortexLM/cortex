//! System prompt management for Cortex agents.
//!
//! This module handles loading and processing system prompts with template variable support.

mod system_prompt;

pub use system_prompt::{SystemPromptError, load_system_prompt, replace_variables};
