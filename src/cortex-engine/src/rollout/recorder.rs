//! Rollout recorder for session persistence.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};
use cortex_protocol::{ConversationId, Event, EventMsg};
use serde::Serialize;
use tracing::warn;

use super::SESSIONS_SUBDIR;
use crate::error::Result;

/// Warning threshold for session file size (50MB).
pub const SESSION_SIZE_WARNING_THRESHOLD: u64 = 50 * 1024 * 1024;

/// Maximum tool output size before truncation warning (100KB).
pub const TOOL_OUTPUT_SIZE_WARNING: usize = 100 * 1024;

/// Records session events to a rollout file.
pub struct RolloutRecorder {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    conversation_id: ConversationId,
}

impl RolloutRecorder {
    /// Create a new rollout recorder.
    pub fn new(cortex_home: &PathBuf, conversation_id: ConversationId) -> Result<Self> {
        let sessions_dir = cortex_home.join(SESSIONS_SUBDIR);
        std::fs::create_dir_all(&sessions_dir)?;

        let path = sessions_dir.join(format!("{conversation_id}.jsonl"));

        Ok(Self {
            path,
            writer: None,
            conversation_id,
        })
    }

    /// Get the rollout file path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Get the conversation ID.
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// Initialize the recorder (create/open file).
    pub fn init(&mut self) -> Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        self.writer = Some(BufWriter::new(file));
        Ok(())
    }

    /// Record a session metadata entry.
    pub fn record_meta(&mut self, meta: &SessionMeta) -> Result<()> {
        self.write_entry(&RolloutLine {
            // Use microsecond precision for unambiguous message ordering
            timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
            item: RolloutItem::SessionMeta(meta.clone()),
        })
    }

    /// Record an event.
    pub fn record_event(&mut self, event: &Event) -> Result<()> {
        self.write_entry(&RolloutLine {
            // Use microsecond precision for unambiguous message ordering
            timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
            item: RolloutItem::EventMsg(event.msg.clone()),
        })
    }

    /// Flush the writer.
    pub fn flush(&mut self) -> Result<()> {
        if let Some(writer) = &mut self.writer {
            writer.flush()?;
        }
        Ok(())
    }

    /// Get the current session file size.
    pub fn file_size(&self) -> Result<u64> {
        if self.path.exists() {
            Ok(std::fs::metadata(&self.path)?.len())
        } else {
            Ok(0)
        }
    }

    /// Check if the session file has grown large and emit a warning if needed.
    /// Returns true if the session file is larger than the warning threshold.
    pub fn check_size_warning(&self) -> bool {
        if let Ok(size) = self.file_size() {
            if size > SESSION_SIZE_WARNING_THRESHOLD {
                let size_mb = size as f64 / (1024.0 * 1024.0);
                warn!(
                    session_id = %self.conversation_id,
                    size_mb = format!("{:.1}", size_mb),
                    "Session file is large ({:.1}MB). Consider starting a new session or using /compact to reduce size.",
                    size_mb
                );
                return true;
            }
        }
        false
    }

    fn write_entry<T: Serialize>(&mut self, entry: &T) -> Result<()> {
        if self.writer.is_none() {
            self.init()?;
        }

        if let Some(writer) = &mut self.writer {
            let json = serde_json::to_string(entry)?;
            writeln!(writer, "{json}")?;

            // Periodically check session size (every 100KB of new data approximately)
            // We check the size to warn users about unbounded growth
            let _ = self.check_size_warning();
        }
        Ok(())
    }
}

impl Drop for RolloutRecorder {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// Session metadata.
#[derive(Debug, Clone, Serialize)]
pub struct SessionMeta {
    pub id: ConversationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<ConversationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fork_point: Option<String>,
    pub timestamp: String,
    pub cwd: PathBuf,
    pub model: String,
    pub cli_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Rollout line entry.
#[derive(Debug, Serialize)]
struct RolloutLine {
    timestamp: String,
    #[serde(flatten)]
    item: RolloutItem,
}

/// Rollout item types.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
enum RolloutItem {
    SessionMeta(SessionMeta),
    EventMsg(EventMsg),
}
