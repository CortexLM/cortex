//! Focus change hooks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::types::{HookPriority, HookResult};
use crate::Result;

/// Input for focus.change hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusChangeInput {
    /// Session ID
    pub session_id: String,
    /// Whether the app gained or lost focus
    pub gained: bool,
    /// Previous focus state duration in seconds
    pub previous_state_duration: u64,
}

/// Output for focus.change hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusChangeOutput {
    /// Actions to take
    pub actions: Vec<FocusAction>,
    /// Hook result
    pub result: HookResult,
}

impl FocusChangeOutput {
    pub fn new() -> Self {
        Self {
            actions: Vec::new(),
            result: HookResult::Continue,
        }
    }
}

impl Default for FocusChangeOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Focus change actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FocusAction {
    /// Refresh workspace
    RefreshWorkspace,
    /// Check for file changes
    CheckFileChanges,
    /// Resume paused operation
    Resume,
    /// Pause operation
    Pause,
    /// Custom action
    Custom {
        action: String,
        data: serde_json::Value,
    },
}

/// Handler for focus.change hook.
#[async_trait]
pub trait FocusChangeHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(&self, input: &FocusChangeInput, output: &mut FocusChangeOutput)
    -> Result<()>;
}
