//! Execution sessions.
//!
//! Provides persistent execution sessions with shared state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};
use crate::shell::ShellType;

use super::{ExecRequest, ExecResult, UnifiedExecutor};

/// Execution session configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Session name.
    pub name: String,
    /// Working directory.
    pub cwd: PathBuf,
    /// Shell type.
    pub shell: ShellType,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Default timeout.
    pub timeout: Duration,
    /// Maximum commands in history.
    pub max_history: usize,
    /// Auto-save history.
    pub auto_save: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            shell: ShellType::detect(),
            env: HashMap::new(),
            timeout: Duration::from_secs(300),
            max_history: 1000,
            auto_save: false,
        }
    }
}

/// Execution session.
pub struct ExecSession {
    /// Session ID.
    id: String,
    /// Configuration.
    config: SessionConfig,
    /// Executor.
    executor: UnifiedExecutor,
    /// Command history.
    history: RwLock<Vec<HistoryEntry>>,
    /// Session variables.
    variables: RwLock<HashMap<String, String>>,
    /// Created timestamp.
    created_at: u64,
    /// Last activity timestamp.
    last_activity: RwLock<u64>,
}

impl ExecSession {
    /// Create a new execution session.
    pub fn new(id: impl Into<String>, config: SessionConfig) -> Self {
        let executor = UnifiedExecutor::new()
            .with_shell(config.shell)
            .with_cwd(&config.cwd)
            .with_timeout(config.timeout);

        Self {
            id: id.into(),
            config,
            executor,
            history: RwLock::new(Vec::new()),
            variables: RwLock::new(HashMap::new()),
            created_at: timestamp_now(),
            last_activity: RwLock::new(timestamp_now()),
        }
    }

    /// Get session ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get session name.
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Get working directory.
    pub fn cwd(&self) -> &PathBuf {
        &self.config.cwd
    }

    /// Execute a command.
    pub async fn execute(&self, command: &str) -> Result<ExecResult> {
        *self.last_activity.write().await = timestamp_now();

        // Build request with session context
        let mut request = ExecRequest::new(command)
            .cwd(&self.config.cwd)
            .shell(self.config.shell)
            .timeout(self.config.timeout);

        // Add session environment
        for (key, value) in &self.config.env {
            request = request.env(key, value);
        }

        // Add session variables
        for (key, value) in self.variables.read().await.iter() {
            request = request.env(key, value);
        }

        // Execute
        let result = self.executor.execute(request).await?;

        // Record in history
        self.add_history(command, &result).await;

        Ok(result)
    }

    /// Add entry to history.
    async fn add_history(&self, command: &str, result: &ExecResult) {
        let entry = HistoryEntry {
            command: command.to_string(),
            exit_code: result.exit_code,
            duration_ms: result.duration_ms,
            timestamp: timestamp_now(),
            cwd: self.config.cwd.clone(),
        };

        let mut history = self.history.write().await;
        history.push(entry);

        // Trim if needed
        while history.len() > self.config.max_history {
            history.remove(0);
        }
    }

    /// Get command history.
    pub async fn history(&self) -> Vec<HistoryEntry> {
        self.history.read().await.clone()
    }

    /// Get last N commands.
    pub async fn last_commands(&self, n: usize) -> Vec<HistoryEntry> {
        let history = self.history.read().await;
        history.iter().rev().take(n).cloned().collect()
    }

    /// Clear history.
    pub async fn clear_history(&self) {
        self.history.write().await.clear();
    }

    /// Set a session variable.
    pub async fn set_var(&self, key: impl Into<String>, value: impl Into<String>) {
        self.variables
            .write()
            .await
            .insert(key.into(), value.into());
    }

    /// Get a session variable.
    pub async fn get_var(&self, key: &str) -> Option<String> {
        self.variables.read().await.get(key).cloned()
    }

    /// Remove a session variable.
    pub async fn unset_var(&self, key: &str) {
        self.variables.write().await.remove(key);
    }

    /// Get all session variables.
    pub async fn vars(&self) -> HashMap<String, String> {
        self.variables.read().await.clone()
    }

    /// Change working directory.
    pub async fn cd(&mut self, path: impl Into<PathBuf>) -> Result<()> {
        let path = path.into();

        // Resolve path
        let new_cwd = if path.is_absolute() {
            path
        } else {
            self.config.cwd.join(path)
        };

        // Verify it exists
        if !new_cwd.exists() {
            return Err(CortexError::NotFound(format!(
                "Directory not found: {}",
                new_cwd.display()
            )));
        }

        if !new_cwd.is_dir() {
            return Err(CortexError::InvalidInput(format!(
                "Not a directory: {}",
                new_cwd.display()
            )));
        }

        self.config.cwd = new_cwd.canonicalize()?;
        Ok(())
    }

    /// Get session info.
    pub async fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            name: self.config.name.clone(),
            cwd: self.config.cwd.clone(),
            shell: self.config.shell,
            created_at: self.created_at,
            last_activity: *self.last_activity.read().await,
            history_count: self.history.read().await.len(),
            variable_count: self.variables.read().await.len(),
        }
    }
}

/// History entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Command executed.
    pub command: String,
    /// Exit code.
    pub exit_code: i32,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Timestamp.
    pub timestamp: u64,
    /// Working directory at time of execution.
    pub cwd: PathBuf,
}

impl HistoryEntry {
    /// Check if command succeeded.
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Session info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Session ID.
    pub id: String,
    /// Session name.
    pub name: String,
    /// Working directory.
    pub cwd: PathBuf,
    /// Shell type.
    pub shell: ShellType,
    /// Created timestamp.
    pub created_at: u64,
    /// Last activity timestamp.
    pub last_activity: u64,
    /// Number of history entries.
    pub history_count: usize,
    /// Number of variables.
    pub variable_count: usize,
}

/// Execution session manager.
pub struct ExecSessionManager {
    /// Sessions indexed by ID.
    sessions: RwLock<HashMap<String, Arc<ExecSession>>>,
    /// Default configuration.
    default_config: SessionConfig,
    /// Maximum sessions.
    max_sessions: usize,
}

impl ExecSessionManager {
    /// Create a new session manager.
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            default_config: SessionConfig::default(),
            max_sessions,
        }
    }

    /// Set default configuration.
    pub fn with_default_config(mut self, config: SessionConfig) -> Self {
        self.default_config = config;
        self
    }

    /// Create a new session.
    pub async fn create(&self, name: &str) -> Result<Arc<ExecSession>> {
        let sessions = self.sessions.read().await;

        if sessions.len() >= self.max_sessions {
            return Err(CortexError::InvalidInput(
                "Maximum session limit reached".to_string(),
            ));
        }
        drop(sessions);

        let id = generate_session_id();
        let mut config = self.default_config.clone();
        config.name = name.to_string();

        let session = Arc::new(ExecSession::new(&id, config));
        self.sessions.write().await.insert(id, session.clone());

        Ok(session)
    }

    /// Get a session by ID.
    pub async fn get(&self, id: &str) -> Option<Arc<ExecSession>> {
        self.sessions.read().await.get(id).cloned()
    }

    /// Get a session by name.
    pub async fn get_by_name(&self, name: &str) -> Option<Arc<ExecSession>> {
        self.sessions
            .read()
            .await
            .values()
            .find(|s| s.name() == name)
            .cloned()
    }

    /// List all sessions.
    pub async fn list(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;
        let mut infos = Vec::new();

        for session in sessions.values() {
            infos.push(session.info().await);
        }

        infos
    }

    /// Delete a session.
    pub async fn delete(&self, id: &str) -> bool {
        self.sessions.write().await.remove(id).is_some()
    }

    /// Get session count.
    pub async fn count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Clean up inactive sessions.
    pub async fn cleanup_inactive(&self, timeout: Duration) -> Vec<String> {
        let now = timestamp_now();
        let mut removed = Vec::new();

        let mut sessions = self.sessions.write().await;
        sessions.retain(|id, session| {
            let last = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async { *session.last_activity.read().await })
            });

            let inactive = Duration::from_secs(now.saturating_sub(last));
            if inactive > timeout {
                removed.push(id.clone());
                false
            } else {
                true
            }
        });

        removed
    }
}

impl Default for ExecSessionManager {
    fn default() -> Self {
        Self::new(100)
    }
}

/// Generate a unique session ID.
fn generate_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    format!("exec_{ts:x}")
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

    #[tokio::test]
    async fn test_exec_session() {
        let config = SessionConfig {
            name: "test".to_string(),
            ..Default::default()
        };

        let session = ExecSession::new("test-1", config);

        let result = session.execute("echo hello").await.unwrap();
        assert!(result.success());

        let history = session.history().await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].command, "echo hello");
    }

    #[tokio::test]
    async fn test_session_variables() {
        let session = ExecSession::new("test", SessionConfig::default());

        session.set_var("FOO", "bar").await;
        assert_eq!(session.get_var("FOO").await, Some("bar".to_string()));

        session.unset_var("FOO").await;
        assert!(session.get_var("FOO").await.is_none());
    }

    #[tokio::test]
    async fn test_session_manager() {
        let manager = ExecSessionManager::new(10);

        let session = manager.create("test").await.unwrap();
        assert_eq!(session.name(), "test");

        assert_eq!(manager.count().await, 1);

        let found = manager.get_by_name("test").await;
        assert!(found.is_some());
    }

    #[test]
    fn test_history_entry() {
        let entry = HistoryEntry {
            command: "ls".to_string(),
            exit_code: 0,
            duration_ms: 100,
            timestamp: 12345,
            cwd: PathBuf::from("/tmp"),
        };

        assert!(entry.success());
    }
}
