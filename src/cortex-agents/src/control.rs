//! Agent control for multi-agent operations.
//!
//! Provides a control-plane interface for managing agent threads,
//! handling spawning, lifecycle, and inter-agent communication.

use crate::AgentInfo;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{watch, RwLock};
use uuid::Uuid;

/// Unique identifier for an agent thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AgentThreadId(Uuid);

impl AgentThreadId {
    /// Create a new random thread ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parse from string.
    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl Default for AgentThreadId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AgentThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Status of an agent in the control system.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentThreadStatus {
    /// Agent is being initialized.
    #[default]
    PendingInit,
    /// Agent is running.
    Running,
    /// Agent completed successfully.
    Completed(Option<String>),
    /// Agent encountered an error.
    Errored(String),
    /// Agent was shutdown.
    Shutdown,
    /// Agent was not found.
    NotFound,
}

impl AgentThreadStatus {
    /// Check if the status is final.
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            AgentThreadStatus::Completed(_)
                | AgentThreadStatus::Errored(_)
                | AgentThreadStatus::Shutdown
                | AgentThreadStatus::NotFound
        )
    }
}

/// An agent thread managed by the control system.
pub struct AgentThread {
    /// Thread ID.
    pub id: AgentThreadId,
    /// Agent info.
    pub agent_info: AgentInfo,
    /// Parent thread ID (if sub-agent).
    pub parent_id: Option<AgentThreadId>,
    /// Depth level (0 = root).
    pub depth: u32,
    /// Status sender.
    status_tx: watch::Sender<AgentThreadStatus>,
    /// Status receiver.
    status_rx: watch::Receiver<AgentThreadStatus>,
    /// Creation time.
    pub created_at: std::time::Instant,
}

impl AgentThread {
    /// Create a new agent thread.
    pub fn new(
        id: AgentThreadId,
        agent_info: AgentInfo,
        parent_id: Option<AgentThreadId>,
        depth: u32,
    ) -> Self {
        let (status_tx, status_rx) = watch::channel(AgentThreadStatus::PendingInit);
        Self {
            id,
            agent_info,
            parent_id,
            depth,
            status_tx,
            status_rx,
            created_at: std::time::Instant::now(),
        }
    }

    /// Get current status.
    pub fn status(&self) -> AgentThreadStatus {
        self.status_rx.borrow().clone()
    }

    /// Update status.
    pub fn set_status(&self, status: AgentThreadStatus) {
        if self.status_tx.send(status.clone()).is_err() {
            tracing::warn!(
                "Failed to send status update {:?} for agent {} - all receivers dropped",
                status,
                self.id
            );
        }
    }

    /// Subscribe to status updates.
    pub fn subscribe_status(&self) -> watch::Receiver<AgentThreadStatus> {
        self.status_rx.clone()
    }
}

/// Limits on multi-agent capabilities.
#[derive(Debug, Clone)]
pub struct AgentLimits {
    /// Maximum concurrent agents.
    pub max_concurrent: usize,
    /// Maximum spawn depth.
    pub max_depth: u32,
    /// Maximum total spawns per session.
    pub max_total_spawns: usize,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_concurrent: 10,
            max_depth: 1,
            max_total_spawns: 100,
        }
    }
}

/// Shared state for agent control.
pub struct AgentControlState {
    /// Active threads.
    threads: RwLock<HashMap<AgentThreadId, Arc<AgentThread>>>,
    /// Spawn limits.
    limits: AgentLimits,
    /// Total spawn count.
    total_spawns: AtomicUsize,
    /// Active thread count.
    active_count: AtomicUsize,
}

impl AgentControlState {
    /// Create new state.
    pub fn new(limits: AgentLimits) -> Self {
        Self {
            threads: RwLock::new(HashMap::new()),
            limits,
            total_spawns: AtomicUsize::new(0),
            active_count: AtomicUsize::new(0),
        }
    }

    /// Get a thread by ID.
    pub async fn get_thread(&self, id: AgentThreadId) -> Option<Arc<AgentThread>> {
        let threads = self.threads.read().await;
        threads.get(&id).cloned()
    }

    /// Insert a new thread.
    pub async fn insert_thread(&self, thread: AgentThread) {
        let id = thread.id;
        let thread = Arc::new(thread);
        let mut threads = self.threads.write().await;
        threads.insert(id, thread);
        self.active_count.fetch_add(1, Ordering::AcqRel);
    }

    /// Remove a thread.
    pub async fn remove_thread(&self, id: AgentThreadId) -> Option<Arc<AgentThread>> {
        let mut threads = self.threads.write().await;
        let removed = threads.remove(&id);
        if removed.is_some() {
            self.active_count.fetch_sub(1, Ordering::AcqRel);
        }
        removed
    }

    /// Get active count.
    pub fn active_count(&self) -> usize {
        self.active_count.load(Ordering::Acquire)
    }

    /// Get total spawns.
    pub fn total_spawns(&self) -> usize {
        self.total_spawns.load(Ordering::Acquire)
    }

    /// Check if spawn is allowed.
    pub fn can_spawn(&self, depth: u32) -> bool {
        let active = self.active_count();
        let total = self.total_spawns();

        active < self.limits.max_concurrent
            && depth <= self.limits.max_depth
            && total < self.limits.max_total_spawns
    }

    /// Record a spawn.
    pub fn record_spawn(&self) {
        self.total_spawns.fetch_add(1, Ordering::AcqRel);
    }
}

impl Default for AgentControlState {
    fn default() -> Self {
        Self::new(AgentLimits::default())
    }
}

/// Control-plane handle for multi-agent operations.
#[derive(Clone)]
pub struct AgentControl {
    /// Shared state.
    state: Arc<AgentControlState>,
}

impl AgentControl {
    /// Create a new agent control.
    pub fn new() -> Self {
        Self {
            state: Arc::new(AgentControlState::default()),
        }
    }

    /// Create with custom limits.
    pub fn with_limits(limits: AgentLimits) -> Self {
        Self {
            state: Arc::new(AgentControlState::new(limits)),
        }
    }

    /// Spawn a new agent thread.
    pub async fn spawn_agent(
        &self,
        agent_info: AgentInfo,
        parent_id: Option<AgentThreadId>,
    ) -> Result<AgentThreadId, AgentControlError> {
        let depth = if let Some(pid) = parent_id {
            if let Some(parent) = self.state.get_thread(pid).await {
                parent.depth + 1
            } else {
                return Err(AgentControlError::ParentNotFound(pid));
            }
        } else {
            0
        };

        // Check limits
        if !self.state.can_spawn(depth) {
            if depth > self.state.limits.max_depth {
                return Err(AgentControlError::DepthLimitExceeded);
            }
            if self.state.active_count() >= self.state.limits.max_concurrent {
                return Err(AgentControlError::ConcurrencyLimitExceeded);
            }
            return Err(AgentControlError::SpawnLimitExceeded);
        }

        let id = AgentThreadId::new();
        let thread = AgentThread::new(id, agent_info, parent_id, depth);

        self.state.insert_thread(thread).await;
        self.state.record_spawn();

        Ok(id)
    }

    /// Get agent status.
    pub async fn get_status(&self, id: AgentThreadId) -> AgentThreadStatus {
        match self.state.get_thread(id).await {
            Some(thread) => thread.status(),
            None => AgentThreadStatus::NotFound,
        }
    }

    /// Subscribe to status updates.
    pub async fn subscribe_status(
        &self,
        id: AgentThreadId,
    ) -> Result<watch::Receiver<AgentThreadStatus>, AgentControlError> {
        match self.state.get_thread(id).await {
            Some(thread) => Ok(thread.subscribe_status()),
            None => Err(AgentControlError::AgentNotFound(id)),
        }
    }

    /// Update agent status.
    pub async fn set_status(
        &self,
        id: AgentThreadId,
        status: AgentThreadStatus,
    ) -> Result<(), AgentControlError> {
        match self.state.get_thread(id).await {
            Some(thread) => {
                thread.set_status(status);
                Ok(())
            }
            None => Err(AgentControlError::AgentNotFound(id)),
        }
    }

    /// Shutdown an agent.
    pub async fn shutdown_agent(&self, id: AgentThreadId) -> Result<(), AgentControlError> {
        if let Some(thread) = self.state.get_thread(id).await {
            thread.set_status(AgentThreadStatus::Shutdown);
            // Keep in registry for status queries
            Ok(())
        } else {
            Err(AgentControlError::AgentNotFound(id))
        }
    }

    /// Get active agent count.
    pub fn active_count(&self) -> usize {
        self.state.active_count()
    }

    /// Get total spawns.
    pub fn total_spawns(&self) -> usize {
        self.state.total_spawns()
    }

    /// Get state reference (for advanced use).
    pub fn state(&self) -> &Arc<AgentControlState> {
        &self.state
    }
}

impl Default for AgentControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors for agent control operations.
#[derive(Debug, thiserror::Error)]
pub enum AgentControlError {
    /// Agent not found.
    #[error("Agent not found: {0}")]
    AgentNotFound(AgentThreadId),

    /// Parent agent not found.
    #[error("Parent agent not found: {0}")]
    ParentNotFound(AgentThreadId),

    /// Depth limit exceeded.
    #[error("Agent depth limit exceeded")]
    DepthLimitExceeded,

    /// Concurrency limit exceeded.
    #[error("Concurrency limit exceeded")]
    ConcurrencyLimitExceeded,

    /// Total spawn limit exceeded.
    #[error("Total spawn limit exceeded")]
    SpawnLimitExceeded,

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentInfo;

    #[tokio::test]
    async fn test_agent_control_spawn() {
        let control = AgentControl::new();
        let info = AgentInfo::new("test");

        let id = control.spawn_agent(info, None).await.unwrap();

        let status = control.get_status(id).await;
        assert!(matches!(status, AgentThreadStatus::PendingInit));
    }

    #[tokio::test]
    async fn test_agent_control_depth_limit() {
        let limits = AgentLimits {
            max_depth: 1,
            ..Default::default()
        };
        let control = AgentControl::with_limits(limits);

        // Spawn root agent
        let root = control
            .spawn_agent(AgentInfo::new("root"), None)
            .await
            .unwrap();

        // Spawn child agent
        let child = control
            .spawn_agent(AgentInfo::new("child"), Some(root))
            .await
            .unwrap();

        // Try to spawn grandchild - should fail
        let result = control
            .spawn_agent(AgentInfo::new("grandchild"), Some(child))
            .await;

        assert!(matches!(result, Err(AgentControlError::DepthLimitExceeded)));
    }
}
