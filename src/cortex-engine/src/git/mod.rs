//! Git integration module for Cortex CLI.
//!
//! This module provides git-related functionality including:
//! - AI-powered git hooks (pre-commit, commit-msg, pre-push, etc.)
//! - Commit message generation
//! - Code review integration
//! - Security scanning for commits
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
//! // List installed hooks
//! for (hook, is_cortex) in manager.list_installed()? {
//!     println!("{}: {}", hook, if is_cortex { "Cortex" } else { "Other" });
//! }
//! # Ok(())
//! # }
//! ```

pub mod hooks;

// Re-export main types
pub use hooks::{
    // Traits for custom hooks
    CommitMsgHook,
    // Hook types
    GitHook,
    // Hook runner
    GitHookRunner,
    HookConfig,
    HookExecutionResult,
    // Issue types
    HookIssue,
    HookManager,
    HookStatus,
    IssueCategory,
    IssueSeverity,
    PatternCheck,
    PreCommitHook,
    PrePushHook,
    PrepareCommitMsgHook,
    // Utilities
    should_ignore_path,
};
