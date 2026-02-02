//! Session persistence (rollout files).
//!
//! Stores conversation history in JSONL format for resumption.

pub mod reader;
pub mod recorder;

pub use reader::{RolloutEntry, read_rollout};
pub use recorder::RolloutRecorder;

use std::path::PathBuf;

use cortex_protocol::ConversationId;

/// Sessions subdirectory name.
pub const SESSIONS_SUBDIR: &str = "sessions";

/// Archived sessions subdirectory name.
pub const ARCHIVED_SESSIONS_SUBDIR: &str = "archived";

/// Get the rollout file path for a conversation.
pub fn get_rollout_path(cortex_home: &PathBuf, conversation_id: &ConversationId) -> PathBuf {
    cortex_home
        .join(SESSIONS_SUBDIR)
        .join(format!("{conversation_id}.jsonl"))
}
