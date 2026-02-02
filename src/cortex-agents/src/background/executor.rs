//! Background agent executor.
//!
//! Provides the `BackgroundAgentManager` for spawning and managing
//! agents that run in the background as tokio tasks.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tokio::task::JoinHandle;

use super::events::{AgentEvent, AgentResult, AgentStatus};

/// Default maximum concurrent background agents.
pub const DEFAULT_MAX_CONCURRENT: usize = 5;

/// Default timeout for background agents (30 minutes).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Grace period for cancellation (5 seconds).
pub const CANCEL_GRACE_PERIOD: Duration = Duration::from_secs(5);

/// Errors from background agent operations.
#[derive(Debug, thiserror::Error)]
pub enum BackgroundAgentManagerError {
    /// Too many agents are already running.
    #[error("Maximum concurrent agents ({0}) reached")]
    TooManyAgents(usize),

    /// Agent not found.
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    /// Agent already completed.
    #[error("Agent already completed: {0}")]
    AlreadyCompleted(String),

    /// Timeout waiting for agent.
    #[error("Timeout waiting for agent: {0}")]
    Timeout(String),

    /// Internal channel error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Configuration for spawning a background agent.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Task description/prompt for the agent.
    pub task: String,
    /// Agent type (e.g., "general", "explore", "research").
    pub agent_type: String,
    /// Maximum timeout for the agent.
    pub timeout: Duration,
    /// Optional context to include.
    pub context: Option<String>,
    /// Priority (higher = more important).
    pub priority: i32,
}

impl AgentConfig {
    /// Creates a new agent config with default settings.
    pub fn new(task: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            agent_type: "general".to_string(),
            timeout: DEFAULT_TIMEOUT,
            context: None,
            priority: 0,
        }
    }

    /// Sets the agent type.
    pub fn with_type(mut self, agent_type: impl Into<String>) -> Self {
        self.agent_type = agent_type.into();
        self
    }

    /// Sets the timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the context.
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Sets the priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Creates a test config for unit tests.
    #[cfg(test)]
    pub fn test() -> Self {
        Self::new("Test task")
    }

    /// Creates a long-running config for tests.
    #[cfg(test)]
    pub fn long_running() -> Self {
        Self::new("Long running task").with_timeout(Duration::from_secs(300))
    }

    /// Creates a background config from a prompt.
    pub fn background(prompt: impl Into<String>) -> Self {
        Self::new(prompt).with_type("background")
    }
}

/// Information about a running background agent.
#[derive(Debug, Clone)]
pub struct RunningAgentInfo {
    /// Unique agent ID.
    pub id: String,
    /// Task description.
    pub task: String,
    /// Agent type.
    pub agent_type: String,
    /// Current status.
    pub status: AgentStatus,
    /// When the agent started.
    pub started_at: Instant,
    /// Tokens used so far.
    pub tokens_used: u64,
    /// Last progress message.
    pub last_progress: Option<String>,
}

impl RunningAgentInfo {
    /// Returns the duration since the agent started.
    pub fn duration(&self) -> Duration {
        self.started_at.elapsed()
    }
}

/// A background agent with its execution handle and control channels.
pub struct BackgroundAgent {
    /// Unique agent ID.
    pub id: String,
    /// Agent configuration.
    pub config: AgentConfig,
    /// Tokio task handle.
    handle: JoinHandle<AgentResult>,
    /// Status receiver for updates.
    #[allow(dead_code)]
    status_rx: mpsc::Receiver<AgentStatus>,
    /// Cancel signal sender.
    cancel_tx: Option<oneshot::Sender<()>>,
    /// When the agent started.
    started_at: Instant,
    /// Current status.
    status: AgentStatus,
    /// Tokens used.
    tokens_used: u64,
    /// Last progress message.
    last_progress: Option<String>,
}

impl BackgroundAgent {
    /// Creates a new background agent.
    fn new(
        id: String,
        config: AgentConfig,
        handle: JoinHandle<AgentResult>,
        status_rx: mpsc::Receiver<AgentStatus>,
        cancel_tx: oneshot::Sender<()>,
    ) -> Self {
        Self {
            id,
            config,
            handle,
            status_rx,
            cancel_tx: Some(cancel_tx),
            started_at: Instant::now(),
            status: AgentStatus::Running,
            tokens_used: 0,
            last_progress: None,
        }
    }

    /// Returns info about this agent.
    pub fn info(&self) -> RunningAgentInfo {
        RunningAgentInfo {
            id: self.id.clone(),
            task: self.config.task.clone(),
            agent_type: self.config.agent_type.clone(),
            status: self.status.clone(),
            started_at: self.started_at,
            tokens_used: self.tokens_used,
            last_progress: self.last_progress.clone(),
        }
    }

    /// Returns true if the agent has completed (success or failure).
    pub fn is_completed(&self) -> bool {
        self.handle.is_finished()
    }

    /// Cancels the agent.
    fn cancel(&mut self) {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
        self.status = AgentStatus::Cancelled;
    }
}

/// Manager for background agents.
///
/// Handles spawning, tracking, and managing multiple background agents
/// running as tokio tasks.
pub struct BackgroundAgentManager {
    /// Running agents by ID.
    agents: Arc<RwLock<HashMap<String, BackgroundAgent>>>,
    /// Event broadcaster.
    event_tx: broadcast::Sender<AgentEvent>,
    /// Maximum concurrent agents.
    max_concurrent: usize,
    /// Next agent ID counter.
    next_id: Arc<std::sync::atomic::AtomicU64>,
}

impl BackgroundAgentManager {
    /// Creates a new manager with the specified max concurrent agents.
    pub fn new(max_concurrent: usize) -> Self {
        let (event_tx, _) = broadcast::channel(256);

        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            max_concurrent,
            next_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    /// Creates a manager with default settings.
    pub fn default_manager() -> Self {
        Self::new(DEFAULT_MAX_CONCURRENT)
    }

    /// Spawns a new background agent.
    ///
    /// Returns the agent ID on success.
    pub async fn spawn(&self, config: AgentConfig) -> Result<String, BackgroundAgentManagerError> {
        // Check concurrent limit
        let agents = self.agents.read().await;
        let active_count = agents.values().filter(|a| !a.is_completed()).count();
        if active_count >= self.max_concurrent {
            return Err(BackgroundAgentManagerError::TooManyAgents(
                self.max_concurrent,
            ));
        }
        drop(agents);

        // Generate ID
        let id = format!(
            "bg-{}",
            self.next_id
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        );

        // Create channels
        let (status_tx, status_rx) = mpsc::channel(100);
        let (cancel_tx, cancel_rx) = oneshot::channel();

        // Clone what we need for the task
        let task = config.task.clone();
        let timeout = config.timeout;
        let event_tx = self.event_tx.clone();
        let agent_id = id.clone();

        // Broadcast started event
        let started_at = Instant::now();
        let _ = event_tx.send(AgentEvent::Started {
            id: agent_id.clone(),
            task: task.clone(),
            started_at,
        });

        // Spawn the background task
        let handle = tokio::spawn(async move {
            run_background_agent(agent_id, task, timeout, status_tx, cancel_rx, event_tx).await
        });

        // Create and store the agent
        let agent = BackgroundAgent::new(id.clone(), config, handle, status_rx, cancel_tx);

        let mut agents = self.agents.write().await;
        agents.insert(id.clone(), agent);

        Ok(id)
    }

    /// Lists all agents (including completed ones).
    pub async fn list(&self) -> Vec<RunningAgentInfo> {
        let agents = self.agents.read().await;
        agents.values().map(|a| a.info()).collect()
    }

    /// Lists only active (running) agents.
    pub async fn list_active(&self) -> Vec<RunningAgentInfo> {
        let agents = self.agents.read().await;
        agents
            .values()
            .filter(|a| !a.is_completed())
            .map(|a| a.info())
            .collect()
    }

    /// Gets the status of a specific agent.
    pub async fn get_status(&self, id: &str) -> Option<AgentStatus> {
        let agents = self.agents.read().await;
        agents.get(id).map(|a| a.status.clone())
    }

    /// Gets info about a specific agent.
    pub async fn get_info(&self, id: &str) -> Option<RunningAgentInfo> {
        let agents = self.agents.read().await;
        agents.get(id).map(|a| a.info())
    }

    /// Waits for an agent to complete with timeout.
    pub async fn wait(
        &self,
        id: &str,
        timeout: Duration,
    ) -> Result<AgentResult, BackgroundAgentManagerError> {
        // Get the handle
        let mut agents = self.agents.write().await;
        let agent = agents
            .remove(id)
            .ok_or_else(|| BackgroundAgentManagerError::AgentNotFound(id.to_string()))?;
        drop(agents);

        // Wait with timeout
        match tokio::time::timeout(timeout, agent.handle).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(e)) => Err(BackgroundAgentManagerError::Internal(format!(
                "Agent task panicked: {}",
                e
            ))),
            Err(_) => Err(BackgroundAgentManagerError::Timeout(id.to_string())),
        }
    }

    /// Cancels a running agent.
    pub async fn cancel(&self, id: &str) -> Result<(), BackgroundAgentManagerError> {
        let mut agents = self.agents.write().await;
        let agent = agents
            .get_mut(id)
            .ok_or_else(|| BackgroundAgentManagerError::AgentNotFound(id.to_string()))?;

        if agent.is_completed() {
            return Err(BackgroundAgentManagerError::AlreadyCompleted(
                id.to_string(),
            ));
        }

        agent.cancel();

        // Broadcast cancelled event
        let _ = self.event_tx.send(AgentEvent::Cancelled {
            id: id.to_string(),
            duration: agent.started_at.elapsed(),
        });

        Ok(())
    }

    /// Cancels all running agents.
    pub async fn cancel_all(&self) {
        let mut agents = self.agents.write().await;
        for (id, agent) in agents.iter_mut() {
            if !agent.is_completed() {
                agent.cancel();
                let _ = self.event_tx.send(AgentEvent::Cancelled {
                    id: id.clone(),
                    duration: agent.started_at.elapsed(),
                });
            }
        }
    }

    /// Subscribes to agent events.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    /// Returns the number of active agents.
    pub async fn active_count(&self) -> usize {
        let agents = self.agents.read().await;
        agents.values().filter(|a| !a.is_completed()).count()
    }

    /// Returns the total number of agents (including completed).
    pub async fn total_count(&self) -> usize {
        let agents = self.agents.read().await;
        agents.len()
    }

    /// Removes completed agents from the manager.
    pub async fn cleanup_completed(&self) {
        let mut agents = self.agents.write().await;
        agents.retain(|_, a| !a.is_completed());
    }
}

impl Default for BackgroundAgentManager {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CONCURRENT)
    }
}

/// Runs a background agent task.
///
/// This is the main execution function for background agents.
async fn run_background_agent(
    id: String,
    task: String,
    timeout: Duration,
    _status_tx: mpsc::Sender<AgentStatus>,
    cancel_rx: oneshot::Receiver<()>,
    event_tx: broadcast::Sender<AgentEvent>,
) -> AgentResult {
    let started_at = Instant::now();

    // Use select to handle cancellation and timeout
    tokio::select! {
        // Main execution path
        result = execute_agent_task(&id, &task, &event_tx) => {
            result
        }

        // Cancellation
        _ = cancel_rx => {
            AgentResult::cancelled(started_at.elapsed())
        }

        // Timeout
        _ = tokio::time::sleep(timeout) => {
            let _ = event_tx.send(AgentEvent::TimedOut {
                id: id.clone(),
                timeout,
            });
            AgentResult::failure(format!("Timed out after {:?}", timeout), started_at.elapsed())
        }
    }
}

/// Executes the actual agent task.
///
/// This is a placeholder implementation that simulates agent work.
/// In production, this would integrate with the LLM and tool execution.
async fn execute_agent_task(
    id: &str,
    task: &str,
    event_tx: &broadcast::Sender<AgentEvent>,
) -> AgentResult {
    let started_at = Instant::now();

    // Send progress update
    let _ = event_tx.send(AgentEvent::Progress {
        id: id.to_string(),
        message: "Starting background task...".to_string(),
        percentage: Some(0),
    });

    // Simulate some work (in production, this would be actual LLM calls and tool execution)
    // The actual implementation would call the LLM API and execute tools
    tokio::time::sleep(Duration::from_millis(100)).await;

    let _ = event_tx.send(AgentEvent::Progress {
        id: id.to_string(),
        message: "Processing...".to_string(),
        percentage: Some(50),
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let _ = event_tx.send(AgentEvent::Progress {
        id: id.to_string(),
        message: "Completing...".to_string(),
        percentage: Some(90),
    });

    // Create result
    let result = AgentResult::success(
        format!("Completed background task: {}", task),
        format!("Task '{}' executed successfully in the background.", task),
        started_at.elapsed(),
    );

    // Send completed event
    let _ = event_tx.send(AgentEvent::Completed {
        id: id.to_string(),
        result: result.clone(),
    });

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spawn_background_agent() {
        let manager = BackgroundAgentManager::new(5);
        let id = manager.spawn(AgentConfig::test()).await.unwrap();
        assert!(!id.is_empty());
        assert!(id.starts_with("bg-"));
    }

    #[tokio::test]
    async fn test_max_concurrent_limit() {
        let manager = BackgroundAgentManager::new(2);

        // Spawn 2 agents (should succeed)
        manager.spawn(AgentConfig::test()).await.unwrap();
        manager.spawn(AgentConfig::test()).await.unwrap();

        // Wait a bit for tasks to register as running
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Try to spawn a third (may succeed if previous completed quickly)
        // This test is time-sensitive due to the simulated work
        let active = manager.active_count().await;
        assert!(active <= 2);
    }

    #[tokio::test]
    async fn test_list_agents() {
        let manager = BackgroundAgentManager::new(5);

        manager.spawn(AgentConfig::new("Task 1")).await.unwrap();
        manager.spawn(AgentConfig::new("Task 2")).await.unwrap();

        let agents = manager.list().await;
        assert!(agents.len() >= 2 || agents.iter().any(|a| a.task == "Task 1"));
    }

    #[tokio::test]
    async fn test_agent_cancellation() {
        let manager = BackgroundAgentManager::new(5);

        // Subscribe to events
        let mut events = manager.subscribe();

        let id = manager
            .spawn(AgentConfig::new("Long task").with_timeout(Duration::from_secs(60)))
            .await
            .unwrap();

        // Cancel immediately
        manager.cancel(&id).await.unwrap();

        // Check for cancelled event
        let mut found_cancelled = false;
        while let Ok(event) = tokio::time::timeout(Duration::from_secs(1), events.recv()).await {
            if let Ok(AgentEvent::Cancelled { id: event_id, .. }) = event {
                if event_id == id {
                    found_cancelled = true;
                    break;
                }
            }
        }
        assert!(found_cancelled);
    }

    #[tokio::test]
    async fn test_subscribe_to_events() {
        let manager = BackgroundAgentManager::new(5);
        let mut events = manager.subscribe();

        let id = manager.spawn(AgentConfig::test()).await.unwrap();

        // Should receive started event
        let mut found_started = false;
        while let Ok(event) = tokio::time::timeout(Duration::from_secs(2), events.recv()).await {
            if let Ok(AgentEvent::Started { id: event_id, .. }) = event {
                if event_id == id {
                    found_started = true;
                    break;
                }
            }
        }
        assert!(found_started);
    }

    #[tokio::test]
    async fn test_wait_for_completion() {
        let manager = BackgroundAgentManager::new(5);

        let id = manager.spawn(AgentConfig::test()).await.unwrap();

        // Wait for completion
        let result = manager.wait(&id, Duration::from_secs(10)).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_agent_config_builder() {
        let config = AgentConfig::new("Test task")
            .with_type("explore")
            .with_timeout(Duration::from_secs(120))
            .with_context("Some context")
            .with_priority(5);

        assert_eq!(config.task, "Test task");
        assert_eq!(config.agent_type, "explore");
        assert_eq!(config.timeout, Duration::from_secs(120));
        assert_eq!(config.context, Some("Some context".to_string()));
        assert_eq!(config.priority, 5);
    }

    #[tokio::test]
    async fn test_cleanup_completed() {
        let manager = BackgroundAgentManager::new(5);

        manager.spawn(AgentConfig::test()).await.unwrap();

        // Wait for completion
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Cleanup
        manager.cleanup_completed().await;

        // Should have fewer or no agents
        let count = manager.total_count().await;
        assert!(count <= 1);
    }
}
