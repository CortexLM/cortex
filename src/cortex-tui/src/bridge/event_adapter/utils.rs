//! Utility functions for event adaptation.
//!
//! This module provides low-level helper functions for data transformation
//! and formatting used throughout the event adapter.

use uuid::Uuid;

/// Parse a string UUID, returning None on failure.
///
/// # Arguments
///
/// * `s` - The string to parse as a UUID
///
/// # Returns
///
/// * `Some(Uuid)` - If parsing succeeds
/// * `None` - If the string is not a valid UUID
pub fn parse_uuid(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s).ok()
}

/// Decode a base64-encoded output chunk.
///
/// # Arguments
///
/// * `base64_chunk` - The base64-encoded string
///
/// # Returns
///
/// The decoded string, or an empty string on failure.
pub fn decode_output_chunk(base64_chunk: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(base64_chunk)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

/// Format a command array as a string for display.
///
/// # Arguments
///
/// * `command` - The command as a vector of strings
///
/// # Returns
///
/// A space-separated string representation of the command.
pub fn format_command(command: &[String]) -> String {
    if command.is_empty() {
        "unknown".to_string()
    } else {
        command.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_uuid_valid() {
        let valid = "550e8400-e29b-41d4-a716-446655440000";
        assert!(parse_uuid(valid).is_some());
    }

    #[test]
    fn test_parse_uuid_invalid() {
        let invalid = "not-a-uuid";
        assert!(parse_uuid(invalid).is_none());
    }

    #[test]
    fn test_format_command() {
        let cmd = vec!["ls".to_string(), "-la".to_string(), "/home".to_string()];
        assert_eq!(format_command(&cmd), "ls -la /home");
        assert_eq!(format_command(&[]), "unknown");
    }

    #[test]
    fn test_decode_output_chunk() {
        // Base64 for "Hello"
        let encoded = "SGVsbG8=";
        assert_eq!(decode_output_chunk(encoded), "Hello");

        // Invalid base64
        assert_eq!(decode_output_chunk("!!!invalid!!!"), "");
    }
}
