//! Session management utilities for the run command.

use anyhow::{Result, bail};
use std::path::PathBuf;

use cortex_engine::list_sessions;
use cortex_engine::rollout::get_rollout_path;
use cortex_protocol::ConversationId;

/// Session handling mode.
#[derive(Debug)]
pub enum SessionMode {
    /// Continue the most recent session.
    ContinueLast,
    /// Continue a specific session by ID.
    Continue(String),
    /// Create a new session with optional title.
    New { title: Option<String> },
}

/// Resolve and validate a session ID, supporting both full UUID and 8-char short IDs.
/// Returns the full ConversationId if the session exists.
pub fn resolve_session_id(session_id: &str, cortex_home: &PathBuf) -> Result<ConversationId> {
    // Check for empty session ID first (#2843)
    if session_id.is_empty() {
        bail!(
            "Session ID cannot be empty. Use 'cortex sessions' to list available sessions, or 'cortex run -c' to continue the last session."
        );
    }

    // Validate session ID format - must contain only alphanumeric, hyphens, and underscores
    if !session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!(
            "Invalid session ID format. Session IDs must contain only alphanumeric characters, hyphens, and underscores. Example: 'my-session-123' or '550e8400-e29b-41d4-a716-446655440000'"
        );
    }

    // Try to parse as full UUID first
    let conversation_id: ConversationId = match session_id.parse() {
        Ok(id) => id,
        Err(_) => {
            // If parsing failed, check if it's a short ID (8 chars)
            if session_id.len() == 8 {
                // Try to find a session with matching prefix
                let sessions = list_sessions(cortex_home)?;
                let matching: Vec<_> = sessions
                    .iter()
                    .filter(|s| s.id.starts_with(session_id))
                    .collect();

                match matching.len() {
                    0 => bail!(
                        "No session found with ID prefix '{}'. Use 'cortex sessions' to list available sessions.",
                        session_id
                    ),
                    1 => matching[0].id.parse().map_err(|_| {
                        anyhow::anyhow!("Internal error: invalid session ID format")
                    })?,
                    _ => bail!(
                        "Ambiguous session ID prefix '{}' matches {} sessions. Please provide more characters or use the full UUID.",
                        session_id,
                        matching.len()
                    ),
                }
            } else {
                bail!(
                    "Invalid session ID: '{}'. Expected a full UUID (e.g., '550e8400-e29b-41d4-a716-446655440000') or an 8-character prefix.",
                    session_id
                );
            }
        }
    };

    // Verify the session exists by checking if the rollout file exists
    let rollout_path = get_rollout_path(cortex_home, &conversation_id);
    if !rollout_path.exists() {
        bail!(
            "Session not found: '{}'. The session may have been deleted. Use 'cortex sessions' to list available sessions.",
            session_id
        );
    }

    Ok(conversation_id)
}
