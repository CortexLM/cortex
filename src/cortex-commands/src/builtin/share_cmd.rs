//! Share command - generates a share link for the current session.
//!
//! Usage: /share [duration]
//!
//! Examples:
//! - `/share` - Share with default 7-day expiration
//! - `/share 24h` - Share with 24-hour expiration
//! - `/share 30d` - Share with 30-day expiration
//! - `/share never` - Share with no expiration

use std::time::Duration;

/// Result of executing the share command.
#[derive(Debug, Clone)]
pub enum ShareResult {
    /// Share link created successfully.
    Created {
        /// The generated share URL.
        url: String,
        /// When the share expires (human-readable), or "never" if no expiration.
        expires: String,
    },
    /// Error during share creation.
    Error(String),
}

impl ShareResult {
    /// Get a user-friendly message for the result.
    pub fn message(&self) -> String {
        match self {
            ShareResult::Created { url, expires } => {
                format!("Share URL: {}\nExpires: {}", url, expires)
            }
            ShareResult::Error(e) => format!("Error: Failed to create share link: {}", e),
        }
    }
}

/// Parse a duration string into a Duration.
///
/// Supported formats:
/// - `30d` - days
/// - `24h` - hours
/// - `60m` - minutes
/// - `never` - no expiration (returns None)
pub fn parse_duration(s: &str) -> Result<Option<Duration>, String> {
    let s = s.trim().to_lowercase();

    if s == "never" || s == "none" || s.is_empty() {
        return Ok(None);
    }

    let (num_str, unit) = if s.ends_with('d') {
        (&s[..s.len() - 1], "d")
    } else if s.ends_with('h') {
        (&s[..s.len() - 1], "h")
    } else if s.ends_with('m') {
        (&s[..s.len() - 1], "m")
    } else {
        // Default to days if no unit specified
        (s.as_str(), "d")
    };

    let num: u64 = num_str
        .parse()
        .map_err(|_| format!("Invalid duration: {}", s))?;

    let secs = match unit {
        "d" => num * 24 * 60 * 60,
        "h" => num * 60 * 60,
        "m" => num * 60,
        _ => return Err(format!("Unknown duration unit: {}", unit)),
    };

    Ok(Some(Duration::from_secs(secs)))
}

/// Format a duration for display.
pub fn format_duration(d: Option<Duration>) -> String {
    match d {
        None => "never".to_string(),
        Some(d) => {
            let secs = d.as_secs();
            if secs >= 24 * 60 * 60 {
                format!("{} days", secs / (24 * 60 * 60))
            } else if secs >= 60 * 60 {
                format!("{} hours", secs / (60 * 60))
            } else {
                format!("{} minutes", secs / 60)
            }
        }
    }
}

/// Default share duration (7 days).
pub const DEFAULT_SHARE_DURATION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(
            parse_duration("7d").unwrap(),
            Some(Duration::from_secs(7 * 24 * 60 * 60))
        );
        assert_eq!(
            parse_duration("24h").unwrap(),
            Some(Duration::from_secs(24 * 60 * 60))
        );
        assert_eq!(
            parse_duration("60m").unwrap(),
            Some(Duration::from_secs(60 * 60))
        );
        assert_eq!(parse_duration("never").unwrap(), None);
        assert_eq!(parse_duration("").unwrap(), None);
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(
            format_duration(Some(Duration::from_secs(7 * 24 * 60 * 60))),
            "7 days"
        );
        assert_eq!(
            format_duration(Some(Duration::from_secs(24 * 60 * 60))),
            "1 days"
        );
        assert_eq!(
            format_duration(Some(Duration::from_secs(2 * 60 * 60))),
            "2 hours"
        );
        assert_eq!(format_duration(None), "never");
    }
}
