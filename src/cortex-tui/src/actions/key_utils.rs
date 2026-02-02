//! Key utility functions for parsing and formatting key events.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Parse a key string like "Ctrl+C" into a KeyEvent.
///
/// Supported formats:
/// - Single character: "a", "1", "?"
/// - With modifiers: "Ctrl+C", "Shift+Enter", "Alt+X"
/// - Special keys: "Enter", "Esc", "Tab", "Space", "Backspace"
/// - Function keys: "F1", "F2", ..., "F12"
/// - Arrow keys: "Up", "Down", "Left", "Right"
/// - Page keys: "PageUp", "PageDown", "Home", "End"
pub fn parse_key_string(s: &str) -> Option<KeyEvent> {
    let parts: Vec<&str> = s.split('+').collect();
    let mut modifiers = KeyModifiers::NONE;

    let key_part = if parts.len() == 1 {
        parts[0]
    } else {
        // Parse modifiers
        for &part in &parts[..parts.len() - 1] {
            match part.to_lowercase().as_str() {
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                "super" | "meta" | "cmd" => modifiers |= KeyModifiers::SUPER,
                _ => return None,
            }
        }
        parts.last()?
    };

    let code = parse_key_code(key_part)?;
    Some(KeyEvent::new(code, modifiers))
}

/// Parse a key code from a string.
fn parse_key_code(s: &str) -> Option<KeyCode> {
    // Check for function keys first
    if let Some(num) = s.strip_prefix('F').or_else(|| s.strip_prefix('f'))
        && let Ok(n) = num.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Some(KeyCode::F(n));
    }

    match s.to_lowercase().as_str() {
        // Special keys
        "enter" | "return" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Esc),
        "tab" => Some(KeyCode::Tab),
        "backtab" => Some(KeyCode::BackTab),
        "space" => Some(KeyCode::Char(' ')),
        "backspace" | "bs" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "insert" | "ins" => Some(KeyCode::Insert),

        // Arrow keys
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),

        // Page keys
        "pageup" | "pgup" => Some(KeyCode::PageUp),
        "pagedown" | "pgdn" | "pgdown" => Some(KeyCode::PageDown),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),

        // Single character
        _ if s.chars().count() == 1 => Some(KeyCode::Char(s.chars().next()?)),

        _ => None,
    }
}

/// Format a KeyEvent for display.
///
/// Returns a human-readable string representation of the key event.
pub fn format_key(key: &KeyEvent) -> String {
    let mut parts = Vec::new();

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift");
    }
    if key.modifiers.contains(KeyModifiers::SUPER) {
        parts.push("Super");
    }

    let key_name = format_key_code(&key.code);
    parts.push(&key_name);

    parts.join("+")
}

/// Format a KeyCode for display.
fn format_key_code(code: &KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => {
            if c.is_uppercase() || !c.is_alphanumeric() {
                c.to_string()
            } else {
                c.to_uppercase().to_string()
            }
        }
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BackTab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Null => "Null".to_string(),
        _ => format!("{code:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_key_string() {
        // Simple keys
        assert_eq!(
            parse_key_string("a"),
            Some(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key_string("Enter"),
            Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key_string("Esc"),
            Some(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        );

        // With modifiers
        assert_eq!(
            parse_key_string("Ctrl+C"),
            Some(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key_string("Shift+Enter"),
            Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT))
        );
        assert_eq!(
            parse_key_string("Ctrl+Shift+X"),
            Some(KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ))
        );

        // Function keys
        assert_eq!(
            parse_key_string("F1"),
            Some(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key_string("Ctrl+F12"),
            Some(KeyEvent::new(KeyCode::F(12), KeyModifiers::CONTROL))
        );

        // Invalid
        assert_eq!(parse_key_string("InvalidKey"), None);
    }

    #[test]
    fn test_format_key() {
        assert_eq!(
            format_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            "Ctrl+C"
        );
        assert_eq!(
            format_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            "Enter"
        );
        assert_eq!(
            format_key(&KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
            "F1"
        );
        assert_eq!(
            format_key(&KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            "Ctrl+Shift+X"
        );
    }
}
