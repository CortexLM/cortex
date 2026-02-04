//! Session persistence storage for cortex-app-server.
//!
//! Saves sessions and message history to disk so they survive server restarts.
//! Uses file locking to prevent corruption when multiple server instances share
//! the same storage directory.

use std::fs;
use std::io::{BufRead, BufReader, BufWriter};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Stored session metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub id: String,
    pub model: String,
    pub cwd: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub title: Option<String>,
}

/// A message in the session history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub role: String, // "user" or "assistant"
    pub content: String,
    pub timestamp: i64,
    #[serde(default)]
    pub tool_calls: Vec<StoredToolCall>,
}

/// A tool call record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub output: Option<String>,
    pub success: bool,
    pub duration_ms: Option<u64>,
}

/// Session storage manager.
pub struct SessionStorage {
    sessions_dir: PathBuf,
    history_dir: PathBuf,
}

impl SessionStorage {
    /// Create a new session storage.
    pub fn new(base_dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();
        let sessions_dir = base_dir.join("sessions");
        let history_dir = base_dir.join("history");

        fs::create_dir_all(&sessions_dir)?;
        fs::create_dir_all(&history_dir)?;

        info!("Session storage initialized at {:?}", base_dir);

        Ok(Self {
            sessions_dir,
            history_dir,
        })
    }

    /// Get the default storage location (~/.cortex/app-server/).
    pub fn default_location() -> std::io::Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
        })?;
        let base_dir = home.join(".cortex").join("app-server");
        Self::new(base_dir)
    }

    /// Save a session to disk with exclusive file locking.
    ///
    /// Uses file locking to prevent concurrent write corruption when multiple
    /// server instances share the same storage directory.
    pub fn save_session(&self, session: &StoredSession) -> std::io::Result<()> {
        let path = self.session_path(&session.id);
        let file = fs::File::create(&path)?;

        // Acquire exclusive lock for writing
        file.lock_exclusive()?;

        let writer = BufWriter::new(&file);
        let result = serde_json::to_writer_pretty(writer, session).map_err(std::io::Error::other);

        // Lock is automatically released when file is dropped
        file.unlock()?;

        result?;
        debug!("Saved session {} to {:?}", session.id, path);
        Ok(())
    }

    /// Load a session from disk with shared file locking.
    ///
    /// Uses shared locking to allow concurrent reads while preventing
    /// reads during writes.
    pub fn load_session(&self, id: &str) -> std::io::Result<StoredSession> {
        let path = self.session_path(id);
        let file = fs::File::open(&path)?;

        // Acquire shared lock for reading
        file.lock_shared()?;

        let reader = BufReader::new(&file);
        let session: StoredSession = serde_json::from_reader(reader)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        file.unlock()?;
        Ok(session)
    }

    /// Delete a session from disk.
    pub fn delete_session(&self, id: &str) -> std::io::Result<()> {
        let session_path = self.session_path(id);
        if session_path.exists() {
            fs::remove_file(&session_path)?;
        }

        let history_path = self.history_path(id);
        if history_path.exists() {
            fs::remove_file(&history_path)?;
        }

        info!("Deleted session {}", id);
        Ok(())
    }

    /// List all stored sessions.
    pub fn list_sessions(&self) -> std::io::Result<Vec<StoredSession>> {
        let mut sessions = Vec::new();

        if !self.sessions_dir.exists() {
            return Ok(sessions);
        }

        for entry in fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e == "json").unwrap_or(false) {
                match self.load_session_from_path(&path) {
                    Ok(session) => sessions.push(session),
                    Err(e) => warn!("Failed to load session from {:?}: {}", path, e),
                }
            }
        }

        // Sort by updated_at descending (newest first)
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    /// Append a message to session history (JSONL format).
    ///
    /// This function uses file locking to prevent concurrent write corruption
    /// and ensures data durability by calling sync_all() (fsync) after writing.
    pub fn append_message(&self, session_id: &str, message: &StoredMessage) -> std::io::Result<()> {
        let path = self.history_path(session_id);
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        // Acquire exclusive lock for writing
        file.lock_exclusive()?;

        let json = serde_json::to_string(message).map_err(std::io::Error::other)?;

        // Write using a mutable reference to the locked file
        use std::io::Write;
        let mut writer = &file;
        writeln!(writer, "{}", json)?;

        // Ensure data is durably written to disk (fsync) to prevent data loss on crash
        file.sync_all()?;

        file.unlock()?;

        debug!("Appended message to session {} history", session_id);
        Ok(())
    }

    /// Read all messages from session history.
    pub fn read_history(&self, session_id: &str) -> std::io::Result<Vec<StoredMessage>> {
        let path = self.history_path(session_id);

        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(&path)?;
        let reader = BufReader::new(file);
        let mut messages = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if !line.trim().is_empty() {
                match serde_json::from_str::<StoredMessage>(&line) {
                    Ok(msg) => messages.push(msg),
                    Err(e) => warn!("Failed to parse message line: {}", e),
                }
            }
        }

        Ok(messages)
    }

    /// Update session title based on first user message.
    pub fn update_session_title(&self, session_id: &str, title: &str) -> std::io::Result<()> {
        let mut session = self.load_session(session_id)?;
        session.title = Some(title.to_string());
        session.updated_at = chrono::Utc::now().timestamp();
        self.save_session(&session)
    }

    /// Update session timestamp.
    pub fn touch_session(&self, session_id: &str) -> std::io::Result<()> {
        if let Ok(mut session) = self.load_session(session_id) {
            session.updated_at = chrono::Utc::now().timestamp();
            self.save_session(&session)?;
        }
        Ok(())
    }

    fn session_path(&self, id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{}.json", id))
    }

    fn history_path(&self, id: &str) -> PathBuf {
        self.history_dir.join(format!("{}.jsonl", id))
    }

    fn load_session_from_path(&self, path: &Path) -> std::io::Result<StoredSession> {
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);
        let session: StoredSession = serde_json::from_reader(reader)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(session)
    }
}

impl Default for SessionStorage {
    fn default() -> Self {
        Self::default_location().expect("Failed to create default session storage")
    }
}
