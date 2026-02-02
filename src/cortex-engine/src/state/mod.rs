//! State management modules.
//!
//! Provides state tracking for sessions, turns, and services.

pub mod service;
pub mod session;
pub mod turn;

pub use service::{ServiceManager, ServiceState, ServiceStatus};
pub use session::{SessionPhase, SessionState, SessionStateManager};
pub use turn::{TurnEvent, TurnManager, TurnPhase, TurnState};

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Global application state.
pub struct AppState {
    /// Session state manager.
    pub sessions: SessionStateManager,
    /// Service manager.
    pub services: ServiceManager,
    /// Configuration.
    pub config: StateConfig,
    /// Metrics.
    pub metrics: StateMetrics,
}

impl AppState {
    /// Create new application state.
    pub fn new(config: StateConfig) -> Self {
        Self {
            sessions: SessionStateManager::new(config.max_sessions),
            services: ServiceManager::new(),
            config,
            metrics: StateMetrics::default(),
        }
    }

    /// Get active session count.
    pub async fn active_sessions(&self) -> usize {
        self.sessions.active_count().await
    }

    /// Get service status.
    pub async fn service_status(&self, name: &str) -> Option<ServiceStatus> {
        self.services.status(name).await
    }
}

/// State configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateConfig {
    /// Maximum concurrent sessions.
    pub max_sessions: usize,
    /// Session timeout.
    pub session_timeout: Duration,
    /// Turn timeout.
    pub turn_timeout: Duration,
    /// Enable state persistence.
    pub persist: bool,
    /// Persistence directory.
    pub persist_dir: Option<std::path::PathBuf>,
}

impl Default for StateConfig {
    fn default() -> Self {
        Self {
            max_sessions: 100,
            session_timeout: Duration::from_secs(3600),
            turn_timeout: Duration::from_secs(300),
            persist: false,
            persist_dir: None,
        }
    }
}

/// State metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateMetrics {
    /// Total sessions created.
    pub sessions_created: u64,
    /// Total sessions completed.
    pub sessions_completed: u64,
    /// Total turns executed.
    pub turns_executed: u64,
    /// Total tool calls.
    pub tool_calls: u64,
    /// Total tokens used.
    pub tokens_used: u64,
    /// Average turn duration in milliseconds.
    pub avg_turn_duration_ms: u64,
}

impl StateMetrics {
    /// Record a session creation.
    pub fn record_session_created(&mut self) {
        self.sessions_created += 1;
    }

    /// Record a session completion.
    pub fn record_session_completed(&mut self) {
        self.sessions_completed += 1;
    }

    /// Record a turn.
    pub fn record_turn(&mut self, duration_ms: u64, tool_calls: u64, tokens: u64) {
        self.turns_executed += 1;
        self.tool_calls += tool_calls;
        self.tokens_used += tokens;

        // Update average
        let total_duration = self.avg_turn_duration_ms * (self.turns_executed - 1) + duration_ms;
        self.avg_turn_duration_ms = total_duration / self.turns_executed;
    }
}

/// State change event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateEvent {
    /// Session created.
    SessionCreated { session_id: String },
    /// Session state changed.
    SessionStateChanged {
        session_id: String,
        phase: SessionPhase,
    },
    /// Session ended.
    SessionEnded { session_id: String },
    /// Turn started.
    TurnStarted { session_id: String, turn_id: String },
    /// Turn completed.
    TurnCompleted { session_id: String, turn_id: String },
    /// Service started.
    ServiceStarted { name: String },
    /// Service stopped.
    ServiceStopped { name: String },
}

/// State event listener.
#[async_trait::async_trait]
pub trait StateEventListener: Send + Sync {
    /// Handle a state event.
    async fn on_event(&self, event: StateEvent);
}

/// State broadcaster for publishing events.
pub struct StateBroadcaster {
    listeners: RwLock<Vec<Arc<dyn StateEventListener>>>,
}

impl StateBroadcaster {
    /// Create a new broadcaster.
    pub fn new() -> Self {
        Self {
            listeners: RwLock::new(Vec::new()),
        }
    }

    /// Add a listener.
    pub async fn add_listener(&self, listener: Arc<dyn StateEventListener>) {
        self.listeners.write().await.push(listener);
    }

    /// Broadcast an event.
    pub async fn broadcast(&self, event: StateEvent) {
        let listeners = self.listeners.read().await;
        for listener in listeners.iter() {
            listener.on_event(event.clone()).await;
        }
    }
}

impl Default for StateBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}
