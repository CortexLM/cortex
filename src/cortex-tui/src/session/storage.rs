//! Session file storage operations.
//!
//! Handles reading and writing session data to the filesystem.
//! Uses a directory-per-session structure with atomic writes for safety.

use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use super::types::{SessionMeta, SessionSummary, StoredMessage};
use crate::providers::config::CortexConfig;

// ============================================================
// CONSTANTS
// ============================================================

/// Metadata file name.
const META_FILE: &str = "meta.json";

/// History file name.
const HISTORY_FILE: &str = "history.jsonl";

// ============================================================
// SECURITY HELPERS
// ============================================================

/// Sanitize a session ID to prevent path traversal attacks.
///
/// Only allows alphanumeric characters, hyphens, and underscores.
/// Any other characters (including path separators) are replaced with underscores.
fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Validate a session ID for safe filesystem use.
///
/// Returns true if the session_id contains only safe characters.
pub fn validate_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ============================================================
// SESSION STORAGE
// ============================================================

/// Handles session file operations.
pub struct SessionStorage {
    /// Base directory for all sessions.
    base_dir: PathBuf,
}

impl SessionStorage {
    /// Creates a new SessionStorage with the default directory.
    pub fn new() -> Result<Self> {
        let base_dir = CortexConfig::sessions_dir()?;
        Ok(Self { base_dir })
    }

    /// Creates a new SessionStorage with a custom directory.
    pub fn with_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Gets the base directory.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Gets the directory for a specific session.
    ///
    /// # Security
    ///
    /// The session_id is validated to prevent path traversal attacks.
    /// Only alphanumeric characters, hyphens, and underscores are allowed.
    ///
    /// # Panics
    ///
    /// This function does not panic but will return an invalid path if
    /// the session_id contains disallowed characters. Use `validate_session_id`
    /// before calling this function for untrusted input.
    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        // Sanitize session_id to prevent path traversal
        let sanitized_id = sanitize_session_id(session_id);
        self.base_dir.join(&sanitized_id)
    }

    /// Gets the metadata file path for a session.
    pub fn meta_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join(META_FILE)
    }

    /// Gets the history file path for a session.
    pub fn history_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join(HISTORY_FILE)
    }

    /// Checks if a session exists.
    pub fn exists(&self, session_id: &str) -> bool {
        self.meta_path(session_id).exists()
    }

    /// Ensures the base directory exists.
    pub fn ensure_base_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.base_dir)
            .with_context(|| format!("Failed to create sessions directory: {:?}", self.base_dir))
    }

    /// Ensures a session directory exists.
    pub fn ensure_session_dir(&self, session_id: &str) -> Result<PathBuf> {
        let dir = self.session_dir(session_id);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create session directory: {:?}", dir))?;
        Ok(dir)
    }

    // ========================================================================
    // METADATA OPERATIONS
    // ========================================================================

    /// Saves session metadata.
    ///
    /// Uses atomic write (temp file + rename) with fsync for durability.
    /// This prevents data loss on crash or forceful termination.
    pub fn save_meta(&self, meta: &SessionMeta) -> Result<()> {
        self.ensure_session_dir(&meta.id)?;

        let path = self.meta_path(&meta.id);
        let content =
            serde_json::to_string_pretty(meta).context("Failed to serialize session metadata")?;

        // Atomic write: write to temp file, fsync, then rename
        let temp_path = path.with_extension("json.tmp");

        // Write and sync temp file
        let file = File::create(&temp_path)
            .with_context(|| format!("Failed to create temp metadata file: {:?}", temp_path))?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(content.as_bytes())
            .with_context(|| format!("Failed to write temp metadata file: {:?}", temp_path))?;
        writer.flush()?;

        // Ensure data is durably written to disk (fsync) before rename
        writer.get_ref().sync_all().with_context(|| {
            format!("Failed to sync temp metadata file to disk: {:?}", temp_path)
        })?;

        // Rename temp file to final path
        fs::rename(&temp_path, &path)
            .with_context(|| format!("Failed to rename metadata file: {:?}", path))?;

        // Sync parent directory on Unix for crash safety (ensures directory entry is persisted)
        #[cfg(unix)]
        if let Some(parent) = path.parent()
            && let Ok(dir) = File::open(parent)
        {
            let _ = dir.sync_all();
        }

        Ok(())
    }

    /// Loads session metadata.
    pub fn load_meta(&self, session_id: &str) -> Result<SessionMeta> {
        let path = self.meta_path(session_id);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read metadata file: {:?}", path))?;
        let meta: SessionMeta = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse metadata file: {:?}", path))?;
        Ok(meta)
    }

    // ========================================================================
    // HISTORY OPERATIONS
    // ========================================================================

    /// Appends a message to the history file.
    ///
    /// This function ensures data durability by:
    /// 1. Flushing the BufWriter to the OS buffer
    /// 2. Calling sync_all() to force data to disk (fsync)
    ///
    /// This prevents data loss on crash or forceful termination.
    pub fn append_message(&self, session_id: &str, message: &StoredMessage) -> Result<()> {
        self.ensure_session_dir(session_id)?;

        let path = self.history_path(session_id);
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open history file: {:?}", path))?;

        let mut writer = BufWriter::new(file);
        let line = serde_json::to_string(message).context("Failed to serialize message")?;
        writeln!(writer, "{}", line)
            .with_context(|| format!("Failed to write to history file: {:?}", path))?;
        writer.flush()?;

        // Ensure data is durably written to disk (fsync) to prevent data loss on crash
        writer
            .get_ref()
            .sync_all()
            .with_context(|| format!("Failed to sync history file to disk: {:?}", path))?;

        Ok(())
    }

    /// Loads all messages from the history file.
    pub fn load_messages(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let path = self.history_path(session_id);

        if !path.exists() {
            return Ok(vec![]);
        }

        let file = File::open(&path)
            .with_context(|| format!("Failed to open history file: {:?}", path))?;
        let reader = BufReader::new(file);

        let mut messages = Vec::new();
        for (line_num, line_result) in reader.lines().enumerate() {
            let line = line_result.with_context(|| {
                format!("Failed to read line {} from history file", line_num + 1)
            })?;

            if line.trim().is_empty() {
                continue;
            }

            let message: StoredMessage = serde_json::from_str(&line)
                .with_context(|| format!("Failed to parse message at line {}", line_num + 1))?;
            messages.push(message);
        }

        Ok(messages)
    }

    /// Rewrites the entire history file (used for fork/compact/undo).
    pub fn rewrite_messages(&self, session_id: &str, messages: &[StoredMessage]) -> Result<()> {
        self.rewrite_history(session_id, messages)
    }

    /// Rewrites the entire history file (used for fork/compact).
    ///
    /// Uses atomic write (temp file + rename) with fsync for durability.
    pub fn rewrite_history(&self, session_id: &str, messages: &[StoredMessage]) -> Result<()> {
        self.ensure_session_dir(session_id)?;

        let path = self.history_path(session_id);
        let temp_path = path.with_extension("jsonl.tmp");

        let file = File::create(&temp_path)
            .with_context(|| format!("Failed to create temp history file: {:?}", temp_path))?;
        let mut writer = BufWriter::new(file);

        for message in messages {
            let line = serde_json::to_string(message).context("Failed to serialize message")?;
            writeln!(writer, "{}", line)?;
        }
        writer.flush()?;

        // Ensure data is durably written before rename
        writer.get_ref().sync_all().with_context(|| {
            format!("Failed to sync temp history file to disk: {:?}", temp_path)
        })?;

        fs::rename(&temp_path, &path)
            .with_context(|| format!("Failed to rename history file: {:?}", path))?;

        // Sync parent directory on Unix for crash safety (ensures directory entry is persisted)
        #[cfg(unix)]
        if let Some(parent) = path.parent()
            && let Ok(dir) = File::open(parent)
        {
            let _ = dir.sync_all();
        }

        Ok(())
    }

    // ========================================================================
    // SESSION LISTING
    // ========================================================================

    /// Lists all sessions (sorted by updated_at descending).
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.ensure_base_dir()?;

        let mut summaries = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && let Some(session_id) = path.file_name().and_then(|n| n.to_str())
                    && let Ok(meta) = self.load_meta(session_id)
                {
                    summaries.push(SessionSummary::from(&meta));
                }
            }
        }

        // Sort by updated_at descending (most recent first)
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        Ok(summaries)
    }

    /// Lists recent sessions (non-archived, limited count).
    pub fn list_recent_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        let all = self.list_sessions()?;
        Ok(all
            .into_iter()
            .filter(|s| !s.archived)
            .take(limit)
            .collect())
    }

    // ========================================================================
    // SESSION DELETION
    // ========================================================================

    /// Deletes a session and all its files.
    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let dir = self.session_dir(session_id);
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .with_context(|| format!("Failed to delete session directory: {:?}", dir))?;
        }
        Ok(())
    }

    /// Archives a session (sets archived flag in metadata).
    pub fn archive_session(&self, session_id: &str) -> Result<()> {
        let mut meta = self.load_meta(session_id)?;
        meta.archived = true;
        self.save_meta(&meta)?;
        Ok(())
    }

    /// Unarchives a session.
    pub fn unarchive_session(&self, session_id: &str) -> Result<()> {
        let mut meta = self.load_meta(session_id)?;
        meta.archived = false;
        self.save_meta(&meta)?;
        Ok(())
    }

    // ========================================================================
    // FORK OPERATIONS
    // ========================================================================

    /// Forks a session, copying messages up to a certain point.
    pub fn fork_session(
        &self,
        source_id: &str,
        new_meta: &SessionMeta,
        up_to_message_id: Option<&str>,
    ) -> Result<()> {
        // Load source messages
        let source_messages = self.load_messages(source_id)?;

        // Filter messages if a cutoff point is specified
        let messages_to_copy: Vec<StoredMessage> = if let Some(msg_id) = up_to_message_id {
            // Find the index of the cutoff message
            let mut result = Vec::new();
            for msg in source_messages {
                let is_target = msg.id == msg_id;
                result.push(msg);
                if is_target {
                    break;
                }
            }
            result
        } else {
            source_messages
        };

        // Save new session
        self.save_meta(new_meta)?;
        self.rewrite_history(&new_meta.id, &messages_to_copy)?;

        Ok(())
    }
}

impl Default for SessionStorage {
    fn default() -> Self {
        Self::new().expect("Failed to create session storage")
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_storage() -> (SessionStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = SessionStorage::with_dir(temp_dir.path().to_path_buf());
        (storage, temp_dir)
    }

    #[test]
    fn test_session_storage_paths() {
        let (storage, _temp) = create_test_storage();
        let session_id = "test-session-123";

        assert!(
            storage
                .session_dir(session_id)
                .ends_with("test-session-123")
        );
        assert!(storage.meta_path(session_id).ends_with("meta.json"));
        assert!(storage.history_path(session_id).ends_with("history.jsonl"));
    }

    #[test]
    fn test_save_and_load_meta() {
        let (storage, _temp) = create_test_storage();
        let meta = SessionMeta::new("cortex", "test-model");
        let session_id = meta.id.clone();

        storage.save_meta(&meta).unwrap();
        assert!(storage.exists(&session_id));

        let loaded = storage.load_meta(&session_id).unwrap();
        assert_eq!(loaded.id, meta.id);
        assert_eq!(loaded.provider, "cortex");
    }

    #[test]
    fn test_append_and_load_messages() {
        let (storage, _temp) = create_test_storage();
        let meta = SessionMeta::new("cortex", "test-model");
        let session_id = meta.id.clone();

        storage.save_meta(&meta).unwrap();

        let msg1 = StoredMessage::user("Hello!");
        let msg2 = StoredMessage::assistant("Hi there!");

        storage.append_message(&session_id, &msg1).unwrap();
        storage.append_message(&session_id, &msg2).unwrap();

        let messages = storage.load_messages(&session_id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "Hello!");
        assert_eq!(messages[1].content, "Hi there!");
    }

    #[test]
    fn test_list_sessions() {
        let (storage, _temp) = create_test_storage();

        // Create a few sessions
        for i in 0..3 {
            let meta = SessionMeta::new("cortex", &format!("model-{}", i));
            storage.save_meta(&meta).unwrap();
        }

        let sessions = storage.list_sessions().unwrap();
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn test_delete_session() {
        let (storage, _temp) = create_test_storage();
        let meta = SessionMeta::new("cortex", "test-model");
        let session_id = meta.id.clone();

        storage.save_meta(&meta).unwrap();
        assert!(storage.exists(&session_id));

        storage.delete_session(&session_id).unwrap();
        assert!(!storage.exists(&session_id));
    }

    #[test]
    fn test_archive_session() {
        let (storage, _temp) = create_test_storage();
        let meta = SessionMeta::new("cortex", "test-model");
        let session_id = meta.id.clone();

        storage.save_meta(&meta).unwrap();
        storage.archive_session(&session_id).unwrap();

        let loaded = storage.load_meta(&session_id).unwrap();
        assert!(loaded.archived);
    }

    #[test]
    fn test_validate_session_id() {
        // Valid IDs
        assert!(validate_session_id("abc-123"));
        assert!(validate_session_id("test_session"));
        assert!(validate_session_id("ABC123"));

        // Invalid IDs - path traversal attempts
        assert!(!validate_session_id("../../../etc"));
        assert!(!validate_session_id(".."));
        assert!(!validate_session_id("test/../passwd"));
        assert!(!validate_session_id("test/subdir"));
        assert!(!validate_session_id(""));
    }

    #[test]
    fn test_sanitize_session_id() {
        // Normal ID stays the same
        assert_eq!(sanitize_session_id("abc-123"), "abc-123");
        assert_eq!(sanitize_session_id("test_session"), "test_session");

        // Path traversal gets sanitized - verify no path separators remain
        // and that result ends with "etc" (exact underscore count may vary by platform)
        let sanitized = sanitize_session_id("../../../etc");
        assert!(!sanitized.contains('/'));
        assert!(!sanitized.contains('\\'));
        assert!(!sanitized.contains(".."));
        assert!(sanitized.ends_with("etc"));

        assert_eq!(sanitize_session_id("test/subdir"), "test_subdir");
        assert_eq!(sanitize_session_id("test\x00evil"), "test_evil");
    }

    #[test]
    fn test_session_dir_path_traversal() {
        let (storage, temp) = create_test_storage();
        let base_dir = temp.path().to_path_buf();

        // Attempt path traversal - should be sanitized
        let malicious_id = "../../../etc/passwd";
        let result_path = storage.session_dir(malicious_id);

        // The result should still be under base_dir, not escaping it
        assert!(result_path.starts_with(&base_dir));
        assert!(!result_path.to_string_lossy().contains(".."));
    }
}
