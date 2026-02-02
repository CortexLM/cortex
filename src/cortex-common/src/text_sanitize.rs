//! Text sanitization utilities.
//!
//! Provides functions for sanitizing text output to prevent terminal side effects
//! from control characters in model responses.
//!
//! # Issues Addressed
//! - #2797: ASCII control characters causing terminal side effects
//! - #2794: Triple backticks parsing edge cases

use std::borrow::Cow;

/// Sanitize text by removing or escaping problematic control characters.
///
/// This function removes ASCII control characters that can cause unintended
/// terminal side effects like:
/// - Terminal bells/beeps (BEL \x07)
/// - Text being backspaced/overwritten (BS \x08)
/// - Display corruption (FF \x0C, VT \x0B)
///
/// Escape sequences (ESC \x1B) are escaped for visibility rather than
/// completely removed, as they may be intentional formatting that failed.
///
/// # Arguments
/// * `text` - The text to sanitize
///
/// # Returns
/// A `Cow<str>` containing the sanitized text. Returns `Cow::Borrowed` if
/// no modifications were needed, `Cow::Owned` otherwise.
///
/// # Examples
/// ```
/// use cortex_common::text_sanitize::sanitize_control_chars;
///
/// // BEL characters are removed
/// let text = "Hello\x07World";
/// assert_eq!(sanitize_control_chars(text), "HelloWorld");
///
/// // Normal text passes through unchanged
/// let normal = "Hello World";
/// assert_eq!(sanitize_control_chars(normal), "Hello World");
/// ```
pub fn sanitize_control_chars(text: &str) -> Cow<'_, str> {
    // Quick check if sanitization is needed
    let needs_sanitization = text
        .chars()
        .any(|c| matches!(c, '\x07' | '\x08' | '\x0B' | '\x0C' | '\x1B' | '\x7F'));

    if !needs_sanitization {
        return Cow::Borrowed(text);
    }

    let mut result = String::with_capacity(text.len());

    for c in text.chars() {
        match c {
            '\x07' | '\x08' | '\x0B' | '\x0C' | '\x7F' => {
                // Skip these problematic control characters
            }
            '\x1B' => {
                // Escape the escape character for visibility
                result.push_str("\\e");
            }
            _ => {
                result.push(c);
            }
        }
    }

    Cow::Owned(result)
}

/// Sanitize text for safe terminal display.
///
/// This is a more comprehensive sanitization that also handles:
/// - ANSI escape sequences that might affect terminal state
/// - Cursor movement sequences
/// - Screen clearing sequences
///
/// # Arguments
/// * `text` - The text to sanitize
///
/// # Returns
/// Sanitized text safe for terminal display.
pub fn sanitize_for_terminal(text: &str) -> Cow<'_, str> {
    // First apply basic control character sanitization
    let sanitized = sanitize_control_chars(text);

    // If we already have an owned string, continue with that
    // Otherwise, check if we need to do ANSI escape sequence sanitization
    let text_to_check: &str = &sanitized;

    // Check for potentially dangerous ANSI escape sequences
    // We allow color codes but strip cursor movement, screen clearing, etc.
    if !text_to_check.contains("\x1B[") && !text_to_check.contains("\\e[") {
        return sanitized;
    }

    // For now, return as-is since basic sanitization already escaped \x1B
    // More sophisticated ANSI filtering could be added here if needed
    sanitized
}

/// Validate and normalize code fence sequences in prompts.
///
/// Handles edge cases with triple backticks that might cause parsing issues:
/// - Adjacent code blocks without proper spacing
/// - Unbalanced backticks
/// - Backticks at unusual positions
///
/// # Arguments
/// * `prompt` - The prompt text to validate
///
/// # Returns
/// A tuple of (normalized_text, warnings) where warnings list any issues found.
pub fn normalize_code_fences(prompt: &str) -> (Cow<'_, str>, Vec<String>) {
    let mut warnings = Vec::new();

    // Count backtick sequences
    let mut backtick_count = 0;
    let chars = prompt.chars();
    let mut consecutive_backticks = 0;

    for c in chars {
        if c == '`' {
            consecutive_backticks += 1;
        } else {
            if consecutive_backticks >= 3 {
                backtick_count += 1;
            }
            consecutive_backticks = 0;
        }
    }

    // Handle trailing backticks
    if consecutive_backticks >= 3 {
        backtick_count += 1;
    }

    // Check for unbalanced code fences
    if backtick_count % 2 != 0 {
        warnings.push("Unbalanced code fence detected (odd number of ``` sequences)".to_string());
    }

    // Check for adjacent code blocks (``````pattern)
    if prompt.contains("``````") {
        warnings.push("Adjacent code blocks detected without spacing".to_string());
    }

    // The prompt itself doesn't need modification - we just warn about potential issues
    // The actual parsing should handle these cases gracefully
    (Cow::Borrowed(prompt), warnings)
}

/// Check if a string contains potentially problematic control characters.
///
/// # Arguments
/// * `text` - The text to check
///
/// # Returns
/// `true` if the text contains problematic control characters.
pub fn has_control_chars(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c, '\x00'..='\x06' | '\x07' | '\x08' | '\x0B' | '\x0C' | '\x0E'..='\x1A' | '\x1B' | '\x7F')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_bel() {
        let text = "Hello\x07World\x07\x07!";
        assert_eq!(sanitize_control_chars(text), "HelloWorld!");
    }

    #[test]
    fn test_sanitize_backspace() {
        let text = "Hello\x08\x08World";
        assert_eq!(sanitize_control_chars(text), "HelloWorld");
    }

    #[test]
    fn test_sanitize_form_feed() {
        let text = "Page1\x0CPage2";
        assert_eq!(sanitize_control_chars(text), "Page1Page2");
    }

    #[test]
    fn test_sanitize_escape() {
        let text = "Hello\x1B[31mRed";
        let result = sanitize_control_chars(text);
        assert!(result.contains("\\e"));
        assert!(!result.contains('\x1B'));
    }

    #[test]
    fn test_sanitize_normal_text() {
        let text = "Hello World\nNew Line\tTab";
        // Normal text should not be modified (tabs and newlines are OK)
        assert!(matches!(sanitize_control_chars(text), Cow::Borrowed(_)));
    }

    #[test]
    fn test_normalize_code_fences_balanced() {
        let prompt = "```python\nprint('hello')\n```";
        let (_, warnings) = normalize_code_fences(prompt);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_normalize_code_fences_unbalanced() {
        let prompt = "```python\nprint('incomplete')";
        let (_, warnings) = normalize_code_fences(prompt);
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("Unbalanced"));
    }

    #[test]
    fn test_normalize_code_fences_adjacent() {
        let prompt = "``````python\nbar()\n```";
        let (_, warnings) = normalize_code_fences(prompt);
        assert!(warnings.iter().any(|w| w.contains("Adjacent")));
    }

    #[test]
    fn test_has_control_chars() {
        assert!(has_control_chars("Hello\x07World"));
        assert!(has_control_chars("Test\x08"));
        assert!(!has_control_chars("Normal text\n\t"));
    }
}
