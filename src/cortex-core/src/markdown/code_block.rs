//! Code block rendering with syntax highlighting.
//!
//! This module provides rendering for fenced code blocks with:
//! - Syntax highlighting via cortex_tui_syntax
//! - Bordered display with language tags
//! - Optional line numbers
//! - Incremental rendering for streaming content
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_engine::markdown::code_block::{CodeBlockRenderer, IncrementalCodeBlock};
//! use cortex_tui_syntax::Highlighter;
//! use std::sync::Arc;
//!
//! let highlighter = Arc::new(Highlighter::new());
//! let renderer = Arc::new(CodeBlockRenderer::new(highlighter));
//!
//! // One-shot rendering
//! let lines = renderer.render("fn main() {}", Some("rust"), 80);
//!
//! // Streaming rendering
//! let mut block = IncrementalCodeBlock::new(renderer, Some("rust".to_string()));
//! block.append("fn main");
//! block.append("() {}");
//! let lines = block.get_lines(80);
//! ```

use cortex_tui_syntax::{HighlightSpan, Highlighter};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::sync::Arc;

use crate::style::{BORDER, INFO, SURFACE_0, TEXT};

// ============================================================
// Border Characters
// ============================================================

/// Box-drawing characters for code block borders.
pub mod border {
    /// Top-left corner: `┌`
    pub const TOP_LEFT: char = '┌';
    /// Top-right corner: `┐`
    pub const TOP_RIGHT: char = '┐';
    /// Bottom-left corner: `└`
    pub const BOTTOM_LEFT: char = '└';
    /// Bottom-right corner: `┘`
    pub const BOTTOM_RIGHT: char = '┘';
    /// Horizontal line: `─`
    pub const HORIZONTAL: char = '─';
    /// Vertical line: `│`
    pub const VERTICAL: char = '│';
}

// ============================================================
// CodeBlockRenderer
// ============================================================

/// Renderer for code blocks with syntax highlighting.
///
/// This renderer provides bordered code blocks with:
/// - Language tag in the top border
/// - Syntax highlighting for known languages
/// - Optional line numbers
/// - Consistent styling from the Cortex theme
#[derive(Debug, Clone)]
pub struct CodeBlockRenderer {
    /// The syntax highlighter.
    highlighter: Arc<Highlighter>,
    /// Color for borders.
    border_color: Color,
    /// Background color for code content.
    background_color: Color,
    /// Style for unhighlighted text.
    text_style: Style,
    /// Style for the language tag.
    lang_tag_style: Style,
    /// Whether to show line numbers.
    show_line_numbers: bool,
}

impl CodeBlockRenderer {
    /// Creates a new code block renderer with default styling.
    pub fn new(highlighter: Arc<Highlighter>) -> Self {
        Self {
            highlighter,
            border_color: BORDER,
            background_color: SURFACE_0,
            text_style: Style::default().fg(TEXT),
            lang_tag_style: Style::default().fg(INFO).add_modifier(Modifier::ITALIC),
            show_line_numbers: false,
        }
    }

    /// Sets the border color.
    pub fn with_border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    /// Sets the background color.
    pub fn with_background(mut self, color: Color) -> Self {
        self.background_color = color;
        self
    }

    /// Sets the default text style.
    pub fn with_text_style(mut self, style: Style) -> Self {
        self.text_style = style;
        self
    }

    /// Sets the language tag style.
    pub fn with_lang_tag_style(mut self, style: Style) -> Self {
        self.lang_tag_style = style;
        self
    }

    /// Enables or disables line numbers.
    pub fn with_line_numbers(mut self, show: bool) -> Self {
        self.show_line_numbers = show;
        self
    }

    /// Renders a code block with borders.
    ///
    /// The output format is:
    /// ```text
    /// ┌─ rust ───────────────────────────┐
    /// │ fn main() {                      │
    /// │     println!("Hello, world!");   │
    /// │ }                                │
    /// └──────────────────────────────────┘
    /// ```
    pub fn render(&self, code: &str, language: Option<&str>, max_width: u16) -> Vec<Line<'static>> {
        let border_style = Style::default().fg(self.border_color);
        let bg_style = Style::default().bg(self.background_color);

        // Highlight the code
        let mut highlighted_lines =
            highlight_code(&self.highlighter, code, language, self.text_style);

        // Add line numbers if enabled
        if self.show_line_numbers {
            highlighted_lines = render_with_line_numbers(
                highlighted_lines,
                Style::default()
                    .fg(self.border_color)
                    .add_modifier(Modifier::DIM),
            );
        }

        // Calculate content width (inside borders)
        // Account for "│ " prefix and " │" suffix (4 chars total)
        let content_width = max_width.saturating_sub(4) as usize;

        // Find the maximum line width to determine box width
        let max_line_width = highlighted_lines
            .iter()
            .map(|spans| spans_width(spans))
            .max()
            .unwrap_or(0)
            .min(content_width);

        // Use the larger of max_line_width or a minimum width for the language tag
        let lang_min_width = language.map(|l| l.len() + 3).unwrap_or(0); // " rust "
        let box_content_width = max_line_width.max(lang_min_width).max(10);
        let box_width = (box_content_width + 4) as u16; // Add border chars

        let mut lines = Vec::with_capacity(highlighted_lines.len() + 2);

        // Top border with language tag
        lines.push(render_top_border(
            language,
            box_width,
            border_style,
            self.lang_tag_style,
        ));

        // Content lines
        for spans in highlighted_lines {
            lines.push(render_content_line(
                spans,
                box_width,
                border_style,
                bg_style,
            ));
        }

        // Bottom border
        lines.push(render_bottom_border(box_width, border_style));

        lines
    }

    /// Renders code without borders (just highlighted lines).
    ///
    /// Useful when you want syntax highlighting without the box decoration.
    pub fn render_bare(&self, code: &str, language: Option<&str>) -> Vec<Line<'static>> {
        let mut highlighted_lines =
            highlight_code(&self.highlighter, code, language, self.text_style);

        // Add line numbers if enabled
        if self.show_line_numbers {
            highlighted_lines = render_with_line_numbers(
                highlighted_lines,
                Style::default()
                    .fg(self.border_color)
                    .add_modifier(Modifier::DIM),
            );
        }

        highlighted_lines.into_iter().map(Line::from).collect()
    }

    /// Returns a reference to the highlighter.
    pub fn highlighter(&self) -> &Highlighter {
        &self.highlighter
    }
}

// ============================================================
// IncrementalCodeBlock
// ============================================================

/// Incremental code block for streaming content.
///
/// Caches rendered output and only re-renders when content changes.
/// This is optimized for streaming scenarios where content is appended
/// frequently and you want to minimize re-rendering overhead.
#[derive(Debug)]
pub struct IncrementalCodeBlock {
    /// The source code content.
    source: String,
    /// The language for syntax highlighting.
    language: Option<String>,
    /// Cached rendered lines.
    cached_lines: Vec<Line<'static>>,
    /// Whether the cache is dirty (needs re-render).
    dirty: bool,
    /// The renderer to use.
    renderer: Arc<CodeBlockRenderer>,
    /// Last rendered width (for detecting width changes).
    last_width: u16,
}

impl IncrementalCodeBlock {
    /// Creates a new incremental code block.
    pub fn new(renderer: Arc<CodeBlockRenderer>, language: Option<String>) -> Self {
        Self {
            source: String::new(),
            language,
            cached_lines: Vec::new(),
            dirty: true,
            renderer,
            last_width: 0,
        }
    }

    /// Appends content to the code block.
    ///
    /// Marks the cache as dirty so the next call to `get_lines` will re-render.
    pub fn append(&mut self, content: &str) {
        self.source.push_str(content);
        self.dirty = true;
    }

    /// Sets the entire source content.
    ///
    /// Uses smart diffing to only mark as dirty if content actually changed.
    pub fn set_source(&mut self, source: &str) {
        if self.source != source {
            self.source.clear();
            self.source.push_str(source);
            self.dirty = true;
        }
    }

    /// Gets the rendered lines, re-rendering if necessary.
    ///
    /// This method caches the result, so subsequent calls with the same
    /// content and width will return the cached lines.
    pub fn get_lines(&mut self, max_width: u16) -> &[Line<'static>] {
        // Check if width changed
        if self.last_width != max_width {
            self.dirty = true;
            self.last_width = max_width;
        }

        self.rehighlight(max_width);
        &self.cached_lines
    }

    /// Forces a re-render on the next `get_lines` call.
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Clears all content.
    pub fn clear(&mut self) {
        self.source.clear();
        self.cached_lines.clear();
        self.dirty = true;
    }

    /// Returns whether the content has changed since last render.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Returns the current source content.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the language.
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Sets the language for highlighting.
    pub fn set_language(&mut self, language: Option<String>) {
        if self.language != language {
            self.language = language;
            self.dirty = true;
        }
    }

    /// Re-highlights the content if dirty.
    fn rehighlight(&mut self, max_width: u16) {
        if !self.dirty {
            return;
        }

        self.cached_lines = self
            .renderer
            .render(&self.source, self.language.as_deref(), max_width);
        self.dirty = false;
    }
}

// ============================================================
// Syntax Highlighting Helpers
// ============================================================

/// Converts a cortex_tui_syntax HighlightSpan to a ratatui Span.
fn highlight_span_to_ratatui(span: &HighlightSpan) -> Span<'static> {
    // Convert cortex_tui_text::Style to ratatui::style::Style
    let mut style = Style::default();

    // Extract foreground color
    if let Some(fg) = span.style.fg {
        style = style.fg(cortex_tui_color_to_ratatui(fg));
    }

    // Extract background color
    if let Some(bg) = span.style.bg {
        style = style.bg(cortex_tui_color_to_ratatui(bg));
    }

    // Extract modifiers from attributes
    if span.style.attributes.is_bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.style.attributes.is_italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.style.attributes.is_underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if span.style.attributes.is_strikethrough() {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if span.style.attributes.is_dim() {
        style = style.add_modifier(Modifier::DIM);
    }

    Span::styled(span.text.clone(), style)
}

/// Converts a cortex_tui_text Color to a ratatui Color.
///
/// cortex_tui_text::Color uses f32 values (0.0-1.0), while ratatui uses u8 (0-255).
fn cortex_tui_color_to_ratatui(color: cortex_tui_text::Color) -> Color {
    let (r, g, b) = color.to_rgb_u8();
    Color::Rgb(r, g, b)
}

/// Highlights code using the highlighter, falling back to plain text if language is unknown.
fn highlight_code(
    highlighter: &Highlighter,
    code: &str,
    language: Option<&str>,
    fallback_style: Style,
) -> Vec<Vec<Span<'static>>> {
    // Split code into lines first
    let code_lines: Vec<&str> = code.lines().collect();

    // Handle empty code
    if code_lines.is_empty() {
        return vec![vec![Span::styled("", fallback_style)]];
    }

    // Try to highlight if we have a language
    if let Some(lang) = language {
        if highlighter.has_language(lang) {
            // Highlight the entire code
            match highlighter.highlight(code, lang) {
                Ok(highlighted) => {
                    // Convert HighlightSpan to ratatui Span, split by line
                    return split_highlighted_spans_by_line(highlighted.spans(), code);
                }
                Err(_) => {
                    // Fall through to plain text rendering
                }
            }
        }
    }

    // Fallback: render as plain text with the fallback style
    code_lines
        .into_iter()
        .map(|line| vec![Span::styled(line.to_string(), fallback_style)])
        .collect()
}

/// Splits highlighted spans into lines.
///
/// This handles the case where a span may cross multiple lines.
fn split_highlighted_spans_by_line(
    spans: &[HighlightSpan],
    source: &str,
) -> Vec<Vec<Span<'static>>> {
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();

    for span in spans {
        let text = &span.text;

        // Split span text by newlines
        let mut first = true;
        for part in text.split('\n') {
            if !first {
                // Start a new line
                lines.push(std::mem::take(&mut current_line));
            }
            first = false;

            if !part.is_empty() {
                // Create a new span with the same style but different text
                let mut new_span = span.clone();
                new_span.text = part.to_string();
                current_line.push(highlight_span_to_ratatui(&new_span));
            }
        }
    }

    // Don't forget the last line
    if !current_line.is_empty() || lines.is_empty() {
        lines.push(current_line);
    }

    // Handle trailing newline in source
    if source.ends_with('\n') && !lines.is_empty() {
        lines.push(Vec::new());
    }

    lines
}

// ============================================================
// Border Rendering Helpers
// ============================================================

/// Renders the top border line with optional language tag.
///
/// Format: `┌─ rust ───────────────────────────┐`
fn render_top_border(
    language: Option<&str>,
    width: u16,
    border_style: Style,
    lang_style: Style,
) -> Line<'static> {
    let width = width as usize;

    if width < 4 {
        return Line::from(vec![Span::styled(
            format!("{}{}", border::TOP_LEFT, border::TOP_RIGHT),
            border_style,
        )]);
    }

    let mut spans = Vec::new();

    // Top-left corner
    spans.push(Span::styled(border::TOP_LEFT.to_string(), border_style));

    if let Some(lang) = language {
        // "─ rust "
        let lang_part = format!("{} {} ", border::HORIZONTAL, lang);
        let lang_len = lang_part.chars().count();

        spans.push(Span::styled(border::HORIZONTAL.to_string(), border_style));
        spans.push(Span::styled(format!(" {} ", lang), lang_style));

        // Fill remaining with horizontal lines
        let remaining = width.saturating_sub(2 + lang_len);
        if remaining > 0 {
            spans.push(Span::styled(
                border::HORIZONTAL.to_string().repeat(remaining),
                border_style,
            ));
        }
    } else {
        // No language tag - just horizontal lines
        let horizontal_count = width.saturating_sub(2);
        spans.push(Span::styled(
            border::HORIZONTAL.to_string().repeat(horizontal_count),
            border_style,
        ));
    }

    // Top-right corner
    spans.push(Span::styled(border::TOP_RIGHT.to_string(), border_style));

    Line::from(spans)
}

/// Renders a content line with side borders.
///
/// Format: `│ content here                     │`
fn render_content_line(
    spans: Vec<Span<'static>>,
    width: u16,
    border_style: Style,
    bg_style: Style,
) -> Line<'static> {
    let width = width as usize;

    if width < 4 {
        return Line::from(vec![
            Span::styled(border::VERTICAL.to_string(), border_style),
            Span::styled(border::VERTICAL.to_string(), border_style),
        ]);
    }

    // Calculate current content width
    let content_width = spans_width(&spans);
    let inner_width = width.saturating_sub(4); // Account for "│ " and " │"

    let mut result = Vec::with_capacity(spans.len() + 4);

    // Left border with space
    result.push(Span::styled(format!("{} ", border::VERTICAL), border_style));

    // Content spans
    result.extend(spans);

    // Padding to fill width
    let padding_needed = inner_width.saturating_sub(content_width);
    if padding_needed > 0 {
        result.push(Span::styled(" ".repeat(padding_needed), bg_style));
    }

    // Right border with space
    result.push(Span::styled(format!(" {}", border::VERTICAL), border_style));

    Line::from(result)
}

/// Renders the bottom border line.
///
/// Format: `└──────────────────────────────────┘`
fn render_bottom_border(width: u16, border_style: Style) -> Line<'static> {
    let width = width as usize;

    if width < 2 {
        return Line::from(vec![Span::styled(
            border::BOTTOM_LEFT.to_string(),
            border_style,
        )]);
    }

    let horizontal_count = width.saturating_sub(2);

    Line::from(vec![
        Span::styled(border::BOTTOM_LEFT.to_string(), border_style),
        Span::styled(
            border::HORIZONTAL.to_string().repeat(horizontal_count),
            border_style,
        ),
        Span::styled(border::BOTTOM_RIGHT.to_string(), border_style),
    ])
}

// ============================================================
// Line Number Rendering
// ============================================================

/// Adds line numbers to highlighted lines.
///
/// Format: `  1 │ content`
fn render_with_line_numbers(
    lines: Vec<Vec<Span<'static>>>,
    line_number_style: Style,
) -> Vec<Vec<Span<'static>>> {
    let line_count = lines.len();
    let number_width = line_count.to_string().len();

    lines
        .into_iter()
        .enumerate()
        .map(|(idx, mut spans)| {
            let line_num = idx + 1;
            let prefix = format!(
                "{:>width$} {} ",
                line_num,
                border::VERTICAL,
                width = number_width
            );

            let mut new_spans = Vec::with_capacity(spans.len() + 1);
            new_spans.push(Span::styled(prefix, line_number_style));
            new_spans.append(&mut spans);
            new_spans
        })
        .collect()
}

// ============================================================
// Utility Functions
// ============================================================

/// Calculates the display width of a slice of spans.
///
/// Uses unicode_width for accurate width calculation with CJK and emoji.
fn spans_width(spans: &[Span]) -> usize {
    use unicode_width::UnicodeWidthStr;
    spans.iter().map(|s| s.content.width()).sum()
}

/// Pads spans to reach a target width.
fn pad_to_width(
    mut spans: Vec<Span<'static>>,
    current_width: usize,
    target_width: usize,
    bg_style: Style,
) -> Vec<Span<'static>> {
    let padding = target_width.saturating_sub(current_width);
    if padding > 0 {
        spans.push(Span::styled(" ".repeat(padding), bg_style));
    }
    spans
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_highlighter() -> Arc<Highlighter> {
        Arc::new(Highlighter::new())
    }

    fn create_test_renderer() -> Arc<CodeBlockRenderer> {
        Arc::new(CodeBlockRenderer::new(create_test_highlighter()))
    }

    #[test]
    fn test_render_simple_code_block() {
        let renderer = create_test_renderer();
        let lines = renderer.render("hello world", None, 40);

        // Should have top border, content, bottom border
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_render_with_language_tag() {
        let renderer = create_test_renderer();
        let lines = renderer.render("fn main() {}", Some("rust"), 40);

        // Check top border contains language
        let top_line = &lines[0];
        let top_content: String = top_line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(top_content.contains("rust"));
    }

    #[test]
    fn test_render_multiline_code() {
        let renderer = create_test_renderer();
        let code = "line 1\nline 2\nline 3";
        let lines = renderer.render(code, None, 40);

        // Top border + 3 content lines + bottom border
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn test_render_with_unknown_language() {
        let renderer = create_test_renderer();
        // "unknown_lang" is not registered, should fall back to plain text
        let lines = renderer.render("some code", Some("unknown_lang"), 40);

        // Should still render successfully
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_render_bare() {
        let renderer = create_test_renderer();
        let lines = renderer.render_bare("line 1\nline 2", None);

        // Should have 2 lines, no borders
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_render_with_line_numbers() {
        let renderer = CodeBlockRenderer::new(create_test_highlighter()).with_line_numbers(true);
        let lines = renderer.render_bare("line 1\nline 2\nline 3", None);

        assert_eq!(lines.len(), 3);

        // Check that line numbers are present
        let first_line: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first_line.contains("1"));
    }

    #[test]
    fn test_render_empty_code() {
        let renderer = create_test_renderer();
        let lines = renderer.render("", None, 40);

        // Should have top border, one empty content line, bottom border
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_incremental_append() {
        let renderer = create_test_renderer();
        let mut block = IncrementalCodeBlock::new(renderer, None);

        block.append("hello");
        assert!(block.is_dirty());
        assert_eq!(block.source(), "hello");

        block.append(" world");
        assert_eq!(block.source(), "hello world");
    }

    #[test]
    fn test_incremental_set_source() {
        let renderer = create_test_renderer();
        let mut block = IncrementalCodeBlock::new(renderer, None);

        block.set_source("first");
        assert!(block.is_dirty());

        let _ = block.get_lines(40);
        assert!(!block.is_dirty());

        // Setting same source shouldn't mark dirty
        block.set_source("first");
        assert!(!block.is_dirty());

        // Setting different source should mark dirty
        block.set_source("second");
        assert!(block.is_dirty());
    }

    #[test]
    fn test_incremental_get_lines_caching() {
        let renderer = create_test_renderer();
        let mut block = IncrementalCodeBlock::new(renderer, None);

        block.set_source("test code");
        let lines1 = block.get_lines(40);
        let len1 = lines1.len();

        // Should not be dirty after get_lines
        assert!(!block.is_dirty());

        // Calling again should return cached
        let lines2 = block.get_lines(40);
        assert_eq!(lines2.len(), len1);
    }

    #[test]
    fn test_incremental_width_change() {
        let renderer = create_test_renderer();
        let mut block = IncrementalCodeBlock::new(renderer, None);

        block.set_source("test code");
        let _ = block.get_lines(40);
        assert!(!block.is_dirty());

        // Width change should trigger re-render
        let _ = block.get_lines(80);
        // After render, should not be dirty
        assert!(!block.is_dirty());
    }

    #[test]
    fn test_incremental_clear() {
        let renderer = create_test_renderer();
        let mut block = IncrementalCodeBlock::new(renderer, None);

        block.append("some content");
        block.clear();

        assert!(block.source().is_empty());
        assert!(block.is_dirty());
    }

    #[test]
    fn test_incremental_invalidate() {
        let renderer = create_test_renderer();
        let mut block = IncrementalCodeBlock::new(renderer, None);

        block.set_source("test");
        let _ = block.get_lines(40);
        assert!(!block.is_dirty());

        block.invalidate();
        assert!(block.is_dirty());
    }

    #[test]
    fn test_unicode_in_code() {
        let renderer = create_test_renderer();
        let code = "let emoji = '🦀';\nlet chinese = '中文';";
        let lines = renderer.render(code, None, 60);

        // Should render without panicking
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_various_widths() {
        let renderer = create_test_renderer();
        let code = "hello world";

        // Very narrow
        let lines = renderer.render(code, None, 10);
        assert!(lines.len() >= 3);

        // Normal
        let lines = renderer.render(code, None, 80);
        assert!(lines.len() >= 3);

        // Very wide
        let lines = renderer.render(code, None, 200);
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_border_characters() {
        assert_eq!(border::TOP_LEFT, '┌');
        assert_eq!(border::TOP_RIGHT, '┐');
        assert_eq!(border::BOTTOM_LEFT, '└');
        assert_eq!(border::BOTTOM_RIGHT, '┘');
        assert_eq!(border::HORIZONTAL, '─');
        assert_eq!(border::VERTICAL, '│');
    }

    #[test]
    fn test_builder_pattern() {
        let highlighter = create_test_highlighter();
        let renderer = CodeBlockRenderer::new(highlighter)
            .with_border_color(Color::Red)
            .with_background(Color::Black)
            .with_text_style(Style::default().fg(Color::White))
            .with_lang_tag_style(Style::default().fg(Color::Yellow))
            .with_line_numbers(true);

        assert!(renderer.show_line_numbers);
    }

    #[test]
    fn test_spans_width() {
        let spans = vec![Span::raw("hello"), Span::raw(" "), Span::raw("world")];
        assert_eq!(spans_width(&spans), 11);
    }

    #[test]
    fn test_pad_to_width() {
        let spans = vec![Span::raw("hi")];
        let padded = pad_to_width(spans, 2, 10, Style::default());
        assert_eq!(padded.len(), 2);
        assert_eq!(spans_width(&padded), 10);
    }

    #[test]
    fn test_render_top_border_without_language() {
        let border_style = Style::default();
        let lang_style = Style::default();
        let line = render_top_border(None, 20, border_style, lang_style);

        let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(content.starts_with('┌'));
        assert!(content.ends_with('┐'));
    }

    #[test]
    fn test_render_top_border_with_language() {
        let border_style = Style::default();
        let lang_style = Style::default();
        let line = render_top_border(Some("rust"), 30, border_style, lang_style);

        let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(content.contains("rust"));
        assert!(content.starts_with('┌'));
        assert!(content.ends_with('┐'));
    }

    #[test]
    fn test_render_bottom_border() {
        use unicode_width::UnicodeWidthStr;

        let border_style = Style::default();
        let line = render_bottom_border(20, border_style);

        let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(content.starts_with('└'));
        assert!(content.ends_with('┘'));
        // Check display width, not byte length (box-drawing chars are multi-byte)
        assert_eq!(content.width(), 20);
    }

    #[test]
    fn test_render_content_line() {
        let spans = vec![Span::raw("hello")];
        let border_style = Style::default();
        let bg_style = Style::default();
        let line = render_content_line(spans, 20, border_style, bg_style);

        let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(content.starts_with('│'));
        assert!(content.ends_with('│'));
        assert!(content.contains("hello"));
    }

    #[test]
    fn test_line_numbers_format() {
        let lines = vec![
            vec![Span::raw("line 1")],
            vec![Span::raw("line 2")],
            vec![Span::raw("line 3")],
        ];
        let numbered = render_with_line_numbers(lines, Style::default());

        assert_eq!(numbered.len(), 3);

        // Check first line has line number 1
        let first: String = numbered[0].iter().map(|s| s.content.as_ref()).collect();
        assert!(first.contains("1"));
        assert!(first.contains("│"));
    }

    #[test]
    fn test_incremental_language_change() {
        let renderer = create_test_renderer();
        let mut block = IncrementalCodeBlock::new(renderer, Some("rust".to_string()));

        let _ = block.get_lines(40);
        assert!(!block.is_dirty());

        block.set_language(Some("python".to_string()));
        assert!(block.is_dirty());
        assert_eq!(block.language(), Some("python"));
    }
}
