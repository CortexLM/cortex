//! Text truncation utilities.
//!
//! Provides centralized truncation functions used across the codebase.

use std::borrow::Cow;

/// Truncates a string to a maximum length, adding ellipsis if truncated.
///
/// # Arguments
/// * `s` - The string to truncate
/// * `max_len` - Maximum length (including ellipsis)
///
/// # Returns
/// The truncated string with "..." appended if truncation occurred.
///
/// # Examples
/// ```
/// use cortex_common::truncate::truncate_with_ellipsis;
///
/// assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
/// assert_eq!(truncate_with_ellipsis("hello world", 8), "hello...");
/// ```
pub fn truncate_with_ellipsis(s: &str, max_len: usize) -> Cow<'_, str> {
    if s.chars().count() <= max_len {
        Cow::Borrowed(s)
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        Cow::Owned(format!("{}...", truncated))
    }
}

/// Truncates a string to a maximum length, adding unicode ellipsis (…) if truncated.
///
/// # Arguments
/// * `s` - The string to truncate
/// * `max_len` - Maximum character count (including ellipsis)
///
/// # Returns
/// The truncated string with "…" appended if truncation occurred.
///
/// # Examples
/// ```
/// use cortex_common::truncate::truncate_with_unicode_ellipsis;
///
/// assert_eq!(truncate_with_unicode_ellipsis("hello", 10), "hello");
/// assert_eq!(truncate_with_unicode_ellipsis("hello world", 6), "hello…");
/// ```
pub fn truncate_with_unicode_ellipsis(s: &str, max_len: usize) -> Cow<'_, str> {
    let char_count = s.chars().count();
    if char_count <= max_len {
        Cow::Borrowed(s)
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(1)).collect();
        Cow::Owned(format!("{}…", truncated))
    }
}

/// Truncates an ID string to show first N characters with ellipsis.
///
/// Commonly used for displaying tool call IDs, session IDs, etc.
///
/// # Arguments
/// * `id` - The ID string to truncate
/// * `max_len` - Maximum length (default 10 if called with truncate_id_default)
///
/// # Returns
/// The truncated ID with "…" appended if truncation occurred.
pub fn truncate_id(id: &str, max_len: usize) -> Cow<'_, str> {
    truncate_with_unicode_ellipsis(id, max_len)
}

/// Truncates an ID with default max length of 10.
pub fn truncate_id_default(id: &str) -> Cow<'_, str> {
    truncate_id(id, 10)
}

/// Truncates text to a maximum length, taking only the first line.
///
/// Useful for displaying task descriptions, messages, etc.
///
/// # Arguments
/// * `text` - The text to truncate
/// * `max_len` - Maximum character count (including ellipsis)
///
/// # Returns
/// The first line truncated with "…" if it exceeds max_len.
pub fn truncate_first_line(text: &str, max_len: usize) -> Cow<'_, str> {
    let first_line = text.lines().next().unwrap_or(text);
    truncate_with_unicode_ellipsis(first_line, max_len)
}

/// Truncates a command string for display, preserving important parts.
///
/// # Arguments
/// * `command` - The command to truncate
/// * `max_len` - Maximum length
///
/// # Returns
/// The truncated command with "..." appended if truncation occurred.
pub fn truncate_command(command: &str, max_len: usize) -> Cow<'_, str> {
    if command.len() <= max_len {
        Cow::Borrowed(command)
    } else {
        // Take the first part up to max_len - 3 (for "...")
        let truncated = &command[..max_len.saturating_sub(3).min(command.len())];
        // Find last space to avoid cutting in middle of word
        if let Some(last_space) = truncated.rfind(' ') {
            Cow::Owned(format!("{}...", &truncated[..last_space]))
        } else {
            Cow::Owned(format!("{}...", truncated))
        }
    }
}

/// Truncates a string for display in UI widgets.
///
/// # Arguments
/// * `s` - The string to truncate
/// * `max_len` - Maximum character width
///
/// # Returns
/// The truncated string for display.
pub fn truncate_for_display(s: &str, max_len: usize) -> Cow<'_, str> {
    truncate_with_ellipsis(s, max_len)
}

/// Truncates a model name for display in compact spaces.
///
/// # Arguments
/// * `name` - The model name
/// * `max_len` - Maximum length
///
/// # Returns
/// The truncated model name.
pub fn truncate_model_name(name: &str, max_len: usize) -> Cow<'_, str> {
    if name.len() <= max_len {
        Cow::Borrowed(name)
    } else {
        // Try to keep the model family prefix if possible
        if let Some(slash_pos) = name.find('/') {
            let prefix = &name[..slash_pos + 1];
            // Check if we have enough room for prefix + "..." + at least 5 chars of suffix
            if prefix.len() + 3 + 5 <= max_len {
                let suffix_len = max_len - prefix.len() - 3;
                let suffix_start = name.len().saturating_sub(suffix_len);
                return Cow::Owned(format!("{}...{}", prefix, &name[suffix_start..]));
            }
        }
        truncate_with_ellipsis(name, max_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_with_ellipsis_short() {
        assert_eq!(truncate_with_ellipsis("short", 10).as_ref(), "short");
    }

    #[test]
    fn test_truncate_with_ellipsis_exact() {
        assert_eq!(truncate_with_ellipsis("exactlen", 8).as_ref(), "exactlen");
    }

    #[test]
    fn test_truncate_with_ellipsis_long() {
        assert_eq!(
            truncate_with_ellipsis("this is a long string", 10).as_ref(),
            "this is..."
        );
    }

    #[test]
    fn test_truncate_with_unicode_ellipsis() {
        assert_eq!(
            truncate_with_unicode_ellipsis("hello", 10).as_ref(),
            "hello"
        );
        assert_eq!(
            truncate_with_unicode_ellipsis("hello world", 6).as_ref(),
            "hello…"
        );
    }

    #[test]
    fn test_truncate_id() {
        assert_eq!(truncate_id("bg-123456789", 10).as_ref(), "bg-123456…");
        assert_eq!(truncate_id("bg-1", 10).as_ref(), "bg-1");
    }

    #[test]
    fn test_truncate_first_line() {
        assert_eq!(truncate_first_line("line1\nline2", 20).as_ref(), "line1");
        assert_eq!(
            truncate_first_line("very long first line", 10).as_ref(),
            "very long…"
        );
    }

    #[test]
    fn test_truncate_command() {
        assert_eq!(truncate_command("ls -la", 20).as_ref(), "ls -la");
        assert_eq!(
            truncate_command("npm install --save-dev typescript", 20).as_ref(),
            "npm install..."
        );
    }

    #[test]
    fn test_truncate_model_name() {
        assert_eq!(truncate_model_name("gpt-4", 10).as_ref(), "gpt-4");
        // For max_len=20: prefix "anthropic/" (10) + "..." (3) + suffix (7) = 20
        // suffix_start = 32 - 7 = 25, so suffix is "0240229"
        assert_eq!(
            truncate_model_name("anthropic/claude-3-opus-20240229", 20).as_ref(),
            "anthropic/...0240229"
        );
        // For max_len=21: prefix "anthropic/" (10) + "..." (3) + suffix (8) = 21
        // suffix_start = 32 - 8 = 24, so suffix is "20240229"
        assert_eq!(
            truncate_model_name("anthropic/claude-3-opus-20240229", 21).as_ref(),
            "anthropic/...20240229"
        );
    }
}
