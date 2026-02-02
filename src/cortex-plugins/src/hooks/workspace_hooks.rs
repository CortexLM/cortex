//! Workspace change hooks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::prompt_hooks::ContextDocument;
use super::types::{HookPriority, HookResult};
use crate::Result;

/// Input for workspace.changed hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceChangedInput {
    /// Session ID
    pub session_id: String,
    /// Previous working directory
    pub old_cwd: Option<PathBuf>,
    /// New working directory
    pub new_cwd: PathBuf,
    /// Detected project type
    pub project_type: Option<ProjectType>,
}

/// Project types that can be detected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectType {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Java,
    CSharp,
    Ruby,
    Php,
    Swift,
    Kotlin,
    Other { name: String },
}

/// Output for workspace.changed hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceChangedOutput {
    /// Context to add based on workspace
    pub context: Vec<ContextDocument>,
    /// Configuration overrides for this workspace
    pub config_overrides: HashMap<String, serde_json::Value>,
    /// Suggested agent
    pub suggested_agent: Option<String>,
    /// Hook result
    pub result: HookResult,
}

impl WorkspaceChangedOutput {
    pub fn new() -> Self {
        Self {
            context: Vec::new(),
            config_overrides: HashMap::new(),
            suggested_agent: None,
            result: HookResult::Continue,
        }
    }
}

impl Default for WorkspaceChangedOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Handler for workspace.changed hook.
#[async_trait]
pub trait WorkspaceChangedHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &WorkspaceChangedInput,
        output: &mut WorkspaceChangedOutput,
    ) -> Result<()>;
}
