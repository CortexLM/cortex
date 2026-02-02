//! Text styling utilities.
//!
//! Provides consistent text styling across components.

use cortex_core::style::{CYAN_PRIMARY, ERROR, SUCCESS, TEXT, TEXT_DIM, TEXT_MUTED, WARNING};
use ratatui::style::{Color, Modifier, Style};

/// Text style presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextStyle {
    /// Default text color
    #[default]
    Normal,
    /// Dimmed/secondary text
    Dim,
    /// Very muted text (hints, placeholders)
    Muted,
    /// Accent/highlight color
    Accent,
    /// Success message
    Success,
    /// Warning message
    Warning,
    /// Error message
    Error,
    /// Bold text
    Bold,
    /// Italic text
    Italic,
    /// Code/monospace text
    Code,
}

impl TextStyle {
    /// Convert to ratatui Style.
    pub fn to_style(self) -> Style {
        match self {
            TextStyle::Normal => Style::default().fg(TEXT),
            TextStyle::Dim => Style::default().fg(TEXT_DIM),
            TextStyle::Muted => Style::default().fg(TEXT_MUTED),
            TextStyle::Accent => Style::default().fg(CYAN_PRIMARY),
            TextStyle::Success => Style::default().fg(SUCCESS),
            TextStyle::Warning => Style::default().fg(WARNING),
            TextStyle::Error => Style::default().fg(ERROR),
            TextStyle::Bold => Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            TextStyle::Italic => Style::default().fg(TEXT).add_modifier(Modifier::ITALIC),
            TextStyle::Code => Style::default().fg(CYAN_PRIMARY),
        }
    }
}

impl From<TextStyle> for Style {
    fn from(ts: TextStyle) -> Self {
        ts.to_style()
    }
}

/// A styled text segment.
#[derive(Debug, Clone)]
pub struct StyledText {
    /// The text content
    pub content: String,
    /// The style to apply
    pub style: Style,
}

impl StyledText {
    /// Create new styled text with default style.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            style: Style::default().fg(TEXT),
        }
    }

    /// Set the text style.
    pub fn with_style(mut self, style: TextStyle) -> Self {
        self.style = style.to_style();
        self
    }

    /// Set a custom ratatui style.
    pub fn with_raw_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// Set the foreground color.
    pub fn fg(mut self, color: Color) -> Self {
        self.style = self.style.fg(color);
        self
    }

    /// Set the background color.
    pub fn bg(mut self, color: Color) -> Self {
        self.style = self.style.bg(color);
        self
    }

    /// Make the text bold.
    pub fn bold(mut self) -> Self {
        self.style = self.style.add_modifier(Modifier::BOLD);
        self
    }

    /// Make the text italic.
    pub fn italic(mut self) -> Self {
        self.style = self.style.add_modifier(Modifier::ITALIC);
        self
    }

    /// Make the text underlined.
    pub fn underlined(mut self) -> Self {
        self.style = self.style.add_modifier(Modifier::UNDERLINED);
        self
    }

    /// Convert to a ratatui Span.
    pub fn to_span(&self) -> ratatui::text::Span<'_> {
        ratatui::text::Span::styled(&self.content, self.style)
    }
}

/// Truncate a string to fit within the given width, adding "..." if truncated.
pub fn truncate_with_ellipsis(s: &str, max_width: usize) -> String {
    if max_width < 4 {
        return s.chars().take(max_width).collect();
    }

    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_width {
        s.to_string()
    } else {
        let mut result: String = chars[..max_width - 3].iter().collect();
        result.push_str("...");
        result
    }
}

/// Pad a string to the given width.
pub fn pad_to_width(s: &str, width: usize, align: TextAlign) -> String {
    let len = s.chars().count();
    if len >= width {
        return s.to_string();
    }

    let padding = width - len;
    match align {
        TextAlign::Left => format!("{}{}", s, " ".repeat(padding)),
        TextAlign::Right => format!("{}{}", " ".repeat(padding), s),
        TextAlign::Center => {
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;
            format!("{}{}{}", " ".repeat(left_pad), s, " ".repeat(right_pad))
        }
    }
}

/// Text alignment options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_style_conversion() {
        let style = TextStyle::Normal.to_style();
        assert_eq!(style.fg, Some(TEXT));

        let style = TextStyle::Accent.to_style();
        assert_eq!(style.fg, Some(CYAN_PRIMARY));
    }

    #[test]
    fn test_styled_text_builder() {
        let text = StyledText::new("Hello")
            .with_style(TextStyle::Accent)
            .bold();

        assert_eq!(text.content, "Hello");
        assert_eq!(text.style.fg, Some(CYAN_PRIMARY));
    }

    #[test]
    fn test_truncate_with_ellipsis() {
        assert_eq!(truncate_with_ellipsis("Hello", 10), "Hello");
        assert_eq!(truncate_with_ellipsis("Hello World", 8), "Hello...");
        assert_eq!(truncate_with_ellipsis("Hi", 2), "Hi");
        assert_eq!(truncate_with_ellipsis("Hello", 3), "Hel");
    }

    #[test]
    fn test_pad_to_width() {
        assert_eq!(pad_to_width("Hi", 5, TextAlign::Left), "Hi   ");
        assert_eq!(pad_to_width("Hi", 5, TextAlign::Right), "   Hi");
        assert_eq!(pad_to_width("Hi", 5, TextAlign::Center), " Hi  ");
        assert_eq!(pad_to_width("Hello", 3, TextAlign::Left), "Hello");
    }
}
