//! Agent control for multi-agent operations.

use super::{
    AgentRole, AgentStatus, CollabError, Guards, Result, SessionSource, SubAgentSource, ThreadId,
    ThreadManagerState, thread_manager::AgentConfig,
};
use std::sync::{Arc, Weak};
use tokio::sync::watch;

/// Control-plane handle for multi-agent operations.
/// Shared per "user session" so that guards are scoped per session.
#[derive(Clone)]
pub struct AgentControl {
    /// Weak handle back to the global thread registry/state.
    /// Weak to avoid reference cycles.
    manager: Weak<ThreadManagerState>,

    /// Guards for this session.
    guards: Arc<Guards>,
}

impl AgentControl {
    /// Create a new agent control with a thread manager.
    pub fn new(manager: Weak<ThreadManagerState>) -> Self {
        Self {
            manager,
            guards: Arc::new(Guards::new()),
        }
    }

    /// Create with custom guards.
    pub fn with_guards(manager: Weak<ThreadManagerState>, guards: Arc<Guards>) -> Self {
        Self { manager, guards }
    }

    /// Get the thread manager state.
    fn get_manager(&self) -> Result<Arc<ThreadManagerState>> {
        self.manager
            .upgrade()
            .ok_or_else(|| CollabError::Internal("Thread manager dropped".to_string()))
    }

    /// Spawn a new agent thread and submit the initial prompt.
    pub async fn spawn_agent(
        &self,
        config: AgentConfig,
        prompt: String,
        session_source: Option<SessionSource>,
    ) -> Result<ThreadId> {
        if prompt.trim().is_empty() {
            return Err(CollabError::EmptyMessage);
        }

        let manager_state = self.get_manager()?;

        // Reserve a spawn slot
        let reservation = self
            .guards
            .reserve_spawn_slot()
            .await
            .ok_or(CollabError::SpawnLimitExceeded)?;

        let id = ThreadId::new();
        let source = session_source.unwrap_or(config.session_source.clone());

        let thread = super::thread_manager::AgentThread::new(id, config.role, source);

        // Set status to running
        thread.set_status(AgentStatus::Running);

        // Insert thread
        manager_state.insert_thread(thread).await;

        // Commit the reservation
        reservation.commit(id).await;

        Ok(id)
    }

    /// Send a user prompt to an existing agent thread.
    pub async fn send_prompt(&self, agent_id: ThreadId, prompt: String) -> Result<String> {
        if prompt.trim().is_empty() {
            return Err(CollabError::EmptyMessage);
        }

        let manager_state = self.get_manager()?;

        // Verify thread exists
        let thread = manager_state
            .get_thread(agent_id)
            .await
            .ok_or(CollabError::AgentNotFound(agent_id))?;

        // Check if thread is still active
        let status = thread.status();
        if status.is_final() {
            return Err(CollabError::Internal(format!(
                "Agent {} is in final state: {}",
                agent_id, status
            )));
        }

        // Generate a submission ID
        let submission_id = uuid::Uuid::new_v4().to_string();

        // In a real implementation, this would send the prompt to the agent's input channel
        Ok(submission_id)
    }

    /// Interrupt the current task for an existing agent thread.
    pub async fn interrupt_agent(&self, agent_id: ThreadId) -> Result<String> {
        let manager_state = self.get_manager()?;

        let thread = manager_state
            .get_thread(agent_id)
            .await
            .ok_or(CollabError::AgentNotFound(agent_id))?;

        // Check if interruptible
        let status = thread.status();
        if status.is_final() {
            return Err(CollabError::Internal(format!(
                "Agent {} is in final state and cannot be interrupted",
                agent_id
            )));
        }

        // In a real implementation, this would send an interrupt signal
        let interrupt_id = uuid::Uuid::new_v4().to_string();
        Ok(interrupt_id)
    }

    /// Submit a shutdown request to an existing agent thread.
    pub async fn shutdown_agent(&self, agent_id: ThreadId) -> Result<String> {
        let manager_state = self.get_manager()?;

        if let Some(thread) = manager_state.get_thread(agent_id).await {
            thread.set_status(AgentStatus::Shutdown);
            self.guards.release_spawned_thread(agent_id).await;
        }

        let shutdown_id = uuid::Uuid::new_v4().to_string();
        Ok(shutdown_id)
    }

    /// Fetch the last known status for agent_id.
    pub async fn get_status(&self, agent_id: ThreadId) -> AgentStatus {
        match self.get_manager() {
            Ok(manager_state) => match manager_state.get_thread(agent_id).await {
                Some(thread) => thread.status(),
                None => AgentStatus::NotFound,
            },
            Err(_) => AgentStatus::NotFound,
        }
    }

    /// Subscribe to status updates for agent_id (watch channel).
    pub async fn subscribe_status(
        &self,
        agent_id: ThreadId,
    ) -> Result<watch::Receiver<AgentStatus>> {
        let manager_state = self.get_manager()?;

        match manager_state.get_thread(agent_id).await {
            Some(thread) => Ok(thread.subscribe_status()),
            None => Err(CollabError::AgentNotFound(agent_id)),
        }
    }

    /// Get the guards for this control.
    pub fn guards(&self) -> &Arc<Guards> {
        &self.guards
    }

    /// Build an agent config for spawning.
    pub fn build_spawn_config(
        &self,
        role: AgentRole,
        prompt: impl Into<String>,
        parent_thread_id: ThreadId,
        depth: i32,
    ) -> AgentConfig {
        let source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id,
            depth,
        });

        AgentConfig::new(role, prompt, source)
    }
}

impl Default for AgentControl {
    fn default() -> Self {
        // Create with a dummy manager - should be replaced in actual use
        Self {
            manager: Weak::new(),
            guards: Arc::new(Guards::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_control_spawn() {
        let state = Arc::new(ThreadManagerState::new());
        let control = AgentControl::new(Arc::downgrade(&state));

        let config = AgentConfig::new(AgentRole::General, "Test task", SessionSource::User);

        let id = control
            .spawn_agent(config, "Test prompt".to_string(), None)
            .await
            .unwrap();

        let status = control.get_status(id).await;
        assert!(matches!(status, AgentStatus::Running));
    }

    #[tokio::test]
    async fn test_agent_control_empty_message() {
        let state = Arc::new(ThreadManagerState::new());
        let control = AgentControl::new(Arc::downgrade(&state));

        let config = AgentConfig::new(AgentRole::General, "Test task", SessionSource::User);

        let result = control.spawn_agent(config, "".to_string(), None).await;

        assert!(matches!(result, Err(CollabError::EmptyMessage)));
    }
}
