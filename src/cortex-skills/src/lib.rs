#![allow(
    clippy::too_many_arguments,
    clippy::field_reassign_with_default,
    clippy::ptr_arg
)]
//! Skills system for Cortex CLI.
//!
//! This crate provides a complete skills framework with:
//! - SKILL.toml metadata parsing
//! - Context isolation for skill execution
//! - Hot reloading without restart
//! - Wildcard pattern matching for skills
//! - Auto-allowed skills support
//!
//! # Skill File Format
//!
//! Skills are defined in directories with `SKILL.toml` and `skill.md`:
//!
//! ```toml
//! # SKILL.toml
//! name = "code-review"
//! description = "Expert code reviewer"
//! version = "1.0.0"
//!
//! # Configuration
//! model = "claude-sonnet-4"
//! reasoning_effort = "high"
//! auto_allowed = true
//! timeout = 300
//!
//! # Tools
//! allowed_tools = ["Read", "Grep", "Glob", "LS"]
//! denied_tools = ["Execute", "Create", "Edit"]
//!
//! # Metadata
//! author = "Cortex Team"
//! icon = "🔍"
//! tags = ["review", "security", "quality"]
//! ```
//!
//! ```markdown
//! <!-- skill.md -->
//! You are an expert code reviewer...
//! ```
//!
//! # Search Paths
//!
//! Skills are loaded from:
//! 1. `.cortex/skills/` (project-local)
//! 2. `~/.config/cortex/skills/` (global)
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_skills::{SkillManager, SkillContext};
//!
//! let mut manager = SkillManager::new(vec![skill_dir]);
//! manager.load_all().await?;
//!
//! if let Some(skill) = manager.get("code-review").await {
//!     let ctx = SkillContext::new(skill);
//!     // Execute skill with context
//! }
//! ```

mod context;
mod error;
mod manager;
mod parser;
mod skill;
mod validation;
mod watcher;

pub use context::SkillContext;
pub use error::{SkillError, SkillResult};
pub use manager::{ReloadEvent, SkillManager};
pub use parser::{SkillToml, parse_skill_toml};
pub use skill::{ReasoningEffort, Skill, SkillConfig, SkillMetadata};
pub use validation::{SkillValidator, ValidationError, ValidationResult};
pub use watcher::SkillWatcher;

/// Re-export common types for convenience.
pub mod prelude {
    pub use crate::{
        ReasoningEffort, Skill, SkillConfig, SkillContext, SkillError, SkillManager, SkillMetadata,
        SkillResult, SkillValidator,
    };
}
