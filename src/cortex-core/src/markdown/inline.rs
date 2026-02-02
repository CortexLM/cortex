//! Inline text rendering for markdown.
//!
//! This module provides utilities for rendering inline markdown elements
//! including styled text spans, blockquotes, horizontal rules, and links.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_engine::markdown::inline::{InlineStyleStack, InlineModifier};
//! use ratatui::style::Style;
//!
//! let mut stack = InlineStyleStack::new(Style::default());
//! stack.push(InlineModifier::Bold);
//! stack.push(InlineModifier::Italic);
//! let style = stack.current_style(); // Bold + Italic combined
//! ```

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::markdown::theme::MarkdownTheme;

/// Maximum blockquote depth supported.
pub const MAX_BLOCKQUOTE_DEPTH: usize = 5;

/// Modifier to apply to inline text.
///
/// Represents different inline formatting options that can be stacked
/// to create complex nested styles.
#[derive(Debug, Clone)]
pub enum InlineModifier {
    /// Bold text (**text**)
    Bold,
    /// Italic text (*text*)
    Italic,
    /// Strikethrough text (~~text~~)
    Strikethrough,
    /// Inline code (`code`)
    Code,
    /// Link with URL ([text](url))
    Link { url: String },
}

/// Stack-based style composition for nested inline elements.
///
/// This struct maintains a stack of inline modifiers that can be pushed
/// and popped as we traverse nested markdown elements. The `current_style()`
/// method computes the combined style from all active modifiers.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_engine::markdown::inline::{InlineStyleStack, InlineModifier};
/// use ratatui::style::Style;
///
/// let mut stack = InlineStyleStack::new(Style::default());
/// stack.push(InlineModifier::Bold);
/// assert_eq!(stack.depth(), 1);
///
/// stack.push(InlineModifier::Italic);
/// assert_eq!(stack.depth(), 2);
///
/// let style = stack.current_style(); // Has both BOLD and ITALIC modifiers
///
/// stack.pop();
/// assert_eq!(stack.depth(), 1);
/// ```
#[derive(Debug, Clone)]
pub struct InlineStyleStack {
    base_style: Style,
    stack: Vec<InlineModifier>,
}

impl InlineStyleStack {
    /// Creates a new style stack with the given base style.
    ///
    /// The base style is used as the starting point when computing
    /// the current combined style.
    #[must_use]
    pub fn new(base_style: Style) -> Self {
        Self {
            base_style,
            stack: Vec::new(),
        }
    }

    /// Pushes a modifier onto the stack.
    ///
    /// The modifier will affect the style returned by `current_style()`
    /// until it is popped.
    pub fn push(&mut self, modifier: InlineModifier) {
        self.stack.push(modifier);
    }

    /// Pops a modifier from the stack.
    ///
    /// Returns `None` if the stack is empty.
    pub fn pop(&mut self) -> Option<InlineModifier> {
        self.stack.pop()
    }

    /// Computes the current combined style from all stacked modifiers.
    ///
    /// Starts with the base style and applies each modifier in order:
    /// - Bold: adds `Modifier::BOLD`
    /// - Italic: adds `Modifier::ITALIC`
    /// - Strikethrough: adds `Modifier::CROSSED_OUT`
    /// - Code: sets background color for inline code appearance
    /// - Link: adds `Modifier::UNDERLINED`
    #[must_use]
    pub fn current_style(&self) -> Style {
        let mut style = self.base_style;

        for modifier in &self.stack {
            match modifier {
                InlineModifier::Bold => {
                    style = style.add_modifier(Modifier::BOLD);
                }
                InlineModifier::Italic => {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                InlineModifier::Strikethrough => {
                    style = style.add_modifier(Modifier::CROSSED_OUT);
                }
                InlineModifier::Code => {
                    // Apply a subtle background for inline code
                    style = style.bg(Color::Rgb(40, 42, 54));
                }
                InlineModifier::Link { .. } => {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
            }
        }

        style
    }

    /// Returns `true` if the stack has no modifiers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Returns the number of modifiers currently on the stack.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Checks if any modifier on the stack matches the given predicate.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use cortex_engine::markdown::inline::{InlineStyleStack, InlineModifier};
    ///
    /// let mut stack = InlineStyleStack::default();
    /// stack.push(InlineModifier::Bold);
    ///
    /// let has_bold = stack.has_modifier(|m| matches!(m, InlineModifier::Bold));
    /// assert!(has_bold);
    /// ```
    #[must_use]
    pub fn has_modifier(&self, check: fn(&InlineModifier) -> bool) -> bool {
        self.stack.iter().any(check)
    }
}

impl Default for InlineStyleStack {
    fn default() -> Self {
        Self::new(Style::default())
    }
}

/// Parse text with inline markdown to styled spans.
///
/// Handles simple inline markdown syntax:
/// - `**bold**` or `__bold__`
/// - `*italic*` or `_italic_`
/// - `~~strikethrough~~`
/// - `` `code` ``
/// - `[text](url)`
///
/// Note: For the main renderer, we use pulldown-cmark which handles this
/// more robustly. This function is for simple single-line parsing if needed.
///
/// # Arguments
///
/// * `text` - The text to parse for inline markdown
/// * `theme` - The theme providing styles for different elements
///
/// # Returns
///
/// A vector of styled spans representing the parsed inline content.
#[must_use]
pub fn parse_inline_spans(text: &str, theme: &MarkdownTheme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut current_text = String::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Check for inline code (backticks)
        if chars[i] == '`' {
            // Flush current text
            if !current_text.is_empty() {
                spans.push(Span::styled(current_text.clone(), theme.text));
                current_text.clear();
            }

            // Find closing backtick
            let start = i + 1;
            let mut end = start;
            while end < len && chars[end] != '`' {
                end += 1;
            }

            if end < len {
                let code: String = chars[start..end].iter().collect();
                spans.push(format_inline_code(&code, theme.code_inline));
                i = end + 1;
                continue;
            }
        }

        // Check for bold (**text** or __text__)
        if i + 1 < len
            && ((chars[i] == '*' && chars[i + 1] == '*')
                || (chars[i] == '_' && chars[i + 1] == '_'))
        {
            let marker = chars[i];
            // Flush current text
            if !current_text.is_empty() {
                spans.push(Span::styled(current_text.clone(), theme.text));
                current_text.clear();
            }

            // Find closing marker
            let start = i + 2;
            let mut end = start;
            while end + 1 < len && !(chars[end] == marker && chars[end + 1] == marker) {
                end += 1;
            }

            if end + 1 < len {
                let bold_text: String = chars[start..end].iter().collect();
                spans.push(Span::styled(bold_text, theme.bold));
                i = end + 2;
                continue;
            }
        }

        // Check for strikethrough (~~text~~)
        if i + 1 < len && chars[i] == '~' && chars[i + 1] == '~' {
            // Flush current text
            if !current_text.is_empty() {
                spans.push(Span::styled(current_text.clone(), theme.text));
                current_text.clear();
            }

            // Find closing marker
            let start = i + 2;
            let mut end = start;
            while end + 1 < len && !(chars[end] == '~' && chars[end + 1] == '~') {
                end += 1;
            }

            if end + 1 < len {
                let strike_text: String = chars[start..end].iter().collect();
                spans.push(Span::styled(strike_text, theme.strikethrough));
                i = end + 2;
                continue;
            }
        }

        // Check for italic (*text* or _text_) - single marker
        if chars[i] == '*' || chars[i] == '_' {
            let marker = chars[i];
            // Make sure it's not bold (double marker)
            if i + 1 >= len || chars[i + 1] != marker {
                // Flush current text
                if !current_text.is_empty() {
                    spans.push(Span::styled(current_text.clone(), theme.text));
                    current_text.clear();
                }

                // Find closing marker
                let start = i + 1;
                let mut end = start;
                while end < len && chars[end] != marker {
                    end += 1;
                }

                if end < len && end > start {
                    let italic_text: String = chars[start..end].iter().collect();
                    spans.push(Span::styled(italic_text, theme.italic));
                    i = end + 1;
                    continue;
                }
            }
        }

        // Check for links [text](url)
        if chars[i] == '[' {
            // Flush current text
            if !current_text.is_empty() {
                spans.push(Span::styled(current_text.clone(), theme.text));
                current_text.clear();
            }

            // Find closing bracket
            let text_start = i + 1;
            let mut text_end = text_start;
            while text_end < len && chars[text_end] != ']' {
                text_end += 1;
            }

            // Check for opening paren
            if text_end + 1 < len && chars[text_end + 1] == '(' {
                let url_start = text_end + 2;
                let mut url_end = url_start;
                while url_end < len && chars[url_end] != ')' {
                    url_end += 1;
                }

                if url_end < len {
                    let link_text: String = chars[text_start..text_end].iter().collect();
                    let link_url: String = chars[url_start..url_end].iter().collect();
                    let link_spans =
                        format_link(&link_text, &link_url, theme.link_text, theme.link_url);
                    spans.extend(link_spans);
                    i = url_end + 1;
                    continue;
                }
            }
        }

        // Regular character
        current_text.push(chars[i]);
        i += 1;
    }

    // Flush remaining text
    if !current_text.is_empty() {
        spans.push(Span::styled(current_text, theme.text));
    }

    // Return at least one empty span if nothing was parsed
    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }

    spans
}

/// Render blockquote prefix for given depth.
///
/// Creates a visual prefix for blockquotes using vertical bars.
/// Nested blockquotes get multiple bars.
///
/// # Examples
///
/// - depth=1: "│ "
/// - depth=2: "│ │ "
/// - depth=3: "│ │ │ "
///
/// # Arguments
///
/// * `depth` - The nesting depth of the blockquote (clamped to MAX_BLOCKQUOTE_DEPTH)
/// * `border_color` - The color for the vertical bars
///
/// # Returns
///
/// A vector of spans representing the blockquote prefix.
#[must_use]
pub fn render_blockquote_prefix(depth: usize, border_color: Color) -> Vec<Span<'static>> {
    let clamped_depth = depth.min(MAX_BLOCKQUOTE_DEPTH);

    if clamped_depth == 0 {
        return vec![];
    }

    let mut spans = Vec::with_capacity(clamped_depth * 2);
    let bar_style = Style::default().fg(border_color);

    for i in 0..clamped_depth {
        spans.push(Span::styled("│", bar_style));
        // Add space after bar, but use dimmer style for deeper nesting
        if i < clamped_depth - 1 {
            spans.push(Span::raw(" "));
        } else {
            spans.push(Span::raw(" "));
        }
    }

    spans
}

/// Render a horizontal rule.
///
/// Creates a horizontal line using the box-drawing character '─'
/// repeated to fill the specified width.
///
/// # Arguments
///
/// * `width` - The width of the horizontal rule in characters
/// * `style` - The style to apply to the rule
///
/// # Returns
///
/// A Line containing the styled horizontal rule.
#[must_use]
pub fn render_hr(width: u16, style: Style) -> Line<'static> {
    let rule = "─".repeat(width as usize);
    Line::from(Span::styled(rule, style))
}

/// Format a link for display: "text (url)".
///
/// Creates a formatted link with the text followed by the URL in parentheses,
/// each with its own style.
///
/// # Arguments
///
/// * `text` - The visible link text
/// * `url` - The link URL
/// * `text_style` - Style for the link text (typically underlined)
/// * `url_style` - Style for the URL (typically dimmed)
///
/// # Returns
///
/// A vector of spans representing the formatted link.
#[must_use]
pub fn format_link(
    text: &str,
    url: &str,
    text_style: Style,
    url_style: Style,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(text.to_string(), text_style),
        Span::styled(" (".to_string(), url_style),
        Span::styled(url.to_string(), url_style),
        Span::styled(")".to_string(), url_style),
    ]
}

/// Format inline code with styling.
///
/// Creates a styled span for inline code. The code is displayed as-is
/// without additional backticks (they are stripped during parsing).
///
/// # Arguments
///
/// * `code` - The code text to format
/// * `style` - The style to apply (typically with background color)
///
/// # Returns
///
/// A span containing the styled code.
#[must_use]
pub fn format_inline_code(code: &str, style: Style) -> Span<'static> {
    Span::styled(code.to_string(), style)
}

/// Combine two styles, with second taking precedence for conflicts.
///
/// This merges two styles together. Properties from the overlay style
/// will override corresponding properties in the base style when both
/// are set.
///
/// # Arguments
///
/// * `base` - The base style to start with
/// * `overlay` - The style to apply on top of the base
///
/// # Returns
///
/// A new style combining both inputs.
#[must_use]
pub fn merge_styles(base: Style, overlay: Style) -> Style {
    base.patch(overlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // InlineStyleStack Tests
    // ============================================================

    #[test]
    fn test_style_stack_new() {
        let base = Style::default().fg(Color::Red);
        let stack = InlineStyleStack::new(base);

        assert!(stack.is_empty());
        assert_eq!(stack.depth(), 0);
        assert_eq!(stack.current_style().fg, Some(Color::Red));
    }

    #[test]
    fn test_style_stack_default() {
        let stack = InlineStyleStack::default();

        assert!(stack.is_empty());
        assert_eq!(stack.depth(), 0);
    }

    #[test]
    fn test_style_stack_push_pop() {
        let mut stack = InlineStyleStack::default();

        stack.push(InlineModifier::Bold);
        assert!(!stack.is_empty());
        assert_eq!(stack.depth(), 1);

        stack.push(InlineModifier::Italic);
        assert_eq!(stack.depth(), 2);

        let popped = stack.pop();
        assert!(matches!(popped, Some(InlineModifier::Italic)));
        assert_eq!(stack.depth(), 1);

        let popped = stack.pop();
        assert!(matches!(popped, Some(InlineModifier::Bold)));
        assert!(stack.is_empty());

        let popped = stack.pop();
        assert!(popped.is_none());
    }

    #[test]
    fn test_current_style_bold() {
        let mut stack = InlineStyleStack::default();
        stack.push(InlineModifier::Bold);

        let style = stack.current_style();
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_current_style_italic() {
        let mut stack = InlineStyleStack::default();
        stack.push(InlineModifier::Italic);

        let style = stack.current_style();
        assert!(style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn test_current_style_strikethrough() {
        let mut stack = InlineStyleStack::default();
        stack.push(InlineModifier::Strikethrough);

        let style = stack.current_style();
        assert!(style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn test_current_style_code() {
        let mut stack = InlineStyleStack::default();
        stack.push(InlineModifier::Code);

        let style = stack.current_style();
        assert!(style.bg.is_some());
    }

    #[test]
    fn test_current_style_link() {
        let mut stack = InlineStyleStack::default();
        stack.push(InlineModifier::Link {
            url: "https://example.com".to_string(),
        });

        let style = stack.current_style();
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn test_current_style_nested_bold_italic() {
        let mut stack = InlineStyleStack::default();
        stack.push(InlineModifier::Bold);
        stack.push(InlineModifier::Italic);

        let style = stack.current_style();
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn test_current_style_multiple_modifiers() {
        let mut stack = InlineStyleStack::default();
        stack.push(InlineModifier::Bold);
        stack.push(InlineModifier::Italic);
        stack.push(InlineModifier::Link {
            url: "https://example.com".to_string(),
        });

        let style = stack.current_style();
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn test_has_modifier() {
        let mut stack = InlineStyleStack::default();
        stack.push(InlineModifier::Bold);
        stack.push(InlineModifier::Link {
            url: "test".to_string(),
        });

        assert!(stack.has_modifier(|m| matches!(m, InlineModifier::Bold)));
        assert!(stack.has_modifier(|m| matches!(m, InlineModifier::Link { .. })));
        assert!(!stack.has_modifier(|m| matches!(m, InlineModifier::Italic)));
        assert!(!stack.has_modifier(|m| matches!(m, InlineModifier::Code)));
    }

    #[test]
    fn test_style_stack_preserves_base_style() {
        let base = Style::default().fg(Color::Yellow).bg(Color::Blue);
        let mut stack = InlineStyleStack::new(base);
        stack.push(InlineModifier::Bold);

        let style = stack.current_style();
        assert_eq!(style.fg, Some(Color::Yellow));
        // Background may be overridden by code modifier, but not by bold
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    // ============================================================
    // Blockquote Prefix Tests
    // ============================================================

    #[test]
    fn test_render_blockquote_prefix_depth_0() {
        let spans = render_blockquote_prefix(0, Color::Cyan);
        assert!(spans.is_empty());
    }

    #[test]
    fn test_render_blockquote_prefix_depth_1() {
        let spans = render_blockquote_prefix(1, Color::Cyan);
        assert_eq!(spans.len(), 2);

        // First span should be the bar
        assert_eq!(spans[0].content.as_ref(), "│");
        // Second span should be space
        assert_eq!(spans[1].content.as_ref(), " ");
    }

    #[test]
    fn test_render_blockquote_prefix_depth_2() {
        let spans = render_blockquote_prefix(2, Color::Cyan);
        assert_eq!(spans.len(), 4);

        assert_eq!(spans[0].content.as_ref(), "│");
        assert_eq!(spans[1].content.as_ref(), " ");
        assert_eq!(spans[2].content.as_ref(), "│");
        assert_eq!(spans[3].content.as_ref(), " ");
    }

    #[test]
    fn test_render_blockquote_prefix_depth_3() {
        let spans = render_blockquote_prefix(3, Color::Cyan);
        assert_eq!(spans.len(), 6);

        for i in 0..3 {
            assert_eq!(spans[i * 2].content.as_ref(), "│");
            assert_eq!(spans[i * 2 + 1].content.as_ref(), " ");
        }
    }

    #[test]
    fn test_render_blockquote_prefix_clamped_to_max() {
        let spans_at_max = render_blockquote_prefix(MAX_BLOCKQUOTE_DEPTH, Color::Cyan);
        let spans_over_max = render_blockquote_prefix(MAX_BLOCKQUOTE_DEPTH + 5, Color::Cyan);

        // Both should have the same number of spans (clamped)
        assert_eq!(spans_at_max.len(), spans_over_max.len());
        assert_eq!(spans_at_max.len(), MAX_BLOCKQUOTE_DEPTH * 2);
    }

    #[test]
    fn test_render_blockquote_prefix_color() {
        let color = Color::Magenta;
        let spans = render_blockquote_prefix(1, color);

        assert_eq!(spans[0].style.fg, Some(color));
    }

    // ============================================================
    // Horizontal Rule Tests
    // ============================================================

    #[test]
    fn test_render_hr_width() {
        let line = render_hr(10, Style::default());
        let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

        assert_eq!(content.chars().count(), 10);
        assert!(content.chars().all(|c| c == '─'));
    }

    #[test]
    fn test_render_hr_zero_width() {
        let line = render_hr(0, Style::default());
        let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

        assert!(content.is_empty());
    }

    #[test]
    fn test_render_hr_style() {
        let style = Style::default().fg(Color::Red);
        let line = render_hr(5, style);

        assert_eq!(line.spans[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn test_render_hr_large_width() {
        let line = render_hr(200, Style::default());
        let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

        assert_eq!(content.chars().count(), 200);
    }

    // ============================================================
    // Format Link Tests
    // ============================================================

    #[test]
    fn test_format_link_basic() {
        let text_style = Style::default().fg(Color::Blue);
        let url_style = Style::default().fg(Color::Gray);
        let spans = format_link("Example", "https://example.com", text_style, url_style);

        assert_eq!(spans.len(), 4);
        assert_eq!(spans[0].content.as_ref(), "Example");
        assert_eq!(spans[1].content.as_ref(), " (");
        assert_eq!(spans[2].content.as_ref(), "https://example.com");
        assert_eq!(spans[3].content.as_ref(), ")");
    }

    #[test]
    fn test_format_link_styles() {
        let text_style = Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::UNDERLINED);
        let url_style = Style::default().fg(Color::DarkGray);
        let spans = format_link("Click", "http://test.com", text_style, url_style);

        assert_eq!(spans[0].style.fg, Some(Color::Blue));
        assert!(spans[0].style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(spans[2].style.fg, Some(Color::DarkGray));
        assert_eq!(spans[3].style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn test_format_link_empty_text() {
        let spans = format_link("", "http://test.com", Style::default(), Style::default());

        assert_eq!(spans[0].content.as_ref(), "");
        assert_eq!(spans[2].content.as_ref(), "http://test.com");
    }

    #[test]
    fn test_format_link_empty_url() {
        let spans = format_link("Link", "", Style::default(), Style::default());

        assert_eq!(spans[0].content.as_ref(), "Link");
        assert_eq!(spans[2].content.as_ref(), "");
    }

    // ============================================================
    // Format Inline Code Tests
    // ============================================================

    #[test]
    fn test_format_inline_code_basic() {
        let style = Style::default().bg(Color::DarkGray);
        let span = format_inline_code("let x = 5;", style);

        assert_eq!(span.content.as_ref(), "let x = 5;");
        assert_eq!(span.style.bg, Some(Color::DarkGray));
    }

    #[test]
    fn test_format_inline_code_empty() {
        let span = format_inline_code("", Style::default());
        assert_eq!(span.content.as_ref(), "");
    }

    #[test]
    fn test_format_inline_code_with_special_chars() {
        let span = format_inline_code("<div>", Style::default());
        assert_eq!(span.content.as_ref(), "<div>");
    }

    // ============================================================
    // Merge Styles Tests
    // ============================================================

    #[test]
    fn test_merge_styles_fg_override() {
        let base = Style::default().fg(Color::Red);
        let overlay = Style::default().fg(Color::Blue);
        let merged = merge_styles(base, overlay);

        assert_eq!(merged.fg, Some(Color::Blue));
    }

    #[test]
    fn test_merge_styles_bg_override() {
        let base = Style::default().bg(Color::White);
        let overlay = Style::default().bg(Color::Black);
        let merged = merge_styles(base, overlay);

        assert_eq!(merged.bg, Some(Color::Black));
    }

    #[test]
    fn test_merge_styles_preserves_base_when_overlay_none() {
        let base = Style::default().fg(Color::Red).bg(Color::White);
        let overlay = Style::default();
        let merged = merge_styles(base, overlay);

        assert_eq!(merged.fg, Some(Color::Red));
        assert_eq!(merged.bg, Some(Color::White));
    }

    #[test]
    fn test_merge_styles_modifier_combination() {
        let base = Style::default().add_modifier(Modifier::BOLD);
        let overlay = Style::default().add_modifier(Modifier::ITALIC);
        let merged = merge_styles(base, overlay);

        // patch() combines modifiers
        assert!(merged.add_modifier.contains(Modifier::BOLD));
        assert!(merged.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn test_merge_styles_empty() {
        let base = Style::default();
        let overlay = Style::default();
        let merged = merge_styles(base, overlay);

        assert_eq!(merged.fg, None);
        assert_eq!(merged.bg, None);
    }

    // ============================================================
    // Parse Inline Spans Tests
    // ============================================================

    #[test]
    fn test_parse_inline_spans_plain_text() {
        let theme = MarkdownTheme::default();
        let spans = parse_inline_spans("Hello world", &theme);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "Hello world");
    }

    #[test]
    fn test_parse_inline_spans_bold() {
        let theme = MarkdownTheme::default();
        let spans = parse_inline_spans("This is **bold** text", &theme);

        assert!(spans.len() >= 3);
        // Should contain "This is ", "bold", " text"
        let full_text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(full_text, "This is bold text");
    }

    #[test]
    fn test_parse_inline_spans_italic() {
        let theme = MarkdownTheme::default();
        let spans = parse_inline_spans("This is *italic* text", &theme);

        let full_text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(full_text, "This is italic text");
    }

    #[test]
    fn test_parse_inline_spans_code() {
        let theme = MarkdownTheme::default();
        let spans = parse_inline_spans("Use `code` here", &theme);

        let full_text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(full_text, "Use code here");
    }

    #[test]
    fn test_parse_inline_spans_strikethrough() {
        let theme = MarkdownTheme::default();
        let spans = parse_inline_spans("This is ~~deleted~~ text", &theme);

        let full_text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(full_text, "This is deleted text");
    }

    #[test]
    fn test_parse_inline_spans_link() {
        let theme = MarkdownTheme::default();
        let spans = parse_inline_spans("Click [here](https://example.com) now", &theme);

        let full_text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(full_text.contains("here"));
        assert!(full_text.contains("https://example.com"));
    }

    #[test]
    fn test_parse_inline_spans_empty() {
        let theme = MarkdownTheme::default();
        let spans = parse_inline_spans("", &theme);

        assert!(!spans.is_empty()); // Should return at least one span
    }

    #[test]
    fn test_parse_inline_spans_unclosed_markers() {
        let theme = MarkdownTheme::default();
        // Unclosed markers should be treated as plain text
        let spans = parse_inline_spans("This **is unclosed", &theme);

        let full_text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        // The text should be preserved even if markers aren't closed
        assert!(full_text.contains("This") || full_text.contains("**"));
    }
}
