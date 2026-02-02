//! Thread manager for multi-agent sessions.

use super::{AgentRole, AgentStatus, Guards, SessionSource};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, watch};
use uuid::Uuid;

/// Unique identifier for an agent thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(Uuid);

impl ThreadId {
    /// Create a new random thread ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create from a UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Parse from string.
    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }

    /// Get the underlying UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ThreadId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for ThreadId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Configuration for spawning a new agent thread.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Role of the agent.
    pub role: AgentRole,

    /// Initial prompt/message.
    pub initial_prompt: String,

    /// Session source.
    pub session_source: SessionSource,

    /// Base instructions to use.
    pub base_instructions: Option<String>,

    /// Model to use (if different from default).
    pub model: Option<String>,

    /// Maximum tokens for response.
    pub max_tokens: Option<usize>,

    /// Temperature for generation.
    pub temperature: Option<f32>,
}

impl AgentConfig {
    /// Create a new agent config with required fields.
    pub fn new(role: AgentRole, prompt: impl Into<String>, source: SessionSource) -> Self {
        Self {
            role,
            initial_prompt: prompt.into(),
            session_source: source,
            base_instructions: None,
            model: None,
            max_tokens: None,
            temperature: None,
        }
    }

    /// Set base instructions.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.base_instructions = Some(instructions.into());
        self
    }

    /// Set model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

/// An agent thread in the collaboration system.
#[derive(Debug)]
pub struct AgentThread {
    /// Thread ID.
    pub id: ThreadId,

    /// Agent role.
    pub role: AgentRole,

    /// Session source.
    pub source: SessionSource,

    /// Status sender for updates.
    status_tx: watch::Sender<AgentStatus>,

    /// Status receiver for subscriptions.
    status_rx: watch::Receiver<AgentStatus>,

    /// Creation timestamp.
    pub created_at: std::time::Instant,
}

impl AgentThread {
    /// Create a new agent thread.
    pub fn new(id: ThreadId, role: AgentRole, source: SessionSource) -> Self {
        let (status_tx, status_rx) = watch::channel(AgentStatus::PendingInit);
        Self {
            id,
            role,
            source,
            status_tx,
            status_rx,
            created_at: std::time::Instant::now(),
        }
    }

    /// Get the current status.
    pub fn status(&self) -> AgentStatus {
        self.status_rx.borrow().clone()
    }

    /// Update the status.
    pub fn set_status(&self, status: AgentStatus) {
        let _ = self.status_tx.send(status);
    }

    /// Subscribe to status updates.
    pub fn subscribe_status(&self) -> watch::Receiver<AgentStatus> {
        self.status_rx.clone()
    }
}

/// Shared state for the thread manager.
pub struct ThreadManagerState {
    /// Active threads.
    threads: RwLock<HashMap<ThreadId, Arc<AgentThread>>>,

    /// Broadcast channel for thread creation events.
    thread_created_tx: broadcast::Sender<ThreadId>,

    /// Guards for spawn limits.
    guards: Arc<Guards>,
}

impl ThreadManagerState {
    /// Create new thread manager state.
    pub fn new() -> Self {
        let (thread_created_tx, _) = broadcast::channel(16);
        Self {
            threads: RwLock::new(HashMap::new()),
            thread_created_tx,
            guards: Arc::new(Guards::new()),
        }
    }

    /// Create with custom guards.
    pub fn with_guards(guards: Arc<Guards>) -> Self {
        let (thread_created_tx, _) = broadcast::channel(16);
        Self {
            threads: RwLock::new(HashMap::new()),
            thread_created_tx,
            guards,
        }
    }

    /// Get a thread by ID.
    pub async fn get_thread(&self, id: ThreadId) -> Option<Arc<AgentThread>> {
        let threads = self.threads.read().await;
        threads.get(&id).cloned()
    }

    /// Insert a new thread.
    pub async fn insert_thread(&self, thread: AgentThread) {
        let id = thread.id;
        let thread = Arc::new(thread);
        let mut threads = self.threads.write().await;
        threads.insert(id, thread);
        let _ = self.thread_created_tx.send(id);
    }

    /// Remove a thread.
    pub async fn remove_thread(&self, id: ThreadId) -> Option<Arc<AgentThread>> {
        let mut threads = self.threads.write().await;
        threads.remove(&id)
    }

    /// List all thread IDs.
    pub async fn list_threads(&self) -> Vec<ThreadId> {
        let threads = self.threads.read().await;
        threads.keys().copied().collect()
    }

    /// Get guards reference.
    pub fn guards(&self) -> &Arc<Guards> {
        &self.guards
    }

    /// Subscribe to thread creation events.
    pub fn subscribe_thread_created(&self) -> broadcast::Receiver<ThreadId> {
        self.thread_created_tx.subscribe()
    }
}

impl Default for ThreadManagerState {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread manager for multi-agent orchestration.
pub struct ThreadManager {
    state: Arc<ThreadManagerState>,
}

impl ThreadManager {
    /// Create a new thread manager.
    pub fn new() -> Self {
        Self {
            state: Arc::new(ThreadManagerState::new()),
        }
    }

    /// Create with custom state.
    pub fn with_state(state: Arc<ThreadManagerState>) -> Self {
        Self { state }
    }

    /// Get the shared state.
    pub fn state(&self) -> &Arc<ThreadManagerState> {
        &self.state
    }

    /// Create a new agent thread.
    pub async fn create_thread(&self, config: AgentConfig) -> super::Result<ThreadId> {
        // Reserve a spawn slot
        let reservation = self
            .state
            .guards
            .reserve_spawn_slot()
            .await
            .ok_or(super::CollabError::SpawnLimitExceeded)?;

        let id = ThreadId::new();
        let thread = AgentThread::new(id, config.role, config.session_source);

        // Insert thread
        self.state.insert_thread(thread).await;

        // Commit the reservation
        reservation.commit(id).await;

        Ok(id)
    }

    /// Get a thread by ID.
    pub async fn get_thread(&self, id: ThreadId) -> Option<Arc<AgentThread>> {
        self.state.get_thread(id).await
    }

    /// Get the status of a thread.
    pub async fn get_status(&self, id: ThreadId) -> AgentStatus {
        match self.state.get_thread(id).await {
            Some(thread) => thread.status(),
            None => AgentStatus::NotFound,
        }
    }

    /// Subscribe to status updates for a thread.
    pub async fn subscribe_status(
        &self,
        id: ThreadId,
    ) -> super::Result<watch::Receiver<AgentStatus>> {
        match self.state.get_thread(id).await {
            Some(thread) => Ok(thread.subscribe_status()),
            None => Err(super::CollabError::AgentNotFound(id)),
        }
    }

    /// Update the status of a thread.
    pub async fn set_status(&self, id: ThreadId, status: AgentStatus) -> super::Result<()> {
        match self.state.get_thread(id).await {
            Some(thread) => {
                thread.set_status(status);
                Ok(())
            }
            None => Err(super::CollabError::AgentNotFound(id)),
        }
    }

    /// Shutdown a thread.
    pub async fn shutdown_thread(&self, id: ThreadId) -> super::Result<()> {
        if let Some(thread) = self.state.get_thread(id).await {
            thread.set_status(AgentStatus::Shutdown);
            self.state.guards.release_spawned_thread(id).await;
            Ok(())
        } else {
            Err(super::CollabError::AgentNotFound(id))
        }
    }

    /// List all active threads.
    pub async fn list_threads(&self) -> Vec<ThreadId> {
        self.state.list_threads().await
    }

    /// Subscribe to thread creation events.
    pub fn subscribe_thread_created(&self) -> broadcast::Receiver<ThreadId> {
        self.state.subscribe_thread_created()
    }

    /// Remove and close all threads.
    pub async fn close_all(&self) {
        let ids = self.list_threads().await;
        for id in ids {
            let _ = self.shutdown_thread(id).await;
        }
    }
}

impl Default for ThreadManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ThreadManager {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_thread_manager() {
        let manager = ThreadManager::new();

        let config = AgentConfig::new(AgentRole::General, "Test task", SessionSource::User);

        let id = manager.create_thread(config).await.unwrap();

        // Check initial status
        let status = manager.get_status(id).await;
        assert!(matches!(status, AgentStatus::PendingInit));

        // Update status
        manager.set_status(id, AgentStatus::Running).await.unwrap();
        let status = manager.get_status(id).await;
        assert!(matches!(status, AgentStatus::Running));

        // Shutdown
        manager.shutdown_thread(id).await.unwrap();
        let status = manager.get_status(id).await;
        assert!(matches!(status, AgentStatus::Shutdown));
    }
}
