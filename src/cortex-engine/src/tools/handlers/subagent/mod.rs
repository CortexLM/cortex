//! Subagent execution system for delegating tasks to specialized agents.
//!
//! This module provides the infrastructure for spawning and managing subagents
//! that can execute complex, multi-step tasks autonomously.

mod executor;
mod progress;
mod result;
mod types;

pub use executor::SubagentExecutor;
pub use progress::{ProgressEvent, SubagentProgress};
pub use result::SubagentResult;
pub use types::{SubagentConfig, SubagentSession, SubagentStatus, SubagentType};
