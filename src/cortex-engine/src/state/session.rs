//! Session state management.
//!
//! Tracks the state of agent sessions including conversation history,
//! active turns, and session metadata.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

use super::turn::TurnManager;

/// Session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Session ID.
    pub id: String,
    /// Session phase.
    pub phase: SessionPhase,
    /// Created timestamp.
    pub created_at: u64,
    /// Last activity timestamp.
    pub last_activity: u64,
    /// Working directory.
    pub working_dir: PathBuf,
    /// Model being used.
    pub model: String,
    /// Provider name.
    pub provider: String,
    /// Turn count.
    pub turn_count: u32,
    /// Total tokens used.
    pub total_tokens: u64,
    /// Session metadata.
    pub metadata: SessionMetadata,
    /// Current turn ID.
    pub current_turn: Option<String>,
    /// Completed turn IDs.
    pub completed_turns: Vec<String>,
}

impl SessionState {
    /// Create a new session state.
    pub fn new(id: impl Into<String>, working_dir: impl Into<PathBuf>) -> Self {
        let now = timestamp_now();
        Self {
            id: id.into(),
            phase: SessionPhase::Initializing,
            created_at: now,
            last_activity: now,
            working_dir: working_dir.into(),
            model: String::new(),
            provider: String::new(),
            turn_count: 0,
            total_tokens: 0,
            metadata: SessionMetadata::default(),
            current_turn: None,
            completed_turns: Vec::new(),
        }
    }

    /// Set model and provider.
    pub fn with_model(mut self, provider: impl Into<String>, model: impl Into<String>) -> Self {
        self.provider = provider.into();
        self.model = model.into();
        self
    }

    /// Transition to a new phase.
    pub fn transition(&mut self, phase: SessionPhase) -> Result<()> {
        // Validate transition
        let valid = match (&self.phase, &phase) {
            (SessionPhase::Initializing, SessionPhase::Ready) => true,
            (SessionPhase::Ready, SessionPhase::Active) => true,
            (SessionPhase::Active, SessionPhase::Ready) => true,
            (SessionPhase::Active, SessionPhase::Paused) => true,
            (SessionPhase::Paused, SessionPhase::Active) => true,
            (SessionPhase::Paused, SessionPhase::Ready) => true,
            (_, SessionPhase::Error(_)) => true,
            (_, SessionPhase::Completed) => true,
            _ => false,
        };

        if !valid {
            return Err(CortexError::InvalidInput(format!(
                "Invalid session transition: {:?} -> {:?}",
                self.phase, phase
            )));
        }

        self.phase = phase;
        self.last_activity = timestamp_now();
        Ok(())
    }

    /// Start a new turn.
    pub fn start_turn(&mut self, turn_id: impl Into<String>) {
        let turn_id = turn_id.into();
        self.current_turn = Some(turn_id);
        self.turn_count += 1;
        self.last_activity = timestamp_now();
        self.phase = SessionPhase::Active;
    }

    /// Complete current turn.
    pub fn complete_turn(&mut self, tokens_used: u64) {
        if let Some(turn_id) = self.current_turn.take() {
            self.completed_turns.push(turn_id);
        }
        self.total_tokens += tokens_used;
        self.last_activity = timestamp_now();
        self.phase = SessionPhase::Ready;
    }

    /// Check if session is active.
    pub fn is_active(&self) -> bool {
        matches!(self.phase, SessionPhase::Active)
    }

    /// Check if session is completed.
    pub fn is_completed(&self) -> bool {
        matches!(self.phase, SessionPhase::Completed)
    }

    /// Get session duration.
    pub fn duration(&self) -> Duration {
        let now = timestamp_now();
        Duration::from_secs(now.saturating_sub(self.created_at))
    }

    /// Check if session has timed out.
    pub fn is_timed_out(&self, timeout: Duration) -> bool {
        let now = timestamp_now();
        let inactive = now.saturating_sub(self.last_activity);
        Duration::from_secs(inactive) > timeout
    }
}

/// Session phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
#[derive(Default)]
pub enum SessionPhase {
    /// Session is initializing.
    #[default]
    Initializing,
    /// Session is ready for input.
    Ready,
    /// Session is actively processing.
    Active,
    /// Session is paused.
    Paused,
    /// Session completed successfully.
    Completed,
    /// Session encountered an error.
    Error(String),
}

/// Session metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// User ID.
    pub user_id: Option<String>,
    /// Session name.
    pub name: Option<String>,
    /// Session tags.
    pub tags: Vec<String>,
    /// Custom data.
    pub custom: HashMap<String, String>,
    /// Git branch at session start.
    pub git_branch: Option<String>,
    /// Git commit at session start.
    pub git_commit: Option<String>,
}

impl SessionMetadata {
    /// Set user ID.
    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Set session name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Add a tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Set custom data.
    pub fn with_custom(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.custom.insert(key.into(), value.into());
        self
    }
}

/// Session state manager.
pub struct SessionStateManager {
    /// Active sessions.
    sessions: RwLock<HashMap<String, SessionState>>,
    /// Turn managers per session.
    turn_managers: RwLock<HashMap<String, TurnManager>>,
    /// Maximum sessions.
    max_sessions: usize,
}

impl SessionStateManager {
    /// Create a new session state manager.
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            turn_managers: RwLock::new(HashMap::new()),
            max_sessions,
        }
    }

    /// Create a new session.
    pub async fn create(&self, state: SessionState) -> Result<String> {
        let mut sessions = self.sessions.write().await;

        if sessions.len() >= self.max_sessions {
            return Err(CortexError::InvalidInput(
                "Maximum session limit reached".to_string(),
            ));
        }

        let id = state.id.clone();
        sessions.insert(id.clone(), state);

        // Create turn manager
        self.turn_managers
            .write()
            .await
            .insert(id.clone(), TurnManager::new());

        Ok(id)
    }

    /// Get a session.
    pub async fn get(&self, id: &str) -> Option<SessionState> {
        self.sessions.read().await.get(id).cloned()
    }

    /// Update a session.
    pub async fn update(&self, id: &str, f: impl FnOnce(&mut SessionState)) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| CortexError::NotFound(format!("Session not found: {id}")))?;
        f(session);
        Ok(())
    }

    /// Delete a session.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.sessions.write().await.remove(id);
        self.turn_managers.write().await.remove(id);
        Ok(())
    }

    /// Get active session count.
    pub async fn active_count(&self) -> usize {
        self.sessions
            .read()
            .await
            .values()
            .filter(|s| s.is_active())
            .count()
    }

    /// List all sessions.
    pub async fn list(&self) -> Vec<SessionInfo> {
        self.sessions
            .read()
            .await
            .values()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                phase: s.phase.clone(),
                created_at: s.created_at,
                turn_count: s.turn_count,
                model: s.model.clone(),
            })
            .collect()
    }

    /// Get turn manager for a session.
    pub async fn turn_manager(&self, session_id: &str) -> Option<TurnManager> {
        self.turn_managers.read().await.get(session_id).cloned()
    }

    /// Clean up timed out sessions.
    pub async fn cleanup_timed_out(&self, timeout: Duration) -> Vec<String> {
        let mut sessions = self.sessions.write().await;
        let mut removed = Vec::new();

        sessions.retain(|id, session| {
            if session.is_timed_out(timeout) {
                removed.push(id.clone());
                false
            } else {
                true
            }
        });

        // Clean up turn managers
        let mut turn_managers = self.turn_managers.write().await;
        for id in &removed {
            turn_managers.remove(id);
        }

        removed
    }
}

/// Session info for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Session ID.
    pub id: String,
    /// Current phase.
    pub phase: SessionPhase,
    /// Created timestamp.
    pub created_at: u64,
    /// Turn count.
    pub turn_count: u32,
    /// Model being used.
    pub model: String,
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_state() {
        let mut state = SessionState::new("test-1", "/tmp");
        assert_eq!(state.phase, SessionPhase::Initializing);

        state.transition(SessionPhase::Ready).unwrap();
        assert_eq!(state.phase, SessionPhase::Ready);

        state.start_turn("turn-1");
        assert_eq!(state.turn_count, 1);
        assert!(state.is_active());

        state.complete_turn(100);
        assert_eq!(state.total_tokens, 100);
        assert!(!state.is_active());
    }

    #[test]
    fn test_invalid_transition() {
        let mut state = SessionState::new("test-1", "/tmp");
        // Can't go from Initializing to Active directly
        assert!(state.transition(SessionPhase::Active).is_err());
    }

    #[tokio::test]
    async fn test_session_manager() {
        let manager = SessionStateManager::new(10);

        let state = SessionState::new("test-1", "/tmp");
        let id = manager.create(state).await.unwrap();

        assert_eq!(id, "test-1");
        assert!(manager.get(&id).await.is_some());

        manager
            .update(&id, |s| {
                s.transition(SessionPhase::Ready).unwrap();
            })
            .await
            .unwrap();

        let updated = manager.get(&id).await.unwrap();
        assert_eq!(updated.phase, SessionPhase::Ready);
    }
}
