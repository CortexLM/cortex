//! Configuration change hooks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::types::{HookPriority, HookResult};
use crate::Result;

/// Input for config.changed hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChangedInput {
    /// Configuration key that changed
    pub key: String,
    /// Old value
    pub old_value: Option<serde_json::Value>,
    /// New value
    pub new_value: serde_json::Value,
    /// Source of the change
    pub source: ConfigChangeSource,
}

/// Config change sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigChangeSource {
    /// User changed via command
    User,
    /// Changed via config file
    File,
    /// Changed by a plugin
    Plugin { plugin_id: String },
    /// System/auto change
    System,
}

/// Output for config.changed hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChangedOutput {
    /// Additional actions to take
    pub actions: Vec<ConfigChangeAction>,
    /// Hook result
    pub result: HookResult,
}

impl ConfigChangedOutput {
    pub fn new() -> Self {
        Self {
            actions: Vec::new(),
            result: HookResult::Continue,
        }
    }
}

impl Default for ConfigChangedOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Actions triggered by config changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigChangeAction {
    /// Reload component
    Reload { component: String },
    /// Show notification
    Notify { message: String },
    /// Restart required
    RestartRequired { reason: String },
}

/// Handler for config.changed hook.
#[async_trait]
pub trait ConfigChangedHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Get config key patterns this hook watches (None = all).
    fn patterns(&self) -> Option<Vec<String>> {
        None
    }

    async fn execute(
        &self,
        input: &ConfigChangedInput,
        output: &mut ConfigChangedOutput,
    ) -> Result<()>;
}
