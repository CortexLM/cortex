//! Application state management.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

use crate::config::ServerConfig;
use crate::error::{AppError, AppResult};
use crate::file_watcher::{FileChangeEvent, FileWatcher};
use crate::session_manager::SessionManager;
use crate::share::ShareManager;
use crate::streaming::CliSessionManager;
use crate::tasks::TaskManager;
use crate::terminal_streaming;
use crate::websocket::WsMessage;

/// Application state shared across request handlers.
pub struct AppState {
    /// Server configuration.
    pub config: ServerConfig,
    /// Active sessions (legacy API state).
    sessions: RwLock<HashMap<String, SessionState>>,
    /// CLI session manager - manages real cortex-core Sessions (WebSocket).
    pub cli_sessions: SessionManager,
    /// CLI session manager for HTTP streaming.
    pub cli_session_manager: CliSessionManager,
    /// Rate limiters by key (IP or API key).
    rate_limiters: RwLock<HashMap<String, RateLimiterState>>,
    /// Metrics collector.
    metrics: RwLock<MetricsState>,
    /// Start time.
    start_time: Instant,
    /// File watcher for /workspace.
    file_watcher: FileWatcher,
    /// Broadcast channel for server-wide messages (terminals, etc.)
    pub broadcast_tx: broadcast::Sender<WsMessage>,
    /// Terminal streaming task handle.
    _terminal_task: Option<tokio::task::JoinHandle<()>>,
    /// Share manager for session sharing.
    pub share_manager: ShareManager,
    /// Task manager for todo list tracking.
    pub task_manager: TaskManager,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("config", &self.config)
            .field("start_time", &self.start_time)
            .finish()
    }
}

impl AppState {
    /// Create new application state with automatic background cleanup.
    pub async fn new(config: ServerConfig) -> AppResult<Self> {
        // Create broadcast channel for server-wide messages
        let (broadcast_tx, _) = broadcast::channel(1000);

        // Start terminal output streaming
        let terminal_task = terminal_streaming::start_terminal_streaming(broadcast_tx.clone());

        let state = Self {
            config,
            sessions: RwLock::new(HashMap::new()),
            cli_sessions: SessionManager::new(),
            cli_session_manager: CliSessionManager::new(),
            rate_limiters: RwLock::new(HashMap::new()),
            metrics: RwLock::new(MetricsState::default()),
            start_time: Instant::now(),
            file_watcher: FileWatcher::new(
                &std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string()),
            ),
            broadcast_tx,
            _terminal_task: Some(terminal_task),
            share_manager: ShareManager::new(),
            task_manager: TaskManager::new(),
        };

        Ok(state)
    }

    /// Start background cleanup task that runs periodically.
    /// Call this after wrapping AppState in Arc to start the cleanup loop.
    pub fn start_cleanup_task(self: &Arc<Self>) {
        let state = Arc::clone(self);
        let cleanup_interval = Duration::from_secs(60); // Run cleanup every minute

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                // Cleanup expired sessions
                state.cleanup_expired_sessions().await;

                // Cleanup stale rate limiter entries
                state.cleanup_rate_limiters().await;

                // Cleanup expired shares
                let shares_cleaned = state.share_manager.cleanup_expired().await;
                if shares_cleaned > 0 {
                    tracing::debug!("Cleaned up {} expired shares", shares_cleaned);
                }

                tracing::debug!(
                    "Background cleanup completed. Active sessions: {}",
                    state.sessions.read().await.len()
                );
            }
        });
    }

    /// Subscribe to file change events.
    pub fn subscribe_file_changes(&self) -> broadcast::Receiver<FileChangeEvent> {
        self.file_watcher.subscribe()
    }

    /// Get uptime duration.
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Create a new session.
    pub async fn create_session(&self, options: CreateSessionOptions) -> AppResult<SessionState> {
        let mut sessions = self.sessions.write().await;

        if sessions.len() >= self.config.sessions.max_concurrent {
            return Err(AppError::Session(
                "Max concurrent sessions reached".to_string(),
            ));
        }

        let session = SessionState::new(options);
        let id = session.id.clone();
        sessions.insert(id.clone(), session.clone());

        // Update metrics
        self.increment_counter("sessions_created").await;

        Ok(session)
    }

    /// Get a session by ID.
    pub async fn get_session(&self, id: &str) -> AppResult<SessionState> {
        let sessions = self.sessions.read().await;
        sessions
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("Session not found: {id}")))
    }

    /// Update a session.
    pub async fn update_session(
        &self,
        id: &str,
        update: impl FnOnce(&mut SessionState),
    ) -> AppResult<SessionState> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| AppError::NotFound(format!("Session not found: {id}")))?;

        update(session);
        session.updated_at = Instant::now();
        Ok(session.clone())
    }

    /// Delete a session.
    pub async fn delete_session(&self, id: &str) -> AppResult<()> {
        let mut sessions = self.sessions.write().await;
        if sessions.remove(id).is_some() {
            self.increment_counter("sessions_deleted").await;
            Ok(())
        } else {
            Err(AppError::NotFound(format!("Session not found: {id}")))
        }
    }

    /// List all sessions.
    pub async fn list_sessions(&self, limit: usize, offset: usize) -> Vec<SessionSummary> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .skip(offset)
            .take(limit)
            .map(|s| SessionSummary {
                id: s.id.clone(),
                model: s.model.clone(),
                status: s.status.clone(),
                message_count: s.messages.len(),
                total_tokens: s.total_tokens,
                created_at: s.created_at,
            })
            .collect()
    }

    /// Check rate limit for a key.
    pub async fn check_rate_limit(&self, key: &str) -> AppResult<()> {
        if !self.config.rate_limit.enabled {
            return Ok(());
        }

        let mut limiters = self.rate_limiters.write().await;
        let now = Instant::now();

        let limiter = limiters
            .entry(key.to_string())
            .or_insert_with(|| RateLimiterState {
                tokens: self.config.rate_limit.burst_size as f64,
                last_update: now,
            });

        // Calculate tokens to add based on elapsed time
        let elapsed = now.duration_since(limiter.last_update).as_secs_f64();
        let tokens_per_second = self.config.rate_limit.requests_per_minute as f64 / 60.0;
        let new_tokens = elapsed * tokens_per_second;

        limiter.tokens =
            (limiter.tokens + new_tokens).min(self.config.rate_limit.burst_size as f64);
        limiter.last_update = now;

        if limiter.tokens >= 1.0 {
            limiter.tokens -= 1.0;
            Ok(())
        } else {
            self.increment_counter("rate_limit_exceeded").await;
            Err(AppError::RateLimitExceeded)
        }
    }

    /// Increment a counter metric.
    pub async fn increment_counter(&self, name: &str) {
        let mut metrics = self.metrics.write().await;
        *metrics.counters.entry(name.to_string()).or_insert(0) += 1;
    }

    /// Get metrics snapshot.
    pub async fn get_metrics(&self) -> MetricsSnapshot {
        let metrics = self.metrics.read().await;
        let sessions = self.sessions.read().await;

        MetricsSnapshot {
            uptime_seconds: self.uptime().as_secs(),
            active_sessions: sessions.len(),
            total_requests: *metrics.counters.get("total_requests").unwrap_or(&0),
            rate_limit_hits: *metrics.counters.get("rate_limit_exceeded").unwrap_or(&0),
            sessions_created: *metrics.counters.get("sessions_created").unwrap_or(&0),
            errors: *metrics.counters.get("errors").unwrap_or(&0),
        }
    }

    /// Cleanup expired sessions.
    pub async fn cleanup_expired_sessions(&self) {
        let timeout = Duration::from_secs(self.config.sessions.timeout);
        let now = Instant::now();

        let mut sessions = self.sessions.write().await;
        let initial_count = sessions.len();
        sessions.retain(|_, session| now.duration_since(session.updated_at) < timeout);
        let removed = initial_count - sessions.len();

        if removed > 0 {
            tracing::info!("Cleaned up {} expired sessions", removed);
        }
    }

    /// Cleanup stale rate limiter entries to prevent memory growth.
    /// Removes entries that haven't been accessed recently.
    pub async fn cleanup_rate_limiters(&self) {
        // Rate limiter entries older than this will be evicted
        const RATE_LIMITER_TTL_SECS: u64 = 3600; // 1 hour

        let now = Instant::now();
        let ttl = Duration::from_secs(RATE_LIMITER_TTL_SECS);

        let mut limiters = self.rate_limiters.write().await;
        let initial_count = limiters.len();

        // Remove entries that haven't been used recently
        limiters.retain(|_, state| now.duration_since(state.last_update) < ttl);

        let removed = initial_count - limiters.len();
        if removed > 0 {
            tracing::debug!("Evicted {} stale rate limiter entries", removed);
        }
    }
}

/// Options for creating a session.
#[derive(Debug, Clone, Default)]
pub struct CreateSessionOptions {
    /// User ID (if authenticated).
    pub user_id: Option<String>,
    /// Model to use.
    pub model: Option<String>,
    /// System prompt.
    pub system_prompt: Option<String>,
    /// Custom metadata.
    pub metadata: Option<serde_json::Value>,
}

/// Session state.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// Unique session ID.
    pub id: String,
    /// User ID (if authenticated).
    pub user_id: Option<String>,
    /// Model being used.
    pub model: String,
    /// Session status.
    pub status: SessionStatus,
    /// Messages in the conversation.
    pub messages: Vec<SessionMessage>,
    /// Total tokens used.
    pub total_tokens: u64,
    /// System prompt.
    pub system_prompt: Option<String>,
    /// Custom metadata.
    pub metadata: Option<serde_json::Value>,
    /// Creation time.
    pub created_at: Instant,
    /// Last update time.
    pub updated_at: Instant,
}

impl SessionState {
    /// Create a new session.
    pub fn new(options: CreateSessionOptions) -> Self {
        let now = Instant::now();
        Self {
            id: Uuid::new_v4().to_string(),
            user_id: options.user_id,
            model: options.model.unwrap_or_else(|| "gpt-4o".to_string()),
            status: SessionStatus::Active,
            messages: Vec::new(),
            total_tokens: 0,
            system_prompt: options.system_prompt,
            metadata: options.metadata,
            created_at: now,
            updated_at: now,
        }
    }

    /// Add a message to the session.
    pub fn add_message(&mut self, message: SessionMessage) {
        self.total_tokens += message.tokens as u64;
        self.messages.push(message);
        self.updated_at = Instant::now();
    }
}

/// Session status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Session is active.
    Active,
    /// Session is processing a request.
    Processing,
    /// Session is paused.
    Paused,
    /// Session is completed.
    Completed,
    /// Session has errored.
    Error(String),
}

/// A message in a session.
#[derive(Debug, Clone)]
pub struct SessionMessage {
    /// Message ID.
    pub id: String,
    /// Role (user, assistant, system, tool).
    pub role: String,
    /// Content.
    pub content: String,
    /// Tokens used.
    pub tokens: u32,
    /// Tool calls (if any).
    pub tool_calls: Option<Vec<ToolCallInfo>>,
    /// Timestamp.
    pub timestamp: Instant,
}

/// Tool call information.
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments (JSON string).
    pub arguments: String,
    /// Result (if completed).
    pub result: Option<String>,
}

/// Session summary for listing.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Session ID.
    pub id: String,
    /// Model.
    pub model: String,
    /// Status.
    pub status: SessionStatus,
    /// Number of messages.
    pub message_count: usize,
    /// Total tokens used.
    pub total_tokens: u64,
    /// Created at.
    pub created_at: Instant,
}

/// Rate limiter state.
#[derive(Debug, Clone)]
struct RateLimiterState {
    /// Available tokens.
    tokens: f64,
    /// Last update time.
    last_update: Instant,
}

/// Metrics state.
#[derive(Debug, Default)]
struct MetricsState {
    /// Counter metrics.
    counters: HashMap<String, u64>,
}

/// Metrics snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    /// Server uptime in seconds.
    pub uptime_seconds: u64,
    /// Active sessions.
    pub active_sessions: usize,
    /// Total requests.
    pub total_requests: u64,
    /// Rate limit hits.
    pub rate_limit_hits: u64,
    /// Sessions created.
    pub sessions_created: u64,
    /// Errors.
    pub errors: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_session() {
        let state = AppState::new(ServerConfig::default()).await.unwrap();
        let session = state
            .create_session(CreateSessionOptions::default())
            .await
            .unwrap();
        assert!(!session.id.is_empty());
        assert_eq!(session.status, SessionStatus::Active);
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let mut config = ServerConfig::default();
        config.rate_limit.requests_per_minute = 1;
        config.rate_limit.burst_size = 1;

        let state = AppState::new(config).await.unwrap();

        // First request should succeed
        assert!(state.check_rate_limit("test").await.is_ok());

        // Second request should fail (no burst)
        assert!(state.check_rate_limit("test").await.is_err());
    }

    #[tokio::test]
    async fn test_session_lifecycle() {
        let state = AppState::new(ServerConfig::default()).await.unwrap();

        let session = state
            .create_session(CreateSessionOptions::default())
            .await
            .unwrap();
        let id = session.id.clone();

        // Get session
        let fetched = state.get_session(&id).await.unwrap();
        assert_eq!(fetched.id, id);

        // Delete session
        state.delete_session(&id).await.unwrap();

        // Should not exist
        assert!(state.get_session(&id).await.is_err());
    }
}
