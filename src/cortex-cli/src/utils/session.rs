//! Session ID resolution and validation utilities.
//!
//! Provides unified session ID handling across all CLI commands,
//! supporting both full UUIDs and 8-character short IDs.

use std::path::Path;

use cortex_engine::{list_sessions, rollout::get_rollout_path};
use cortex_protocol::ConversationId;
use thiserror::Error;

/// Errors that can occur during session ID resolution.
#[derive(Debug, Error)]
pub enum SessionIdError {
    /// Session ID is empty.
    #[error(
        "Session ID cannot be empty. Use 'cortex sessions' to list available sessions, or 'cortex run -c' to continue the last session."
    )]
    Empty,

    /// Session ID contains invalid characters.
    #[error(
        "Invalid session ID format. Session IDs must contain only alphanumeric characters, hyphens, and underscores. Example: 'my-session-123' or '550e8400-e29b-41d4-a716-446655440000'"
    )]
    InvalidCharacters,

    /// No session found with the given ID or prefix.
    #[error(
        "No session found with ID prefix '{0}'. Use 'cortex sessions' to list available sessions."
    )]
    NotFound(String),

    /// Multiple sessions match the given prefix.
    #[error(
        "Ambiguous session ID prefix '{0}' matches {1} sessions. Please provide more characters or use the full UUID."
    )]
    Ambiguous(String, usize),

    /// Invalid UUID format.
    #[error(
        "Invalid session ID: '{0}'. Expected a full UUID (e.g., '550e8400-e29b-41d4-a716-446655440000') or an 8-character prefix."
    )]
    InvalidFormat(String),

    /// Session file does not exist.
    #[error(
        "Session not found: '{0}'. The session may have been deleted. Use 'cortex sessions' to list available sessions."
    )]
    SessionNotFound(String),

    /// Error listing sessions.
    #[error("Failed to list sessions: {0}")]
    ListError(String),
}

/// Resolve and validate a session ID, supporting both full UUID and 8-char short IDs.
///
/// This function provides unified session ID resolution across all CLI commands.
/// It handles:
/// - Full UUID format (e.g., "550e8400-e29b-41d4-a716-446655440000")
/// - Short 8-character prefixes (e.g., "550e8400")
/// - Validation that the session exists
///
/// # Arguments
/// * `session_id` - The session ID to resolve (full UUID or 8-char prefix)
/// * `cortex_home` - Path to the cortex home directory
///
/// # Returns
/// The full `ConversationId` if the session exists, or an error.
///
/// # Examples
/// ```ignore
/// let id = resolve_session_id("550e8400", &cortex_home)?;
/// let id = resolve_session_id("550e8400-e29b-41d4-a716-446655440000", &cortex_home)?;
/// ```
pub fn resolve_session_id(
    session_id: &str,
    cortex_home: &Path,
) -> Result<ConversationId, SessionIdError> {
    let cortex_home = cortex_home.to_path_buf();
    // Check for empty session ID first
    if session_id.is_empty() {
        return Err(SessionIdError::Empty);
    }

    // Validate session ID format - must contain only alphanumeric, hyphens, and underscores
    if !session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(SessionIdError::InvalidCharacters);
    }

    // Try to parse as full UUID first
    let conversation_id: ConversationId = match session_id.parse() {
        Ok(id) => id,
        Err(_) => {
            // If parsing failed, check if it's a short ID (8 chars)
            if session_id.len() == 8 {
                // Try to find a session with matching prefix
                let sessions = list_sessions(&cortex_home)
                    .map_err(|e| SessionIdError::ListError(e.to_string()))?;

                let matching: Vec<_> = sessions
                    .iter()
                    .filter(|s| s.id.starts_with(session_id))
                    .collect();

                match matching.len() {
                    0 => return Err(SessionIdError::NotFound(session_id.to_string())),
                    1 => matching[0]
                        .id
                        .parse()
                        .map_err(|_| SessionIdError::InvalidFormat(session_id.to_string()))?,
                    n => return Err(SessionIdError::Ambiguous(session_id.to_string(), n)),
                }
            } else {
                return Err(SessionIdError::InvalidFormat(session_id.to_string()));
            }
        }
    };

    // Verify the session exists by checking if the rollout file exists
    let rollout_path = get_rollout_path(&cortex_home, &conversation_id);
    if !rollout_path.exists() {
        return Err(SessionIdError::SessionNotFound(session_id.to_string()));
    }

    Ok(conversation_id)
}

/// Get the most recent session from the cortex home directory.
///
/// # Arguments
/// * `cortex_home` - Path to the cortex home directory
///
/// # Returns
/// The most recent session ID, or None if no sessions exist.
pub fn get_most_recent_session(cortex_home: &Path) -> Option<String> {
    let path_buf = cortex_home.to_path_buf();
    list_sessions(&path_buf)
        .ok()
        .and_then(|sessions| sessions.first().map(|s| s.id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ============================================================
    // Tests for empty session ID
    // ============================================================

    #[test]
    fn test_empty_session_id() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("", &home);
        assert!(matches!(result, Err(SessionIdError::Empty)));
    }

    // ============================================================
    // Tests for invalid characters
    // ============================================================

    #[test]
    fn test_invalid_characters() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("session id!", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidCharacters)));
    }

    #[test]
    fn test_invalid_characters_with_space() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("session id", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidCharacters)));
    }

    #[test]
    fn test_invalid_characters_with_special_chars() {
        let home = PathBuf::from("/nonexistent");

        // Test various special characters
        let invalid_ids = [
            "session@id",
            "session#id",
            "session$id",
            "session%id",
            "session^id",
            "session&id",
            "session*id",
            "session(id",
            "session)id",
            "session+id",
            "session=id",
            "session[id",
            "session]id",
            "session{id",
            "session}id",
            "session|id",
            "session\\id",
            "session/id",
            "session:id",
            "session;id",
            "session'id",
            "session\"id",
            "session<id",
            "session>id",
            "session,id",
            "session.id",
            "session?id",
        ];

        for invalid_id in invalid_ids {
            let result = resolve_session_id(invalid_id, &home);
            assert!(
                matches!(result, Err(SessionIdError::InvalidCharacters)),
                "Expected InvalidCharacters error for '{}', got {:?}",
                invalid_id,
                result
            );
        }
    }

    #[test]
    fn test_valid_characters_alphanumeric() {
        let home = PathBuf::from("/nonexistent");

        // These should NOT fail with InvalidCharacters (may fail with other errors)
        let valid_char_ids = [
            "abcdefgh", "ABCDEFGH", "12345678", "abc12345", "ABC-1234", "abc_1234", "a-b-c-d-",
            "a_b_c_d_",
        ];

        for valid_id in valid_char_ids {
            let result = resolve_session_id(valid_id, &home);
            assert!(
                !matches!(result, Err(SessionIdError::InvalidCharacters)),
                "Got unexpected InvalidCharacters error for '{}': {:?}",
                valid_id,
                result
            );
        }
    }

    // ============================================================
    // Tests for invalid format (not 8 chars, not UUID)
    // ============================================================

    #[test]
    fn test_invalid_format_too_short() {
        let home = PathBuf::from("/nonexistent");
        // Less than 8 chars and not a valid UUID
        let result = resolve_session_id("abc", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidFormat(_))));
    }

    #[test]
    fn test_invalid_format_too_long_not_uuid() {
        let home = PathBuf::from("/nonexistent");
        // More than 8 chars but not a valid UUID format
        let result = resolve_session_id("abcdefghijk", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidFormat(_))));
    }

    #[test]
    fn test_invalid_format_wrong_uuid_pattern() {
        let home = PathBuf::from("/nonexistent");
        // Looks like UUID but invalid format
        let result = resolve_session_id("not-a-valid-uuid-format", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidFormat(_))));
    }

    #[test]
    fn test_invalid_format_partial_uuid() {
        let home = PathBuf::from("/nonexistent");
        // Partial UUID (more than 8 chars, less than full UUID)
        let result = resolve_session_id("550e8400-e29b", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidFormat(_))));
    }

    // ============================================================
    // Tests for SessionIdError Display messages
    // ============================================================

    #[test]
    fn test_session_id_error_empty_display() {
        let error = SessionIdError::Empty;
        let message = error.to_string();
        assert!(message.contains("cannot be empty"));
        assert!(message.contains("cortex sessions"));
    }

    #[test]
    fn test_session_id_error_invalid_characters_display() {
        let error = SessionIdError::InvalidCharacters;
        let message = error.to_string();
        assert!(message.contains("alphanumeric"));
        assert!(message.contains("hyphens"));
        assert!(message.contains("underscores"));
    }

    #[test]
    fn test_session_id_error_not_found_display() {
        let error = SessionIdError::NotFound("abc12345".to_string());
        let message = error.to_string();
        assert!(message.contains("abc12345"));
        assert!(message.contains("No session found"));
    }

    #[test]
    fn test_session_id_error_ambiguous_display() {
        let error = SessionIdError::Ambiguous("abc12345".to_string(), 3);
        let message = error.to_string();
        assert!(message.contains("abc12345"));
        assert!(message.contains("3 sessions"));
        assert!(message.contains("Ambiguous"));
    }

    #[test]
    fn test_session_id_error_invalid_format_display() {
        let error = SessionIdError::InvalidFormat("badformat".to_string());
        let message = error.to_string();
        assert!(message.contains("badformat"));
        assert!(message.contains("Invalid session ID"));
    }

    #[test]
    fn test_session_id_error_session_not_found_display() {
        let error = SessionIdError::SessionNotFound("550e8400".to_string());
        let message = error.to_string();
        assert!(message.contains("550e8400"));
        assert!(message.contains("Session not found"));
    }

    #[test]
    fn test_session_id_error_list_error_display() {
        let error = SessionIdError::ListError("IO error".to_string());
        let message = error.to_string();
        assert!(message.contains("IO error"));
        assert!(message.contains("Failed to list sessions"));
    }

    // ============================================================
    // Tests for edge cases
    // ============================================================

    #[test]
    fn test_exactly_eight_char_id() {
        let home = PathBuf::from("/nonexistent");
        // Exactly 8 characters - should try short ID resolution
        let result = resolve_session_id("abcd1234", &home);
        // This will fail with ListError or NotFound because the path doesn't exist,
        // but it should NOT fail with InvalidFormat
        assert!(
            !matches!(result, Err(SessionIdError::InvalidFormat(_))),
            "8-char ID should be treated as short ID, not invalid format"
        );
    }

    #[test]
    fn test_hyphen_only_id() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("--------", &home);
        // Hyphens are valid characters, so this should not fail with InvalidCharacters
        assert!(!matches!(result, Err(SessionIdError::InvalidCharacters)));
    }

    #[test]
    fn test_underscore_only_id() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("________", &home);
        // Underscores are valid characters
        assert!(!matches!(result, Err(SessionIdError::InvalidCharacters)));
    }

    #[test]
    fn test_mixed_case_id() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("AbCdEfGh", &home);
        // Mixed case should be valid characters
        assert!(!matches!(result, Err(SessionIdError::InvalidCharacters)));
    }

    #[test]
    fn test_unicode_characters_rejected() {
        let home = PathBuf::from("/nonexistent");
        // Unicode characters should be rejected
        let result = resolve_session_id("séssion", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidCharacters)));
    }

    #[test]
    fn test_emoji_characters_rejected() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("session🎉", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidCharacters)));
    }

    #[test]
    fn test_newline_characters_rejected() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("session\nid", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidCharacters)));
    }

    #[test]
    fn test_tab_characters_rejected() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("session\tid", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidCharacters)));
    }

    #[test]
    fn test_null_byte_rejected() {
        let home = PathBuf::from("/nonexistent");
        let result = resolve_session_id("session\0id", &home);
        assert!(matches!(result, Err(SessionIdError::InvalidCharacters)));
    }
}
