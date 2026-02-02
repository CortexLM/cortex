//! Git Hooks Integration for Cortex CLI.
//!
//! This module provides AI-powered git hooks for:
//! - Pre-commit code review and validation
//! - Commit message generation and validation  
//! - Pre-push safety checks
//! - Security scanning for secrets
//!
//! # Example
//!
//! ```no_run
//! use cortex_engine::git::{HookManager, GitHook, HookConfig};
//!
//! # async fn example() -> anyhow::Result<()> {
//! // Create hook manager for current repository
//! let manager = HookManager::new(".")?;
//!
//! // Install pre-commit hook with AI review
//! let config = HookConfig::default()
//!     .with_ai_review(true)
//!     .with_secrets_check(true)
//!     .with_blocking(true);
//! manager.install_hook(GitHook::PreCommit, config).await?;
//!
//! // Run hook manually
//! let result = manager.run_hook(GitHook::PreCommit, &[]).await?;
//! if result.passed {
//!     println!("Hook passed!");
//! }
//! # Ok(())
//! # }
//! ```

mod config;
mod manager;
mod results;
mod scanning;
mod traits;
mod types;
mod utils;

// Re-export all public types for backwards compatibility
pub use config::{HookConfig, PatternCheck};
pub use manager::HookManager;
pub use results::{HookExecutionResult, HookIssue, HookStatus, IssueCategory, IssueSeverity};
pub use traits::{CommitMsgHook, GitHookRunner, PreCommitHook, PrePushHook, PrepareCommitMsgHook};
pub use types::GitHook;
pub use utils::should_ignore_path;
