//! Session storage and retrieval with concurrent access protection.
//!
//! This module provides safe session storage using:
//! - RwLock for in-memory cache to allow concurrent reads
//! - Process-level mutex locks to prevent concurrent file modifications
//! - Atomic writes (write to temp file, then rename) for data integrity

use crate::{Result, ResumeError, SessionMeta, SessionSummary};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use tracing::{debug, info};

/// Maximum number of lock entries before triggering cleanup.
const MAX_LOCK_ENTRIES: usize = 10_000;

/// Global file lock manager for session store operations.
/// Prevents concurrent modifications to the same file within the process.
static FILE_LOCKS: once_cell::sync::Lazy<std::sync::Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

/// Remove lock entries that are no longer in use.
///
/// An entry is considered stale when only the HashMap holds a reference
/// to it (strong_count == 1), meaning no caller is currently using the lock.
fn cleanup_stale_file_locks(locks: &mut HashMap<PathBuf, Arc<AsyncMutex<()>>>) {
    locks.retain(|_, arc| Arc::strong_count(arc) > 1);
}

/// Acquire an async lock for a specific file path.
///
/// Automatically cleans up stale lock entries when the map grows too large.
fn get_file_lock(path: &Path) -> Arc<AsyncMutex<()>> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut locks = FILE_LOCKS.lock().unwrap();

    // Clean up stale entries if the map is getting large
    if locks.len() >= MAX_LOCK_ENTRIES {
        cleanup_stale_file_locks(&mut locks);
    }

    locks
        .entry(canonical)
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

/// Perform an atomic write operation: write to temp file, then rename.
/// This ensures readers never see partial content.
async fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Cannot determine parent directory",
        )
    })?;

    // Ensure parent directory exists
    if !parent.exists() {
        fs::create_dir_all(parent).await?;
    }

    // Create temp file name in same directory for same-filesystem rename
    let temp_path = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));

    // Write content to temp file
    fs::write(&temp_path, content).await?;

    // Sync to ensure durability (use OpenOptions for cross-platform compatibility)
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temp_path)?;
    file.sync_all()?;
    drop(file);

    // Atomic rename
    #[cfg(unix)]
    {
        fs::rename(&temp_path, path).await.map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            e
        })?;
    }

    #[cfg(windows)]
    {
        // Windows may need target removed first
        if path.exists() {
            let mut retries = 3;
            loop {
                match fs::remove_file(path).await {
                    Ok(()) => break,
                    Err(_) if retries > 0 => {
                        retries -= 1;
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                    }
                    Err(e) => {
                        let _ = fs::remove_file(&temp_path).await;
                        return Err(e);
                    }
                }
            }
        }
        fs::rename(&temp_path, path).await.map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            e
        })?;
    }

    Ok(())
}

/// Store for session data with thread-safe concurrent access.
pub struct SessionStore {
    /// Base directory for sessions.
    base_dir: PathBuf,
    /// Archived sessions directory.
    archive_dir: PathBuf,
    /// Cache of session metadata with RwLock for concurrent read access.
    cache: RwLock<HashMap<String, SessionMeta>>,
}

impl SessionStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        let archive_dir = base_dir.join("archived");
        Self {
            base_dir,
            archive_dir,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Initialize the store (create directories if needed).
    pub async fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.base_dir).await?;
        fs::create_dir_all(&self.archive_dir).await?;
        Ok(())
    }

    /// List all sessions (newest first).
    pub async fn list_sessions(&self, include_archived: bool) -> Result<Vec<SessionSummary>> {
        let mut sessions = Vec::new();

        // Clone paths to avoid borrow issues
        let base_dir = self.base_dir.clone();
        let archive_dir = self.archive_dir.clone();

        // Read active sessions
        sessions.extend(self.read_sessions_from_dir(&base_dir).await?);

        // Read archived sessions if requested
        if include_archived {
            sessions.extend(self.read_sessions_from_dir(&archive_dir).await?);
        }

        // Sort by last used (newest first)
        sessions.sort_by(|a, b| b.last_used.cmp(&a.last_used));

        Ok(sessions)
    }

    /// Read sessions from a directory.
    async fn read_sessions_from_dir(&self, dir: &Path) -> Result<Vec<SessionSummary>> {
        let mut summaries = Vec::new();

        if !dir.exists() {
            return Ok(summaries);
        }

        let mut entries = fs::read_dir(dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if path.is_dir() {
                if let Some(meta) = self.load_session_meta(&path).await {
                    let preview = self.get_session_preview(&path).await;
                    summaries.push(meta.to_summary(preview.clone()));
                    // Use write lock to update cache
                    let mut cache = self.cache.write().await;
                    cache.insert(meta.id.clone(), meta);
                }
            }
        }

        Ok(summaries)
    }

    /// Load session metadata from a directory.
    async fn load_session_meta(&self, session_dir: &Path) -> Option<SessionMeta> {
        let meta_path = session_dir.join("meta.json");

        if !meta_path.exists() {
            return None;
        }

        let content = fs::read_to_string(&meta_path).await.ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Get a preview of the session (first user message).
    async fn get_session_preview(&self, session_dir: &Path) -> Option<String> {
        let history_path = session_dir.join("history.jsonl");

        if !history_path.exists() {
            return None;
        }

        let content = fs::read_to_string(&history_path).await.ok()?;

        // Get first line and extract text
        if let Some(first_line) = content.lines().next() {
            if let Ok(item) = serde_json::from_str::<serde_json::Value>(first_line) {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    let preview: String = text.chars().take(100).collect();
                    return Some(if text.len() > 100 {
                        format!("{}...", preview)
                    } else {
                        preview
                    });
                }
            }
        }

        None
    }

    /// Get session by ID.
    pub async fn get_session(&self, id: &str) -> Result<SessionMeta> {
        // Check cache first (using read lock for performance)
        {
            let cache = self.cache.read().await;
            if let Some(meta) = cache.get(id) {
                return Ok(meta.clone());
            }
        }

        // Try active sessions
        let session_dir = self.base_dir.join(id);
        if session_dir.exists() {
            if let Some(meta) = self.load_session_meta(&session_dir).await {
                let mut cache = self.cache.write().await;
                cache.insert(id.to_string(), meta.clone());
                return Ok(meta);
            }
        }

        // Try archived sessions
        let archive_dir = self.archive_dir.join(id);
        if archive_dir.exists() {
            if let Some(meta) = self.load_session_meta(&archive_dir).await {
                let mut cache = self.cache.write().await;
                cache.insert(id.to_string(), meta.clone());
                return Ok(meta);
            }
        }

        Err(ResumeError::SessionNotFound(id.to_string()))
    }

    /// Get the most recent session.
    pub async fn get_last_session(&self) -> Result<Option<SessionMeta>> {
        let sessions = self.list_sessions(false).await?;

        if let Some(summary) = sessions.first() {
            return Ok(Some(self.get_session(&summary.id).await?));
        }

        Ok(None)
    }

    /// Save session metadata with file locking and atomic write.
    pub async fn save_session(&self, meta: &SessionMeta) -> Result<()> {
        let session_dir = self.base_dir.join(&meta.id);
        fs::create_dir_all(&session_dir).await?;

        let meta_path = session_dir.join("meta.json");

        // Acquire file lock to prevent concurrent writes
        let lock = get_file_lock(&meta_path);
        let _guard = lock.lock().await;

        let content =
            serde_json::to_string_pretty(meta).map_err(|e| ResumeError::Parse(e.to_string()))?;

        // Use atomic write to prevent partial writes
        atomic_write(&meta_path, content.as_bytes()).await?;

        // Update cache with write lock
        let mut cache = self.cache.write().await;
        cache.insert(meta.id.clone(), meta.clone());

        debug!("Saved session metadata: {}", meta.id);
        Ok(())
    }

    /// Archive a session with proper locking.
    pub async fn archive_session(&self, id: &str) -> Result<()> {
        let source = self.base_dir.join(id);
        let dest = self.archive_dir.join(id);

        // Acquire locks for both source and destination
        let source_lock = get_file_lock(&source);
        let dest_lock = get_file_lock(&dest);

        // Always acquire locks in a consistent order to prevent deadlocks
        let (_guard1, _guard2) = if source < dest {
            (source_lock.lock().await, dest_lock.lock().await)
        } else {
            let g2 = dest_lock.lock().await;
            let g1 = source_lock.lock().await;
            (g1, g2)
        };

        if !source.exists() {
            return Err(ResumeError::SessionNotFound(id.to_string()));
        }

        fs::rename(&source, &dest).await?;

        // Update metadata in cache
        if let Ok(mut meta) = self.get_session(id).await {
            meta.archived = true;
            let mut cache = self.cache.write().await;
            cache.insert(id.to_string(), meta);
        }

        info!("Archived session: {}", id);
        Ok(())
    }

    /// Delete a session permanently with proper locking.
    pub async fn delete_session(&self, id: &str) -> Result<()> {
        // Try active directory
        let active_dir = self.base_dir.join(id);
        if active_dir.exists() {
            let lock = get_file_lock(&active_dir);
            let _guard = lock.lock().await;

            fs::remove_dir_all(&active_dir).await?;
            let mut cache = self.cache.write().await;
            cache.remove(id);
            info!("Deleted session: {}", id);
            return Ok(());
        }

        // Try archived directory
        let archive_dir = self.archive_dir.join(id);
        if archive_dir.exists() {
            let lock = get_file_lock(&archive_dir);
            let _guard = lock.lock().await;

            fs::remove_dir_all(&archive_dir).await?;
            let mut cache = self.cache.write().await;
            cache.remove(id);
            info!("Deleted archived session: {}", id);
            return Ok(());
        }

        Err(ResumeError::SessionNotFound(id.to_string()))
    }

    /// Get session directory path.
    pub fn get_session_dir(&self, id: &str) -> PathBuf {
        self.base_dir.join(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_session_store() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        store.init().await.unwrap();

        // Create and save a session - use tempdir for cross-platform compatibility
        let session_cwd = std::env::temp_dir();
        let meta = SessionMeta::new("test-session", session_cwd).with_title("Test Session");

        store.save_session(&meta).await.unwrap();

        // Retrieve it
        let retrieved = store.get_session("test-session").await.unwrap();
        assert_eq!(retrieved.id, "test-session");

        // List sessions
        let sessions = store.list_sessions(false).await.unwrap();
        assert_eq!(sessions.len(), 1);
    }
}
