//! Session storage operations.
//!
//! The `SessionStorage` struct provides CRUD operations for sessions
//! and message history, with both async and sync variants.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use tokio::fs;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{Result, StorageError};
use crate::paths::CortexPaths;

use super::query::SessionQuery;
use super::types::{SessionSummary, ShareInfo, StoredMessage, StoredSession};

/// Centralized session storage manager.
#[derive(Debug, Clone)]
pub struct SessionStorage {
    paths: CortexPaths,
}

impl SessionStorage {
    /// Create a new session storage with automatic path detection.
    pub fn new() -> Result<Self> {
        let paths = CortexPaths::new()?;
        Ok(Self { paths })
    }

    /// Create session storage with custom paths.
    pub fn with_paths(paths: CortexPaths) -> Self {
        Self { paths }
    }

    /// Initialize storage (create directories).
    pub async fn init(&self) -> Result<()> {
        self.paths.ensure_dirs_async().await?;
        info!(data_dir = %self.paths.data_dir.display(), "Session storage initialized");
        Ok(())
    }

    /// Initialize storage synchronously.
    pub fn init_sync(&self) -> Result<()> {
        self.paths.ensure_dirs()?;
        info!(data_dir = %self.paths.data_dir.display(), "Session storage initialized");
        Ok(())
    }

    /// Get the underlying paths.
    pub fn paths(&self) -> &CortexPaths {
        &self.paths
    }

    // ========================================================================
    // Session CRUD operations
    // ========================================================================

    /// List all sessions, sorted by most recent first.
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let mut sessions = Vec::new();

        if !self.paths.sessions_dir.exists() {
            return Ok(sessions);
        }

        let mut entries = fs::read_dir(&self.paths.sessions_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                match self.load_session_from_path(&path).await {
                    Ok(session) => sessions.push(session.into()),
                    Err(e) => warn!(path = %path.display(), error = %e, "Failed to load session"),
                }
            }
        }

        // Sort by updated_at descending (newest first)
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    /// List all sessions synchronously.
    pub fn list_sessions_sync(&self) -> Result<Vec<SessionSummary>> {
        let mut sessions = Vec::new();

        if !self.paths.sessions_dir.exists() {
            return Ok(sessions);
        }

        for entry in std::fs::read_dir(&self.paths.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                match self.load_session_from_path_sync(&path) {
                    Ok(session) => sessions.push(session.into()),
                    Err(e) => warn!(path = %path.display(), error = %e, "Failed to load session"),
                }
            }
        }

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    /// Get a session by ID.
    pub async fn get_session(&self, id: &str) -> Result<StoredSession> {
        let path = self.paths.session_path(id);
        if !path.exists() {
            return Err(StorageError::SessionNotFound(id.to_string()));
        }
        self.load_session_from_path(&path).await
    }

    /// Get a session by ID synchronously.
    pub fn get_session_sync(&self, id: &str) -> Result<StoredSession> {
        let path = self.paths.session_path(id);
        if !path.exists() {
            return Err(StorageError::SessionNotFound(id.to_string()));
        }
        self.load_session_from_path_sync(&path)
    }

    /// Save a session to disk.
    ///
    /// This function ensures data durability by calling sync_all() (fsync)
    /// after writing to prevent data loss on crash or forceful termination.
    pub async fn save_session(&self, session: &StoredSession) -> Result<()> {
        let path = self.paths.session_path(&session.id);
        let content = serde_json::to_string_pretty(session)?;

        // Write content to file
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .await?;

        use tokio::io::AsyncWriteExt;
        let mut file = file;
        file.write_all(content.as_bytes()).await?;
        file.flush().await?;

        // Ensure data is durably written to disk (fsync) to prevent data loss on crash
        file.sync_all().await?;

        // Sync parent directory on Unix for crash safety (ensures directory entry is persisted)
        #[cfg(unix)]
        {
            if let Some(parent) = path.parent() {
                if let Ok(dir) = fs::File::open(parent).await {
                    let _ = dir.sync_all().await;
                }
            }
        }

        debug!(session_id = %session.id, "Session saved");
        Ok(())
    }

    /// Save a session synchronously.
    ///
    /// This function ensures data durability by calling sync_all() (fsync)
    /// after writing to prevent data loss on crash or forceful termination.
    pub fn save_session_sync(&self, session: &StoredSession) -> Result<()> {
        let path = self.paths.session_path(&session.id);
        let file = std::fs::File::create(&path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, session)?;
        writer.flush()?;

        // Ensure data is durably written to disk (fsync) to prevent data loss on crash
        writer.get_ref().sync_all()?;

        // Sync parent directory on Unix for crash safety (ensures directory entry is persisted)
        #[cfg(unix)]
        {
            if let Some(parent) = path.parent() {
                if let Ok(dir) = std::fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
        }

        debug!(session_id = %session.id, "Session saved");
        Ok(())
    }

    /// Delete a session and its history.
    pub async fn delete_session(&self, id: &str) -> Result<()> {
        let session_path = self.paths.session_path(id);
        let history_path = self.paths.history_path(id);

        if session_path.exists() {
            fs::remove_file(&session_path).await?;
        }
        if history_path.exists() {
            fs::remove_file(&history_path).await?;
        }

        info!(session_id = %id, "Session deleted");
        Ok(())
    }

    /// Delete a session synchronously.
    pub fn delete_session_sync(&self, id: &str) -> Result<()> {
        let session_path = self.paths.session_path(id);
        let history_path = self.paths.history_path(id);

        if session_path.exists() {
            std::fs::remove_file(&session_path)?;
        }
        if history_path.exists() {
            std::fs::remove_file(&history_path)?;
        }

        info!(session_id = %id, "Session deleted");
        Ok(())
    }

    /// Update session title.
    pub async fn update_title(&self, id: &str, title: &str) -> Result<()> {
        let mut session = self.get_session(id).await?;
        session.title = Some(title.to_string());
        session.touch();
        self.save_session(&session).await
    }

    /// Touch session (update timestamp).
    pub async fn touch_session(&self, id: &str) -> Result<()> {
        if let Ok(mut session) = self.get_session(id).await {
            session.touch();
            self.save_session(&session).await?;
        }
        Ok(())
    }

    // ========================================================================
    // Message history operations
    // ========================================================================

    /// Append a message to session history (JSONL format).
    ///
    /// This function ensures data durability by calling sync_all() (fsync)
    /// after writing to prevent data loss on crash or forceful termination.
    pub async fn append_message(&self, session_id: &str, message: &StoredMessage) -> Result<()> {
        let path = self.paths.history_path(session_id);
        let json = serde_json::to_string(message)?;

        // Append line to file
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        use tokio::io::AsyncWriteExt;
        file.write_all(json.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;

        // Ensure data is durably written to disk (fsync) to prevent data loss on crash
        file.sync_all().await?;

        debug!(session_id = %session_id, message_id = %message.id, "Message appended");
        Ok(())
    }

    /// Append a message synchronously.
    ///
    /// This function ensures data durability by calling sync_all() (fsync)
    /// after writing to prevent data loss on crash or forceful termination.
    pub fn append_message_sync(&self, session_id: &str, message: &StoredMessage) -> Result<()> {
        let path = self.paths.history_path(session_id);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        let json = serde_json::to_string(message)?;
        writeln!(file, "{}", json)?;

        // Ensure data is durably written to disk (fsync) to prevent data loss on crash
        file.sync_all()?;

        debug!(session_id = %session_id, message_id = %message.id, "Message appended");
        Ok(())
    }

    /// Read all messages from session history.
    pub async fn get_history(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let path = self.paths.history_path(session_id);

        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&path).await?;
        self.parse_history(&content)
    }

    /// Read history synchronously.
    pub fn get_history_sync(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let path = self.paths.history_path(session_id);

        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = std::fs::File::open(&path)?;
        let reader = BufReader::new(file);
        let mut messages = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if !line.trim().is_empty() {
                match serde_json::from_str::<StoredMessage>(&line) {
                    Ok(msg) => messages.push(msg),
                    Err(e) => warn!(error = %e, "Failed to parse message line"),
                }
            }
        }

        Ok(messages)
    }

    // ========================================================================
    // Query operations
    // ========================================================================

    /// Query sessions with filters.
    pub async fn query_sessions(&self, query: &SessionQuery) -> Result<Vec<SessionSummary>> {
        let all_sessions = self.list_sessions().await?;

        // Filter sessions
        let mut filtered: Vec<_> = all_sessions
            .into_iter()
            .filter(|s| query.matches(s))
            .collect();

        // Sort
        query.apply_sort(&mut filtered);

        // Apply pagination
        Ok(query.apply_pagination(filtered))
    }

    /// Query sessions synchronously.
    pub fn query_sessions_sync(&self, query: &SessionQuery) -> Result<Vec<SessionSummary>> {
        let all_sessions = self.list_sessions_sync()?;

        // Filter sessions
        let mut filtered: Vec<_> = all_sessions
            .into_iter()
            .filter(|s| query.matches(s))
            .collect();

        // Sort
        query.apply_sort(&mut filtered);

        // Apply pagination
        Ok(query.apply_pagination(filtered))
    }

    // ========================================================================
    // Favorites, tags, and sharing operations
    // ========================================================================

    /// Toggle favorite status for a session.
    pub async fn toggle_favorite(&self, id: &str) -> Result<bool> {
        let mut session = self.get_session(id).await?;
        let new_status = session.toggle_favorite();
        self.save_session(&session).await?;
        info!(session_id = %id, is_favorite = new_status, "Toggled favorite status");
        Ok(new_status)
    }

    /// Toggle favorite synchronously.
    pub fn toggle_favorite_sync(&self, id: &str) -> Result<bool> {
        let mut session = self.get_session_sync(id)?;
        let new_status = session.toggle_favorite();
        self.save_session_sync(&session)?;
        info!(session_id = %id, is_favorite = new_status, "Toggled favorite status");
        Ok(new_status)
    }

    /// Add a tag to a session.
    pub async fn add_tag(&self, id: &str, tag: &str) -> Result<()> {
        let mut session = self.get_session(id).await?;
        session.add_tag(tag);
        self.save_session(&session).await?;
        debug!(session_id = %id, tag = tag, "Added tag to session");
        Ok(())
    }

    /// Remove a tag from a session.
    pub async fn remove_tag(&self, id: &str, tag: &str) -> Result<bool> {
        let mut session = self.get_session(id).await?;
        let removed = session.remove_tag(tag);
        if removed {
            self.save_session(&session).await?;
            debug!(session_id = %id, tag = tag, "Removed tag from session");
        }
        Ok(removed)
    }

    /// Create a share link for a session.
    ///
    /// Generates a secure token and constructs a share URL.
    /// The share can optionally expire after a specified duration.
    pub async fn share_session(&self, id: &str, expires_in: Option<Duration>) -> Result<ShareInfo> {
        let mut session = self.get_session(id).await?;

        // Generate a secure token
        let token = generate_share_token();
        let url = format!("https://cortex.ai/share/{}", token);

        session.set_share(token.clone(), url.clone(), expires_in);
        self.save_session(&session).await?;

        let share_info = session.share_info.clone().unwrap();
        info!(session_id = %id, share_token = %token, "Created share link");
        Ok(share_info)
    }

    /// Remove share link from a session.
    pub async fn unshare_session(&self, id: &str) -> Result<()> {
        let mut session = self.get_session(id).await?;
        session.unshare();
        self.save_session(&session).await?;
        info!(session_id = %id, "Removed share link");
        Ok(())
    }

    /// Get share info for a session if it exists and is valid.
    pub async fn get_share_info(&self, id: &str) -> Result<Option<ShareInfo>> {
        let session = self.get_session(id).await?;
        Ok(session.share_info.filter(|s| s.is_valid()))
    }

    /// List favorite sessions.
    pub async fn list_favorites(&self) -> Result<Vec<SessionSummary>> {
        self.query_sessions(&SessionQuery::new().favorites()).await
    }

    /// List sessions with a specific tag.
    pub async fn list_by_tag(&self, tag: &str) -> Result<Vec<SessionSummary>> {
        self.query_sessions(&SessionQuery::new().with_tag(tag))
            .await
    }

    // ========================================================================
    // Private helpers
    // ========================================================================

    async fn load_session_from_path(&self, path: &Path) -> Result<StoredSession> {
        let content = fs::read_to_string(path).await?;
        let session: StoredSession = serde_json::from_str(&content)?;
        Ok(session)
    }

    fn load_session_from_path_sync(&self, path: &Path) -> Result<StoredSession> {
        let file = std::fs::File::open(path)?;
        let reader = BufReader::new(file);
        let session: StoredSession = serde_json::from_reader(reader)?;
        Ok(session)
    }

    fn parse_history(&self, content: &str) -> Result<Vec<StoredMessage>> {
        let mut messages = Vec::new();
        for line in content.lines() {
            if !line.trim().is_empty() {
                match serde_json::from_str::<StoredMessage>(line) {
                    Ok(msg) => messages.push(msg),
                    Err(e) => warn!(error = %e, "Failed to parse message line"),
                }
            }
        }
        Ok(messages)
    }
}

impl Default for SessionStorage {
    fn default() -> Self {
        Self::new().expect("Failed to create session storage")
    }
}

/// Generate a cryptographically secure share token.
fn generate_share_token() -> String {
    // Use UUID v4 which provides cryptographic randomness
    Uuid::new_v4().to_string().replace("-", "")
}
