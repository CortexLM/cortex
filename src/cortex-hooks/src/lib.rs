//! Hook system for Cortex CLI.
//!
//! Provides hooks for:
//! - File editing (formatters)
//! - Session completion
//! - Custom actions
//! - Tool use events (PreToolUse, PostToolUse, PostToolUseFailure)
//! - Session lifecycle events (SessionStart, SessionEnd)
//! - Subagent events (SubagentStart, SubagentStop)
//! - LLM-based prompt hooks for contextual decisions
//!
//! # Async Hooks
//!
//! Hooks can be configured for asynchronous (fire-and-forget) execution
//! using the `async_exec()` builder method. Async hooks return immediately
//! without blocking the main execution flow.
//!
//! # Once Hooks
//!
//! Hooks can be configured to execute only once per session using the
//! `once()` builder method. Subsequent executions will be skipped.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_hooks::{Hook, HookExecutor, HookType, HookContext};
//!
//! let executor = HookExecutor::new();
//!
//! // Register a synchronous hook
//! let hook = Hook::new("format-rs", HookType::FileEdited, vec!["rustfmt", "{file}"])
//!     .with_pattern("*.rs");
//! executor.register(hook).await;
//!
//! // Register an async hook that runs once
//! let async_hook = Hook::new("log-session", HookType::SessionStart, vec!["logger", "Session started"])
//!     .async_exec()
//!     .once();
//! executor.register(async_hook).await;
//!
//! // Run hooks
//! let results = executor.on_file_edited("src/main.rs").await;
//! ```

pub mod config;
pub mod executor;
pub mod formatter;
pub mod hook;
pub mod prompt_hook;
pub mod session_env;

pub use config::HookConfig;
pub use executor::HookExecutor;
pub use formatter::{Formatter, FormatterConfig, BUILTIN_FORMATTERS};
pub use hook::{Hook, HookContext, HookResult, HookStatus, HookType};
pub use prompt_hook::{
    LlmClient, PromptDecision, PromptHook, PromptHookError, PromptHookExecutor, PromptHookResponse,
};
pub use session_env::{delete_session_env, SessionEnvFile};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum HookError {
    #[error("Hook execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Hook not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Hook timeout")]
    Timeout,
    #[error("LLM error: {0}")]
    LlmError(String),
    #[error("Invalid prompt response: {0}")]
    InvalidPromptResponse(String),
}

pub type Result<T> = std::result::Result<T, HookError>;
