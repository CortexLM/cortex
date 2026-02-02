//! Orchestration State Tracking.
//!
//! This module provides functionality for tracking the progress of orchestrated
//! agent tasks and maintaining state files that can be used to resume or
//! monitor multi-agent workflows.
//!
//! # State File Format
//!
//! State files are stored in `.cortex/orchestration/` and use YAML format:
//!
//! ```yaml
//! version: 1
//! project: my-project
//! created_at: 2024-01-15T10:00:00Z
//! updated_at: 2024-01-15T10:30:00Z
//! status: in_progress
//! agents:
//!   - id: agent1
//!     status: completed
//!     started_at: 2024-01-15T10:00:00Z
//!     completed_at: 2024-01-15T10:15:00Z
//!     tasks_completed: 5
//! progress:
//!   total_tasks: 10
//!   completed_tasks: 5
//!   current_agent: agent2
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::atomic::{AtomicWriteError, atomic_write_str};

/// Errors that can occur during state operations.
#[derive(Debug, Error)]
pub enum StateError {
    /// Failed to read state file.
    #[error("Failed to read state file '{path}': {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to write state file.
    #[error("Failed to write state file: {0}")]
    WriteFile(#[from] AtomicWriteError),

    /// Failed to parse state file.
    #[error("Failed to parse state file: {0}")]
    Parse(String),

    /// Failed to serialize state.
    #[error("Failed to serialize state: {0}")]
    Serialize(String),

    /// State directory creation failed.
    #[error("Failed to create state directory '{dir}': {source}")]
    CreateDir {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Overall status of the orchestration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationStatus {
    /// Not yet started.
    #[default]
    Pending,
    /// Currently in progress.
    InProgress,
    /// Completed successfully.
    Completed,
    /// Failed with errors.
    Failed,
    /// Paused/suspended.
    Paused,
}

impl OrchestrationStatus {
    /// Check if the orchestration is terminal (completed or failed).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    /// Check if the orchestration is active.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::InProgress)
    }
}

impl std::fmt::Display for OrchestrationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Paused => write!(f, "paused"),
        }
    }
}

/// Status of an individual agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Waiting to start.
    #[default]
    Pending,
    /// Currently running.
    Running,
    /// Completed successfully.
    Completed,
    /// Failed with errors.
    Failed,
    /// Blocked by dependencies.
    Blocked,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Blocked => write!(f, "blocked"),
        }
    }
}

/// Information about an agent's progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// Unique agent identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Current status.
    pub status: AgentStatus,
    /// When the agent was started.
    pub started_at: Option<String>,
    /// When the agent completed.
    pub completed_at: Option<String>,
    /// Number of tasks completed.
    pub tasks_completed: usize,
    /// Total tasks assigned.
    pub total_tasks: usize,
    /// Error message if failed.
    pub error: Option<String>,
    /// Custom metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl AgentState {
    /// Create a new agent state.
    pub fn new(id: impl Into<String>, name: impl Into<String>, total_tasks: usize) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status: AgentStatus::Pending,
            started_at: None,
            completed_at: None,
            tasks_completed: 0,
            total_tasks,
            error: None,
            metadata: HashMap::new(),
        }
    }

    /// Mark the agent as running.
    pub fn start(&mut self) {
        self.status = AgentStatus::Running;
        self.started_at = Some(chrono::Utc::now().to_rfc3339());
    }

    /// Mark the agent as completed.
    pub fn complete(&mut self) {
        self.status = AgentStatus::Completed;
        self.completed_at = Some(chrono::Utc::now().to_rfc3339());
        self.tasks_completed = self.total_tasks;
    }

    /// Mark the agent as failed.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.status = AgentStatus::Failed;
        self.completed_at = Some(chrono::Utc::now().to_rfc3339());
        self.error = Some(error.into());
    }

    /// Update progress.
    pub fn update_progress(&mut self, tasks_completed: usize) {
        self.tasks_completed = tasks_completed.min(self.total_tasks);
    }

    /// Get completion percentage.
    pub fn progress_percent(&self) -> f32 {
        if self.total_tasks == 0 {
            return 100.0;
        }
        (self.tasks_completed as f32 / self.total_tasks as f32) * 100.0
    }
}

/// Overall progress information.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProgressInfo {
    /// Total number of tasks across all agents.
    pub total_tasks: usize,
    /// Number of completed tasks.
    pub completed_tasks: usize,
    /// Currently active agent ID.
    pub current_agent: Option<String>,
    /// Estimated time remaining (optional).
    pub eta_seconds: Option<u64>,
}

impl ProgressInfo {
    /// Get completion percentage.
    pub fn percent(&self) -> f32 {
        if self.total_tasks == 0 {
            return 100.0;
        }
        (self.completed_tasks as f32 / self.total_tasks as f32) * 100.0
    }
}

/// The complete orchestration state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationState {
    /// State file format version.
    pub version: u32,
    /// Project name.
    pub project: String,
    /// When the orchestration was created.
    pub created_at: String,
    /// When the state was last updated.
    pub updated_at: String,
    /// Overall orchestration status.
    pub status: OrchestrationStatus,
    /// Agent states.
    pub agents: Vec<AgentState>,
    /// Overall progress.
    pub progress: ProgressInfo,
    /// Optional description.
    pub description: Option<String>,
    /// Custom metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl OrchestrationState {
    /// Current state file format version.
    pub const VERSION: u32 = 1;

    /// Create a new orchestration state.
    pub fn new(project: impl Into<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();

        Self {
            version: Self::VERSION,
            project: project.into(),
            created_at: now.clone(),
            updated_at: now,
            status: OrchestrationStatus::Pending,
            agents: Vec::new(),
            progress: ProgressInfo::default(),
            description: None,
            metadata: HashMap::new(),
        }
    }

    /// Add an agent to the orchestration.
    pub fn add_agent(&mut self, agent: AgentState) {
        self.progress.total_tasks += agent.total_tasks;
        self.agents.push(agent);
        self.touch();
    }

    /// Get an agent by ID.
    pub fn get_agent(&self, id: &str) -> Option<&AgentState> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// Get a mutable reference to an agent by ID.
    pub fn get_agent_mut(&mut self, id: &str) -> Option<&mut AgentState> {
        self.agents.iter_mut().find(|a| a.id == id)
    }

    /// Start the orchestration.
    pub fn start(&mut self) {
        self.status = OrchestrationStatus::InProgress;
        self.touch();
    }

    /// Complete the orchestration.
    pub fn complete(&mut self) {
        self.status = OrchestrationStatus::Completed;
        self.touch();
    }

    /// Mark the orchestration as failed.
    pub fn fail(&mut self) {
        self.status = OrchestrationStatus::Failed;
        self.touch();
    }

    /// Update the last modified timestamp.
    pub fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Recalculate progress from agent states.
    pub fn recalculate_progress(&mut self) {
        self.progress.total_tasks = self.agents.iter().map(|a| a.total_tasks).sum();
        self.progress.completed_tasks = self.agents.iter().map(|a| a.tasks_completed).sum();
        self.progress.current_agent = self
            .agents
            .iter()
            .find(|a| a.status == AgentStatus::Running)
            .map(|a| a.id.clone());
    }

    /// Load state from a file.
    pub fn load(path: &Path) -> Result<Self, StateError> {
        let content = std::fs::read_to_string(path).map_err(|source| StateError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;

        serde_yaml::from_str(&content).map_err(|e| StateError::Parse(e.to_string()))
    }

    /// Save state to a file atomically.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|source| StateError::CreateDir {
                dir: parent.to_path_buf(),
                source,
            })?;
        }

        let content =
            serde_yaml::to_string(self).map_err(|e| StateError::Serialize(e.to_string()))?;

        atomic_write_str(path, &content)?;

        Ok(())
    }

    /// Get the default state file path for a project.
    pub fn default_path(project_root: &Path) -> PathBuf {
        project_root.join(".cortex/orchestration/state.yaml")
    }

    /// Load or create a new state for the project.
    pub fn load_or_create(project_root: &Path, project_name: &str) -> Result<Self, StateError> {
        let path = Self::default_path(project_root);

        if path.exists() {
            Self::load(&path)
        } else {
            Ok(Self::new(project_name))
        }
    }
}

/// Manager for orchestration state with auto-save functionality.
pub struct StateManager {
    /// The current state.
    state: OrchestrationState,
    /// Path to save the state.
    path: PathBuf,
    /// Whether auto-save is enabled.
    auto_save: bool,
}

impl StateManager {
    /// Create a new state manager.
    pub fn new(project_root: &Path, project_name: &str) -> Result<Self, StateError> {
        let path = OrchestrationState::default_path(project_root);
        let state = OrchestrationState::load_or_create(project_root, project_name)?;

        Ok(Self {
            state,
            path,
            auto_save: true,
        })
    }

    /// Get a reference to the current state.
    pub fn state(&self) -> &OrchestrationState {
        &self.state
    }

    /// Get a mutable reference to the state.
    pub fn state_mut(&mut self) -> &mut OrchestrationState {
        &mut self.state
    }

    /// Enable or disable auto-save.
    pub fn set_auto_save(&mut self, enabled: bool) {
        self.auto_save = enabled;
    }

    /// Save the current state.
    pub fn save(&self) -> Result<(), StateError> {
        self.state.save(&self.path)
    }

    /// Save if auto-save is enabled.
    pub fn maybe_save(&self) -> Result<(), StateError> {
        if self.auto_save { self.save() } else { Ok(()) }
    }

    /// Add an agent and save.
    pub fn add_agent(&mut self, agent: AgentState) -> Result<(), StateError> {
        self.state.add_agent(agent);
        self.maybe_save()
    }

    /// Start an agent.
    pub fn start_agent(&mut self, id: &str) -> Result<(), StateError> {
        if let Some(agent) = self.state.get_agent_mut(id) {
            agent.start();
            self.state.recalculate_progress();
            self.state.touch();
            self.maybe_save()?;
        }
        Ok(())
    }

    /// Complete an agent.
    pub fn complete_agent(&mut self, id: &str) -> Result<(), StateError> {
        if let Some(agent) = self.state.get_agent_mut(id) {
            agent.complete();
            self.state.recalculate_progress();
            self.state.touch();
            self.maybe_save()?;
        }
        Ok(())
    }

    /// Update agent progress.
    pub fn update_agent_progress(
        &mut self,
        id: &str,
        tasks_completed: usize,
    ) -> Result<(), StateError> {
        if let Some(agent) = self.state.get_agent_mut(id) {
            agent.update_progress(tasks_completed);
            self.state.recalculate_progress();
            self.state.touch();
            self.maybe_save()?;
        }
        Ok(())
    }

    /// Fail an agent.
    pub fn fail_agent(&mut self, id: &str, error: impl Into<String>) -> Result<(), StateError> {
        if let Some(agent) = self.state.get_agent_mut(id) {
            agent.fail(error);
            self.state.recalculate_progress();
            self.state.touch();
            self.maybe_save()?;
        }
        Ok(())
    }

    /// Start the orchestration.
    pub fn start(&mut self) -> Result<(), StateError> {
        self.state.start();
        self.maybe_save()
    }

    /// Complete the orchestration.
    pub fn complete(&mut self) -> Result<(), StateError> {
        self.state.complete();
        self.maybe_save()
    }

    /// Fail the orchestration.
    pub fn fail(&mut self) -> Result<(), StateError> {
        self.state.fail();
        self.maybe_save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_state() {
        let state = OrchestrationState::new("test-project");

        assert_eq!(state.project, "test-project");
        assert_eq!(state.version, OrchestrationState::VERSION);
        assert_eq!(state.status, OrchestrationStatus::Pending);
        assert!(state.agents.is_empty());
    }

    #[test]
    fn test_add_agent() {
        let mut state = OrchestrationState::new("test-project");

        let agent = AgentState::new("agent1", "Agent 1", 5);
        state.add_agent(agent);

        assert_eq!(state.agents.len(), 1);
        assert_eq!(state.progress.total_tasks, 5);
    }

    #[test]
    fn test_agent_lifecycle() {
        let mut agent = AgentState::new("agent1", "Agent 1", 10);

        assert_eq!(agent.status, AgentStatus::Pending);
        assert!(agent.started_at.is_none());

        agent.start();
        assert_eq!(agent.status, AgentStatus::Running);
        assert!(agent.started_at.is_some());

        agent.update_progress(5);
        assert_eq!(agent.tasks_completed, 5);
        assert_eq!(agent.progress_percent(), 50.0);

        agent.complete();
        assert_eq!(agent.status, AgentStatus::Completed);
        assert!(agent.completed_at.is_some());
    }

    #[test]
    fn test_save_and_load() {
        let dir = TempDir::new().unwrap();
        let state_path = dir.path().join(".cortex/orchestration/state.yaml");

        let mut state = OrchestrationState::new("test-project");
        state.add_agent(AgentState::new("agent1", "Agent 1", 5));
        state.start();

        state.save(&state_path).unwrap();
        assert!(state_path.exists());

        let loaded = OrchestrationState::load(&state_path).unwrap();
        assert_eq!(loaded.project, "test-project");
        assert_eq!(loaded.agents.len(), 1);
        assert_eq!(loaded.status, OrchestrationStatus::InProgress);
    }

    #[test]
    fn test_state_manager() {
        let dir = TempDir::new().unwrap();

        let mut manager = StateManager::new(dir.path(), "test-project").unwrap();

        manager
            .add_agent(AgentState::new("agent1", "Agent 1", 5))
            .unwrap();
        manager.start().unwrap();
        manager.start_agent("agent1").unwrap();
        manager.update_agent_progress("agent1", 3).unwrap();

        let state = manager.state();
        assert_eq!(state.status, OrchestrationStatus::InProgress);
        assert_eq!(state.progress.completed_tasks, 3);
        assert_eq!(state.progress.current_agent, Some("agent1".to_string()));
    }

    #[test]
    fn test_recalculate_progress() {
        let mut state = OrchestrationState::new("test-project");

        state.add_agent(AgentState::new("agent1", "Agent 1", 5));
        state.add_agent(AgentState::new("agent2", "Agent 2", 10));

        if let Some(agent) = state.get_agent_mut("agent1") {
            agent.update_progress(3);
        }
        if let Some(agent) = state.get_agent_mut("agent2") {
            agent.update_progress(7);
        }

        state.recalculate_progress();

        assert_eq!(state.progress.total_tasks, 15);
        assert_eq!(state.progress.completed_tasks, 10);
    }
}
