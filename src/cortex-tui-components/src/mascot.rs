//! Cortex Mascot - ASCII art brain character
//!
//! The official Cortex CLI mascot with various expressions.

/// The default Cortex mascot (minimal brain).
pub const MASCOT: &str = r#"      ░▒▓████▓▒░
     ▓██▀▀▀▀██▓
     ██ ▌  ▐ ██
     ▓█▄▄▄▄▄▄█▓
      ░▒▓██▓▒░"#;

/// Mascot thinking (processing).
pub const MASCOT_THINKING: &str = r#"      ░▒▓████▓▒░   ✦
     ▓██▀▀▀▀██▓  ...
     ██ ▌  ▐ ██
     ▓█▄▄▄▄▄▄█▓
      ░▒▓██▓▒░"#;

/// Mascot happy (success).
pub const MASCOT_HAPPY: &str = r#"      ░▒▓████▓▒░     *
     ▓██▀▀▀▀██▓   *
     ██ ^  ^ ██
     ▓█▄▄◡▄▄▄█▓     *
      ░▒▓██▓▒░"#;

/// Mascot working (busy).
pub const MASCOT_WORKING: &str = r#"      ░▒▓████▓▒░   ⚡
     ▓██▀▀▀▀██▓  >>>
     ██ ▌  ▐ ██
     ▓█▄▄══▄▄█▓
      ░▒▓██▓▒░"#;

/// Mascot error (failure).
pub const MASCOT_ERROR: &str = r#"      ░▒▓████▓▒░    ✗
     ▓██▀▀▀▀██▓   !
     ██ ×  × ██
     ▓█▄▄▄▄▄▄█▓
      ░▒▓██▓▒░"#;

/// Mascot success (completed).
pub const MASCOT_SUCCESS: &str = r#"      ░▒▓████▓▒░    ✓
     ▓██▀▀▀▀██▓   *
     ██ ▌  ▐ ██
     ▓█▄▄‿▄▄▄█▓     *
      ░▒▓██▓▒░"#;

/// Mascot sleeping (idle).
pub const MASCOT_SLEEPING: &str = r#"      ░▒▓████▓▒░   z
     ▓██▀▀▀▀██▓  zZ
     ██ ─  ─ ██
     ▓█▄▄▄▄▄▄█▓
      ░▒▓██▓▒░"#;

/// Mascot loading (waiting).
pub const MASCOT_LOADING: &str = r#"      ░▒▓████▓▒░   ◐
     ▓██▀▀▀▀██▓  ···
     ██ ▌  ▐ ██
     ▓█▄▄▄▄▄▄█▓
      ░▒▓██▓▒░"#;

/// Mascot question (asking).
pub const MASCOT_QUESTION: &str = r#"      ░▒▓████▓▒░    ?
     ▓██▀▀▀▀██▓
     ██ ▌  ▐ ██
     ▓█▄▄▄▄▄▄█▓
      ░▒▓██▓▒░"#;

/// Mascot idea (eureka).
pub const MASCOT_IDEA: &str = r#"      ░▒▓████▓▒░   💡
     ▓██▀▀▀▀██▓   !
     ██ ▌  ▐ ██
     ▓█▄▄▄▄▄▄█▓     *
      ░▒▓██▓▒░"#;

/// Mascot wink (playful).
pub const MASCOT_WINK: &str = r#"      ░▒▓████▓▒░     *
     ▓██▀▀▀▀██▓   *
     ██ ▌  ─ ██
     ▓█▄▄▄▄▄▄█▓     *
      ░▒▓██▓▒░"#;

/// Minimal mascot for welcome screens (4 lines).
pub const MASCOT_MINIMAL: &str = r#"   ▄█▀▀▀▀█▄
  ██ ▌  ▐ ██
   █▄▄▄▄▄▄█
    █    █"#;

/// Minimal mascot lines for inline rendering (12 chars wide each).
pub const MASCOT_MINIMAL_LINES: [&str; 4] =
    [" ▄█▀▀▀▀█▄  ", "██ ▌  ▐ ██ ", " █▄▄▄▄▄▄█  ", "  █    █   "];

/// Mascot sparkle (special).
pub const MASCOT_SPARKLE: &str = r#"    ✦ ░▒▓████▓▒░ ✦
     ▓██▀▀▀▀██▓
   * ██ ▌  ▐ ██ *
     ▓█▄▄▄▄▄▄█▓
    ✦ ░▒▓██▓▒░ ✦"#;

/// Mascot expression variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MascotExpression {
    /// Default neutral expression
    #[default]
    Normal,
    /// Thinking/processing
    Thinking,
    /// Happy/cheerful
    Happy,
    /// Working/busy
    Working,
    /// Error/failure
    Error,
    /// Success/completed
    Success,
    /// Sleeping/idle
    Sleeping,
    /// Loading/waiting
    Loading,
    /// Question/asking
    Question,
    /// Idea/eureka
    Idea,
    /// Wink/playful
    Wink,
    /// Sparkle/special
    Sparkle,
}

impl MascotExpression {
    /// Returns the ASCII art for this expression.
    pub fn art(&self) -> &'static str {
        match self {
            MascotExpression::Normal => MASCOT,
            MascotExpression::Thinking => MASCOT_THINKING,
            MascotExpression::Happy => MASCOT_HAPPY,
            MascotExpression::Working => MASCOT_WORKING,
            MascotExpression::Error => MASCOT_ERROR,
            MascotExpression::Success => MASCOT_SUCCESS,
            MascotExpression::Sleeping => MASCOT_SLEEPING,
            MascotExpression::Loading => MASCOT_LOADING,
            MascotExpression::Question => MASCOT_QUESTION,
            MascotExpression::Idea => MASCOT_IDEA,
            MascotExpression::Wink => MASCOT_WINK,
            MascotExpression::Sparkle => MASCOT_SPARKLE,
        }
    }

    /// Returns the number of lines in the mascot art.
    pub fn height(&self) -> usize {
        5
    }

    /// Returns the approximate width of the mascot art.
    pub fn width(&self) -> usize {
        22
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mascot_art() {
        assert!(!MASCOT.is_empty());
        assert_eq!(MASCOT.lines().count(), 5);
    }

    #[test]
    fn test_expression_art() {
        let expr = MascotExpression::default();
        assert_eq!(expr.art(), MASCOT);
    }
}
