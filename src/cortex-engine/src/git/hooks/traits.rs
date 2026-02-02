//! Hook Runner Traits.
//!
//! Traits for implementing custom git hooks.

use async_trait::async_trait;

use crate::error::Result;
use crate::git_info::GitDiff;

use super::config::HookConfig;
use super::results::{HookExecutionResult, HookIssue};

/// Trait for custom hook implementations.
#[async_trait]
pub trait GitHookRunner: Send + Sync {
    /// Run the hook with given arguments.
    async fn run(&self, args: &[&str], config: &HookConfig) -> Result<HookExecutionResult>;
}

/// Pre-commit hook trait.
#[async_trait]
pub trait PreCommitHook: Send + Sync {
    /// Run pre-commit checks.
    async fn check(&self, diff: &GitDiff, config: &HookConfig) -> Result<Vec<HookIssue>>;
}

/// Prepare-commit-msg hook trait.
#[async_trait]
pub trait PrepareCommitMsgHook: Send + Sync {
    /// Generate or modify commit message.
    async fn prepare(&self, diff: &GitDiff, config: &HookConfig) -> Result<String>;
}

/// Commit-msg hook trait.
#[async_trait]
pub trait CommitMsgHook: Send + Sync {
    /// Validate commit message.
    async fn validate(&self, message: &str, config: &HookConfig) -> Result<Vec<HookIssue>>;
}

/// Pre-push hook trait.
#[async_trait]
pub trait PrePushHook: Send + Sync {
    /// Run pre-push checks.
    async fn check(&self, remote: &str, config: &HookConfig) -> Result<Vec<HookIssue>>;
}
