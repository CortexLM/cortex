//! Text display widget.
//!
//! The Text widget displays styled text with support for alignment,
//! wrapping, and truncation.

use crate::buffer::{Buffer, BufferExt};
use crate::event::{Event, EventResult};
use crate::layout::{Dimension, LayoutStyle};
use crate::types::{Color, Rect, Style};
use crate::widget::{Widget, WidgetId, WidgetRef};
use std::any::Any;

/// Text alignment options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    /// Align text to the left (default).
    #[default]
    Left,
    /// Center text horizontally.
    Center,
    /// Align text to the right.
    Right,
}

/// Text wrapping behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// No wrapping; text may extend beyond the widget bounds.
    #[default]
    NoWrap,
    /// Wrap at character boundaries.
    Char,
    /// Wrap at word boundaries.
    Word,
}

/// How to handle text that exceeds available space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Truncation {
    /// No truncation; text may overflow.
    #[default]
    None,
    /// Truncate at the end with ellipsis.
    End,
    /// Truncate at the start with ellipsis.
    Start,
    /// Truncate in the middle with ellipsis.
    Middle,
}

/// A styled text span.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    /// The text content.
    pub text: String,
    /// The style for this span.
    pub style: Style,
}

impl TextSpan {
    /// Creates a new text span with default style.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::new(),
        }
    }

    /// Creates a new text span with the given style.
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    /// Sets the style for this span.
    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// Sets the foreground color.
    pub fn fg(mut self, color: Color) -> Self {
        self.style = self.style.fg(color);
        self
    }

    /// Sets the background color.
    pub fn bg(mut self, color: Color) -> Self {
        self.style = self.style.bg(color);
        self
    }

    /// Makes the text bold.
    pub fn bold(mut self) -> Self {
        self.style = self.style.bold();
        self
    }

    /// Makes the text italic.
    pub fn italic(mut self) -> Self {
        self.style = self.style.italic();
        self
    }

    /// Adds underline to the text.
    pub fn underline(mut self) -> Self {
        self.style = self.style.underline();
        self
    }

    /// Returns the character count.
    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// Returns the display width (accounting for wide characters).
    pub fn display_width(&self) -> usize {
        self.text.chars().map(char_display_width).sum()
    }
}

impl From<&str> for TextSpan {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for TextSpan {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// A widget that displays styled text.
///
/// The Text widget supports:
/// - Multiple styled spans
/// - Text alignment (left, center, right)
/// - Word and character wrapping
/// - Truncation with ellipsis
///
/// # Example
///
/// ```ignore
/// let text = TextWidget::builder()
///     .text("Hello, World!")
///     .align(TextAlign::Center)
///     .style(Style::new().fg(Color::rgb(1.0, 1.0, 0.0)).bold())
///     .build();
/// ```
#[derive(Debug)]
pub struct TextWidget {
    /// Unique identifier.
    id: WidgetId,
    /// Layout style.
    layout: LayoutStyle,
    /// Text spans to display.
    spans: Vec<TextSpan>,
    /// Text alignment.
    align: TextAlign,
    /// Wrap mode.
    wrap: WrapMode,
    /// Truncation behavior.
    truncation: Truncation,
    /// Ellipsis string for truncation.
    ellipsis: String,
    /// Default style applied to all spans.
    base_style: Style,
}

impl TextWidget {
    /// Creates a new empty text widget.
    pub fn new() -> Self {
        Self {
            id: WidgetId::new(),
            layout: LayoutStyle::default(),
            spans: Vec::new(),
            align: TextAlign::Left,
            wrap: WrapMode::NoWrap,
            truncation: Truncation::None,
            ellipsis: "…".to_string(),
            base_style: Style::new(),
        }
    }

    /// Creates a text widget with the given text.
    pub fn with_text(text: impl Into<String>) -> Self {
        let mut widget = Self::new();
        widget.spans.push(TextSpan::new(text));
        widget
    }

    /// Creates a builder for constructing a text widget.
    pub fn builder() -> TextWidgetBuilder {
        TextWidgetBuilder::new()
    }

    /// Sets the text content (replacing existing spans).
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.spans.clear();
        self.spans.push(TextSpan::new(text));
    }

    /// Adds a styled span.
    pub fn add_span(&mut self, span: TextSpan) {
        self.spans.push(span);
    }

    /// Clears all text.
    pub fn clear(&mut self) {
        self.spans.clear();
    }

    /// Sets the alignment.
    pub fn set_align(&mut self, align: TextAlign) {
        self.align = align;
    }

    /// Sets the wrap mode.
    pub fn set_wrap(&mut self, wrap: WrapMode) {
        self.wrap = wrap;
    }

    /// Sets the truncation mode.
    pub fn set_truncation(&mut self, truncation: Truncation) {
        self.truncation = truncation;
    }

    /// Sets the ellipsis string.
    pub fn set_ellipsis(&mut self, ellipsis: impl Into<String>) {
        self.ellipsis = ellipsis.into();
    }

    /// Sets the base style.
    pub fn set_base_style(&mut self, style: Style) {
        self.base_style = style;
    }

    /// Returns the text content as a single string.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Returns the spans.
    pub fn spans(&self) -> &[TextSpan] {
        &self.spans
    }

    /// Returns the total display width of the text.
    pub fn display_width(&self) -> usize {
        self.spans.iter().map(|s| s.display_width()).sum()
    }

    /// Renders a single line of text.
    fn render_line(&self, buffer: &mut Buffer, rect: Rect, line: &str, style: Style) {
        if rect.width == 0 {
            return;
        }

        let line_width = display_width(line);
        let available = rect.width as usize;

        // Handle truncation
        let (display_text, text_width) =
            if line_width > available && !matches!(self.truncation, Truncation::None) {
                let ellipsis_width = display_width(&self.ellipsis);
                if available <= ellipsis_width {
                    (self.ellipsis.clone(), ellipsis_width)
                } else {
                    let target_width = available - ellipsis_width;
                    match self.truncation {
                        Truncation::End => {
                            let truncated = truncate_end(line, target_width);
                            (
                                format!("{}{}", truncated, self.ellipsis),
                                available.min(target_width + ellipsis_width),
                            )
                        }
                        Truncation::Start => {
                            let truncated = truncate_start(line, target_width);
                            (
                                format!("{}{}", self.ellipsis, truncated),
                                available.min(target_width + ellipsis_width),
                            )
                        }
                        Truncation::Middle => {
                            let half = target_width / 2;
                            let start = truncate_end(line, half);
                            let end = truncate_start(line, target_width - half);
                            (format!("{}{}{}", start, self.ellipsis, end), available)
                        }
                        Truncation::None => (line.to_string(), line_width),
                    }
                }
            } else {
                (line.to_string(), line_width)
            };

        // Calculate x position based on alignment
        let x = match self.align {
            TextAlign::Left => rect.x,
            TextAlign::Center => rect.x + (rect.width.saturating_sub(text_width as u16) as i32) / 2,
            TextAlign::Right => rect.right().saturating_sub(text_width as i32),
        };

        buffer.set_string(x, rect.y, &display_text, style);
    }

    /// Wraps text into lines that fit the given width.
    fn wrap_text(&self, text: &str, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![];
        }

        match self.wrap {
            WrapMode::NoWrap => text.lines().map(String::from).collect(),
            WrapMode::Char => wrap_chars(text, width),
            WrapMode::Word => wrap_words(text, width),
        }
    }
}

impl Default for TextWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for TextWidget {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn type_name(&self) -> &'static str {
        "Text"
    }

    fn layout(&self) -> &LayoutStyle {
        &self.layout
    }

    fn layout_mut(&mut self) -> &mut LayoutStyle {
        &mut self.layout
    }

    fn render(&self, buffer: &mut Buffer, rect: Rect) {
        if rect.is_empty() || self.spans.is_empty() {
            return;
        }

        // Combine all spans into a single text for wrapping
        // (Full span-aware wrapping would require more complex logic)
        let full_text = self.text();
        let style = self
            .spans
            .first()
            .map(|s| self.base_style.merge(&s.style))
            .unwrap_or(self.base_style);

        let lines = self.wrap_text(&full_text, rect.width as usize);

        for (i, line) in lines.iter().enumerate() {
            if i as u16 >= rect.height {
                break;
            }
            let line_rect = Rect {
                x: rect.x,
                y: rect.y + i as i32,
                width: rect.width,
                height: 1,
            };
            self.render_line(buffer, line_rect, line, style);
        }
    }

    fn handle_event(&mut self, _event: &Event) -> EventResult {
        // Text widget is not interactive by default
        EventResult::Ignored
    }

    fn children(&self) -> &[WidgetRef] {
        &[]
    }

    fn children_mut(&mut self) -> &mut [WidgetRef] {
        &mut []
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn measure(&self, available_width: f32, available_height: f32) -> (f32, f32) {
        let text = self.text();

        match self.wrap {
            WrapMode::NoWrap => {
                let width = display_width(&text) as f32;
                let height = text.lines().count().max(1) as f32;
                (width.min(available_width), height.min(available_height))
            }
            WrapMode::Char | WrapMode::Word => {
                let lines = self.wrap_text(&text, available_width as usize);
                let max_width = lines.iter().map(|l| display_width(l)).max().unwrap_or(0) as f32;
                let height = lines.len() as f32;
                (max_width.min(available_width), height.min(available_height))
            }
        }
    }
}

/// Builder for constructing TextWidget instances.
#[derive(Debug)]
pub struct TextWidgetBuilder {
    widget: TextWidget,
}

impl TextWidgetBuilder {
    /// Creates a new builder.
    pub fn new() -> Self {
        Self {
            widget: TextWidget::new(),
        }
    }

    /// Sets the text content.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.widget.spans.clear();
        self.widget.spans.push(TextSpan::new(text));
        self
    }

    /// Adds a text span.
    pub fn span(mut self, span: TextSpan) -> Self {
        self.widget.spans.push(span);
        self
    }

    /// Adds a styled text span.
    pub fn styled(mut self, text: impl Into<String>, style: Style) -> Self {
        self.widget.spans.push(TextSpan::styled(text, style));
        self
    }

    /// Sets the text alignment.
    pub fn align(mut self, align: TextAlign) -> Self {
        self.widget.align = align;
        self
    }

    /// Aligns text to the left.
    pub fn left(self) -> Self {
        self.align(TextAlign::Left)
    }

    /// Centers text.
    pub fn center(self) -> Self {
        self.align(TextAlign::Center)
    }

    /// Aligns text to the right.
    pub fn right(self) -> Self {
        self.align(TextAlign::Right)
    }

    /// Sets the wrap mode.
    pub fn wrap(mut self, wrap: WrapMode) -> Self {
        self.widget.wrap = wrap;
        self
    }

    /// Enables character wrapping.
    pub fn wrap_char(self) -> Self {
        self.wrap(WrapMode::Char)
    }

    /// Enables word wrapping.
    pub fn wrap_word(self) -> Self {
        self.wrap(WrapMode::Word)
    }

    /// Disables wrapping.
    pub fn no_wrap(self) -> Self {
        self.wrap(WrapMode::NoWrap)
    }

    /// Sets the truncation mode.
    pub fn truncation(mut self, truncation: Truncation) -> Self {
        self.widget.truncation = truncation;
        self
    }

    /// Truncates at the end.
    pub fn truncate_end(self) -> Self {
        self.truncation(Truncation::End)
    }

    /// Truncates at the start.
    pub fn truncate_start(self) -> Self {
        self.truncation(Truncation::Start)
    }

    /// Truncates in the middle.
    pub fn truncate_middle(self) -> Self {
        self.truncation(Truncation::Middle)
    }

    /// Sets the ellipsis string.
    pub fn ellipsis(mut self, ellipsis: impl Into<String>) -> Self {
        self.widget.ellipsis = ellipsis.into();
        self
    }

    /// Sets the base style for all text.
    pub fn style(mut self, style: Style) -> Self {
        self.widget.base_style = style;
        self
    }

    /// Sets the foreground color.
    pub fn fg(mut self, color: Color) -> Self {
        self.widget.base_style = self.widget.base_style.fg(color);
        self
    }

    /// Sets the background color.
    pub fn bg(mut self, color: Color) -> Self {
        self.widget.base_style = self.widget.base_style.bg(color);
        self
    }

    /// Makes the text bold.
    pub fn bold(mut self) -> Self {
        self.widget.base_style = self.widget.base_style.bold();
        self
    }

    /// Makes the text italic.
    pub fn italic(mut self) -> Self {
        self.widget.base_style = self.widget.base_style.italic();
        self
    }

    /// Adds underline.
    pub fn underline(mut self) -> Self {
        self.widget.base_style = self.widget.base_style.underline();
        self
    }

    /// Sets the width.
    pub fn width(mut self, width: impl Into<Dimension>) -> Self {
        self.widget.layout.width = width.into();
        self
    }

    /// Sets the height.
    pub fn height(mut self, height: impl Into<Dimension>) -> Self {
        self.widget.layout.height = height.into();
        self
    }

    /// Sets the flex grow factor.
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.widget.layout.flex_grow = grow;
        self
    }

    /// Builds the TextWidget.
    pub fn build(self) -> TextWidget {
        self.widget
    }
}

impl Default for TextWidgetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Helper functions

/// Returns the display width of a character.
fn char_display_width(c: char) -> usize {
    if c.is_control() {
        0
    } else if is_wide_char(c) {
        2
    } else {
        1
    }
}

/// Returns true if the character is a wide character.
fn is_wide_char(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x1100..=0x115F |
        0x2E80..=0x9FFF |
        0xAC00..=0xD7A3 |
        0xF900..=0xFAFF |
        0xFE10..=0xFE1F |
        0xFE30..=0xFE6F |
        0xFF00..=0xFF60 |
        0xFFE0..=0xFFE6 |
        0x20000..=0x2FFFF |
        0x30000..=0x3FFFF
    )
}

/// Returns the display width of a string.
fn display_width(s: &str) -> usize {
    s.chars().map(char_display_width).sum()
}

/// Truncates a string from the end to fit within the given width.
fn truncate_end(s: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut result = String::new();

    for c in s.chars() {
        let char_width = char_display_width(c);
        if width + char_width > max_width {
            break;
        }
        width += char_width;
        result.push(c);
    }

    result
}

/// Truncates a string from the start to fit within the given width.
fn truncate_start(s: &str, max_width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut width = 0;
    let mut start_idx = chars.len();

    for (i, &c) in chars.iter().enumerate().rev() {
        let char_width = char_display_width(c);
        if width + char_width > max_width {
            break;
        }
        width += char_width;
        start_idx = i;
    }

    chars[start_idx..].iter().collect()
}

/// Wraps text at character boundaries.
fn wrap_chars(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for line in text.lines() {
        for c in line.chars() {
            let char_width = char_display_width(c);

            if current_width + char_width > width && !current_line.is_empty() {
                lines.push(current_line);
                current_line = String::new();
                current_width = 0;
            }

            current_line.push(c);
            current_width += char_width;
        }

        // End of input line
        if !current_line.is_empty() {
            lines.push(current_line);
            current_line = String::new();
            current_width = 0;
        } else {
            lines.push(String::new());
        }
    }

    // Handle trailing content
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Wraps text at word boundaries.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();

    for line in text.lines() {
        if line.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current_line = String::new();
        let mut current_width = 0;

        for word in line.split_whitespace() {
            let word_width = display_width(word);

            if current_width == 0 {
                // First word on line
                if word_width > width {
                    // Word is too long, use character wrapping
                    let wrapped = wrap_chars(word, width);
                    for (i, w) in wrapped.into_iter().enumerate() {
                        if i > 0 || !current_line.is_empty() {
                            lines.push(std::mem::take(&mut current_line));
                        }
                        current_line = w;
                        current_width = display_width(&current_line);
                    }
                } else {
                    current_line = word.to_string();
                    current_width = word_width;
                }
            } else if current_width + 1 + word_width <= width {
                // Word fits on current line with space
                current_line.push(' ');
                current_line.push_str(word);
                current_width += 1 + word_width;
            } else {
                // Start new line
                lines.push(current_line);
                current_line = String::new();
                current_width = 0;

                if word_width > width {
                    // Word is too long, use character wrapping
                    let wrapped = wrap_chars(word, width);
                    let wrapped_len = wrapped.len();
                    for (i, w) in wrapped.into_iter().enumerate() {
                        if i > 0 {
                            lines.push(std::mem::take(&mut current_line));
                        }
                        current_line = w;
                        current_width = display_width(&current_line);
                    }
                    // If the last wrapped segment should continue, don't push it yet
                    if wrapped_len > 0 && current_width == width {
                        // Line is full, but let the loop continue
                    }
                } else {
                    current_line = word.to_string();
                    current_width = word_width;
                }
            }
        }

        if !current_line.is_empty() {
            lines.push(current_line);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Creates a simple text widget.
pub fn text(content: impl Into<String>) -> TextWidget {
    TextWidget::with_text(content)
}

/// Creates a centered text widget.
pub fn centered_text(content: impl Into<String>) -> TextWidget {
    TextWidget::builder().text(content).center().build()
}

/// Creates a bold text widget.
pub fn bold_text(content: impl Into<String>) -> TextWidget {
    TextWidget::builder().text(content).bold().build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_widget_creation() {
        let widget = TextWidget::new();
        assert_eq!(widget.type_name(), "Text");
        assert!(widget.text().is_empty());
    }

    #[test]
    fn test_text_widget_with_text() {
        let widget = TextWidget::with_text("Hello, World!");
        assert_eq!(widget.text(), "Hello, World!");
    }

    #[test]
    fn test_text_widget_builder() {
        let widget = TextWidget::builder().text("Test").center().bold().build();

        assert_eq!(widget.text(), "Test");
        assert_eq!(widget.align, TextAlign::Center);
        assert!(widget.base_style.is_bold());
    }

    #[test]
    fn test_text_span() {
        let span = TextSpan::new("Hello").bold().fg(Color::rgb(1.0, 0.0, 0.0));
        assert_eq!(span.text, "Hello");
        assert!(span.style.is_bold());
        assert_eq!(span.style.fg, Some(Color::rgb(1.0, 0.0, 0.0)));
    }

    #[test]
    fn test_display_width() {
        assert_eq!(display_width("Hello"), 5);
        assert_eq!(display_width(""), 0);
        // Wide characters would be width 2 each
    }

    #[test]
    fn test_truncate_end() {
        assert_eq!(truncate_end("Hello, World!", 5), "Hello");
        assert_eq!(truncate_end("Hi", 10), "Hi");
    }

    #[test]
    fn test_truncate_start() {
        assert_eq!(truncate_start("Hello, World!", 6), "World!");
        assert_eq!(truncate_start("Hi", 10), "Hi");
    }

    #[test]
    fn test_wrap_chars() {
        let lines = wrap_chars("Hello World", 5);
        assert_eq!(lines, vec!["Hello", " Worl", "d"]);
    }

    #[test]
    fn test_wrap_words() {
        let lines = wrap_words("Hello World", 8);
        assert_eq!(lines, vec!["Hello", "World"]);
    }

    #[test]
    fn test_render_left_aligned() {
        let widget = TextWidget::builder().text("Test").left().build();

        let mut buffer = Buffer::new(10, 1);
        widget.render(&mut buffer, Rect::new(0, 0, 10, 1));

        assert_eq!(buffer.get(0, 0).unwrap().character, 'T');
        assert_eq!(buffer.get(3, 0).unwrap().character, 't');
    }

    #[test]
    fn test_render_centered() {
        let widget = TextWidget::builder().text("Hi").center().build();

        let mut buffer = Buffer::new(10, 1);
        widget.render(&mut buffer, Rect::new(0, 0, 10, 1));

        // "Hi" is 2 chars, centered in 10 should start at position 4
        assert_eq!(buffer.get(4, 0).unwrap().character, 'H');
        assert_eq!(buffer.get(5, 0).unwrap().character, 'i');
    }

    #[test]
    fn test_render_with_truncation() {
        let widget = TextWidget::builder()
            .text("Hello, World!")
            .truncate_end()
            .build();

        let mut buffer = Buffer::new(8, 1);
        widget.render(&mut buffer, Rect::new(0, 0, 8, 1));

        // Should show "Hello, …" (7 chars + 1 ellipsis)
        assert_eq!(buffer.get(0, 0).unwrap().character, 'H');
    }

    #[test]
    fn test_convenience_constructors() {
        let t = text("Hello");
        assert_eq!(t.text(), "Hello");

        let c = centered_text("Center");
        assert_eq!(c.align, TextAlign::Center);

        let b = bold_text("Bold");
        assert!(b.base_style.is_bold());
    }
}
