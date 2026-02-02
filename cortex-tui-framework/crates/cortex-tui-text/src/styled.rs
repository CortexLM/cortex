//! Styled text types for rich text rendering.
//!
//! This module provides types for representing text with styling information,
//! including colors, attributes (bold, italic, etc.), and composition utilities.

use crate::measurement::measure_width;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::fmt;
use std::ops::Add;

/// RGBA color with normalized 0.0-1.0 component values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    /// Red component (0.0-1.0).
    pub r: f32,
    /// Green component (0.0-1.0).
    pub g: f32,
    /// Blue component (0.0-1.0).
    pub b: f32,
    /// Alpha component (0.0-1.0).
    pub a: f32,
}

impl Color {
    /// Create a new color from normalized RGBA values.
    #[inline]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Create a fully opaque color from normalized RGB values.
    #[inline]
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::new(r, g, b, 1.0)
    }

    /// Create a color from 8-bit RGB values (0-255).
    #[inline]
    pub fn from_rgb_u8(r: u8, g: u8, b: u8) -> Self {
        Self::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
    }

    /// Create a color from 8-bit RGBA values (0-255).
    #[inline]
    pub fn from_rgba_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        )
    }

    /// Parse a color from a hex string.
    ///
    /// Supports formats: "#RGB", "#RGBA", "#RRGGBB", "#RRGGBBAA"
    /// The leading '#' is optional.
    ///
    /// # Example
    ///
    /// ```
    /// use cortex_tui_text::styled::Color;
    ///
    /// let red = Color::from_hex("#FF0000").unwrap();
    /// let green = Color::from_hex("00FF00").unwrap();
    /// ```
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');

        match hex.len() {
            3 => {
                // RGB format
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some(Self::from_rgb_u8(r, g, b))
            }
            4 => {
                // RGBA format
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                let a = u8::from_str_radix(&hex[3..4].repeat(2), 16).ok()?;
                Some(Self::from_rgba_u8(r, g, b, a))
            }
            6 => {
                // RRGGBB format
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Self::from_rgb_u8(r, g, b))
            }
            8 => {
                // RRGGBBAA format
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Self::from_rgba_u8(r, g, b, a))
            }
            _ => None,
        }
    }

    /// Convert to 8-bit RGB values.
    #[inline]
    pub fn to_rgb_u8(&self) -> (u8, u8, u8) {
        (
            (self.r * 255.0).round() as u8,
            (self.g * 255.0).round() as u8,
            (self.b * 255.0).round() as u8,
        )
    }

    /// Convert to 8-bit RGBA values.
    #[inline]
    pub fn to_rgba_u8(&self) -> (u8, u8, u8, u8) {
        (
            (self.r * 255.0).round() as u8,
            (self.g * 255.0).round() as u8,
            (self.b * 255.0).round() as u8,
            (self.a * 255.0).round() as u8,
        )
    }

    /// Convert to hex string (RRGGBB format).
    pub fn to_hex(&self) -> String {
        let (r, g, b) = self.to_rgb_u8();
        format!("#{:02X}{:02X}{:02X}", r, g, b)
    }

    // Predefined colors
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub const RED: Self = Self::rgb(1.0, 0.0, 0.0);
    pub const GREEN: Self = Self::rgb(0.0, 1.0, 0.0);
    pub const BLUE: Self = Self::rgb(0.0, 0.0, 1.0);
    pub const YELLOW: Self = Self::rgb(1.0, 1.0, 0.0);
    pub const CYAN: Self = Self::rgb(0.0, 1.0, 1.0);
    pub const MAGENTA: Self = Self::rgb(1.0, 0.0, 1.0);
    pub const GRAY: Self = Self::rgb(0.5, 0.5, 0.5);
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

/// Text attributes as a bitmask for efficient storage and composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextAttributes(u8);

impl TextAttributes {
    pub const NONE: Self = Self(0);
    pub const BOLD: Self = Self(1 << 0);
    pub const DIM: Self = Self(1 << 1);
    pub const ITALIC: Self = Self(1 << 2);
    pub const UNDERLINE: Self = Self(1 << 3);
    pub const BLINK: Self = Self(1 << 4);
    pub const INVERSE: Self = Self(1 << 5);
    pub const HIDDEN: Self = Self(1 << 6);
    pub const STRIKETHROUGH: Self = Self(1 << 7);

    /// Create empty attributes.
    #[inline]
    pub const fn empty() -> Self {
        Self::NONE
    }

    /// Check if any attributes are set.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Check if a specific attribute is set.
    #[inline]
    pub const fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Add an attribute.
    #[inline]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Remove an attribute.
    #[inline]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Toggle an attribute.
    #[inline]
    pub const fn toggle(self, other: Self) -> Self {
        Self(self.0 ^ other.0)
    }

    /// Merge with another set of attributes.
    #[inline]
    pub const fn merge(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Check if bold is set.
    #[inline]
    pub const fn is_bold(&self) -> bool {
        self.contains(Self::BOLD)
    }

    /// Check if dim is set.
    #[inline]
    pub const fn is_dim(&self) -> bool {
        self.contains(Self::DIM)
    }

    /// Check if italic is set.
    #[inline]
    pub const fn is_italic(&self) -> bool {
        self.contains(Self::ITALIC)
    }

    /// Check if underline is set.
    #[inline]
    pub const fn is_underline(&self) -> bool {
        self.contains(Self::UNDERLINE)
    }

    /// Check if blink is set.
    #[inline]
    pub const fn is_blink(&self) -> bool {
        self.contains(Self::BLINK)
    }

    /// Check if inverse is set.
    #[inline]
    pub const fn is_inverse(&self) -> bool {
        self.contains(Self::INVERSE)
    }

    /// Check if hidden is set.
    #[inline]
    pub const fn is_hidden(&self) -> bool {
        self.contains(Self::HIDDEN)
    }

    /// Check if strikethrough is set.
    #[inline]
    pub const fn is_strikethrough(&self) -> bool {
        self.contains(Self::STRIKETHROUGH)
    }
}

impl std::ops::BitOr for TextAttributes {
    type Output = Self;

    #[inline]
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for TextAttributes {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for TextAttributes {
    type Output = Self;

    #[inline]
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

/// Style combining colors and attributes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Style {
    /// Foreground (text) color.
    pub fg: Option<Color>,
    /// Background color.
    pub bg: Option<Color>,
    /// Text attributes (bold, italic, etc.).
    pub attributes: TextAttributes,
}

impl Style {
    /// Create an empty style with no colors or attributes.
    #[inline]
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            attributes: TextAttributes::NONE,
        }
    }

    /// Set the foreground color.
    #[inline]
    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// Set the background color.
    #[inline]
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Add an attribute.
    #[inline]
    pub const fn attr(mut self, attr: TextAttributes) -> Self {
        self.attributes = self.attributes.with(attr);
        self
    }

    /// Set bold attribute.
    #[inline]
    pub const fn bold(self) -> Self {
        self.attr(TextAttributes::BOLD)
    }

    /// Set dim attribute.
    #[inline]
    pub const fn dim(self) -> Self {
        self.attr(TextAttributes::DIM)
    }

    /// Set italic attribute.
    #[inline]
    pub const fn italic(self) -> Self {
        self.attr(TextAttributes::ITALIC)
    }

    /// Set underline attribute.
    #[inline]
    pub const fn underline(self) -> Self {
        self.attr(TextAttributes::UNDERLINE)
    }

    /// Set blink attribute.
    #[inline]
    pub const fn blink(self) -> Self {
        self.attr(TextAttributes::BLINK)
    }

    /// Set inverse attribute.
    #[inline]
    pub const fn inverse(self) -> Self {
        self.attr(TextAttributes::INVERSE)
    }

    /// Set hidden attribute.
    #[inline]
    pub const fn hidden(self) -> Self {
        self.attr(TextAttributes::HIDDEN)
    }

    /// Set strikethrough attribute.
    #[inline]
    pub const fn strikethrough(self) -> Self {
        self.attr(TextAttributes::STRIKETHROUGH)
    }

    /// Merge this style with another, with `other` taking precedence.
    pub fn merge(&self, other: &Style) -> Self {
        Self {
            fg: other.fg.or(self.fg),
            bg: other.bg.or(self.bg),
            attributes: self.attributes.merge(other.attributes),
        }
    }

    /// Check if this style has any styling applied.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.fg.is_none() && self.bg.is_none() && self.attributes.is_empty()
    }
}

/// A span of text with associated styling.
#[derive(Debug, Clone)]
pub struct Span<'a> {
    /// The text content.
    pub text: Cow<'a, str>,
    /// The style applied to this span.
    pub style: Style,
}

impl<'a> Span<'a> {
    /// Create a new unstyled span.
    #[inline]
    pub fn new(text: impl Into<Cow<'a, str>>) -> Self {
        Self {
            text: text.into(),
            style: Style::new(),
        }
    }

    /// Create a span with the given style.
    #[inline]
    pub fn styled(text: impl Into<Cow<'a, str>>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    /// Create a raw (unstyled) span.
    #[inline]
    pub fn raw(text: impl Into<Cow<'a, str>>) -> Self {
        Self::new(text)
    }

    /// Set the foreground color.
    #[inline]
    pub fn fg(mut self, color: Color) -> Self {
        self.style.fg = Some(color);
        self
    }

    /// Set the background color.
    #[inline]
    pub fn bg(mut self, color: Color) -> Self {
        self.style.bg = Some(color);
        self
    }

    /// Add bold styling.
    #[inline]
    pub fn bold(mut self) -> Self {
        self.style = self.style.bold();
        self
    }

    /// Add dim styling.
    #[inline]
    pub fn dim(mut self) -> Self {
        self.style = self.style.dim();
        self
    }

    /// Add italic styling.
    #[inline]
    pub fn italic(mut self) -> Self {
        self.style = self.style.italic();
        self
    }

    /// Add underline styling.
    #[inline]
    pub fn underline(mut self) -> Self {
        self.style = self.style.underline();
        self
    }

    /// Add strikethrough styling.
    #[inline]
    pub fn strikethrough(mut self) -> Self {
        self.style = self.style.strikethrough();
        self
    }

    /// Get the display width of this span.
    #[inline]
    pub fn width(&self) -> usize {
        measure_width(&self.text)
    }

    /// Check if the span is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Convert to owned span (static lifetime).
    pub fn into_owned(self) -> Span<'static> {
        Span {
            text: Cow::Owned(self.text.into_owned()),
            style: self.style,
        }
    }
}

impl<'a> fmt::Display for Span<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.text)
    }
}

impl<'a> From<&'a str> for Span<'a> {
    fn from(s: &'a str) -> Self {
        Self::new(s)
    }
}

impl From<String> for Span<'static> {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// A sequence of styled text spans.
///
/// This is the primary type for representing rich text with multiple
/// styles in a single logical unit.
#[derive(Debug, Clone, Default)]
pub struct StyledText<'a> {
    spans: SmallVec<[Span<'a>; 4]>,
}

impl<'a> StyledText<'a> {
    /// Create empty styled text.
    #[inline]
    pub fn new() -> Self {
        Self {
            spans: SmallVec::new(),
        }
    }

    /// Create styled text from a single span.
    #[inline]
    pub fn from_span(span: Span<'a>) -> Self {
        let mut st = Self::new();
        st.spans.push(span);
        st
    }

    /// Create unstyled text.
    #[inline]
    pub fn plain(text: impl Into<Cow<'a, str>>) -> Self {
        Self::from_span(Span::new(text))
    }

    /// Push a span to the end.
    #[inline]
    pub fn push(&mut self, span: Span<'a>) {
        if !span.is_empty() {
            self.spans.push(span);
        }
    }

    /// Push raw text (unstyled).
    #[inline]
    pub fn push_str(&mut self, text: impl Into<Cow<'a, str>>) {
        self.push(Span::new(text));
    }

    /// Get all spans.
    #[inline]
    pub fn spans(&self) -> &[Span<'a>] {
        &self.spans
    }

    /// Get mutable access to spans.
    #[inline]
    pub fn spans_mut(&mut self) -> &mut [Span<'a>] {
        &mut self.spans
    }

    /// Iterate over spans.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &Span<'a>> {
        self.spans.iter()
    }

    /// Check if empty (no spans or all spans empty).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty() || self.spans.iter().all(|s| s.is_empty())
    }

    /// Get the plain text content (all spans concatenated).
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_ref()).collect()
    }

    /// Calculate total display width.
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.width()).sum()
    }

    /// Get number of spans.
    #[inline]
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Clear all spans.
    #[inline]
    pub fn clear(&mut self) {
        self.spans.clear();
    }

    /// Extend with spans from another StyledText.
    #[inline]
    pub fn extend(&mut self, other: StyledText<'a>) {
        self.spans.extend(other.spans);
    }

    /// Convert to owned (static lifetime).
    pub fn into_owned(self) -> StyledText<'static> {
        StyledText {
            spans: self.spans.into_iter().map(|s| s.into_owned()).collect(),
        }
    }
}

impl<'a> fmt::Display for StyledText<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for span in &self.spans {
            write!(f, "{}", span.text)?;
        }
        Ok(())
    }
}

impl<'a> Add for StyledText<'a> {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self::Output {
        self.extend(rhs);
        self
    }
}

impl<'a, S: Into<Span<'a>>> FromIterator<S> for StyledText<'a> {
    fn from_iter<I: IntoIterator<Item = S>>(iter: I) -> Self {
        let mut st = StyledText::new();
        for item in iter {
            st.push(item.into());
        }
        st
    }
}

impl<'a> IntoIterator for StyledText<'a> {
    type Item = Span<'a>;
    type IntoIter = smallvec::IntoIter<[Span<'a>; 4]>;

    fn into_iter(self) -> Self::IntoIter {
        self.spans.into_iter()
    }
}

/// Builder for constructing styled text with a fluent API.
#[derive(Debug, Default)]
pub struct StyledTextBuilder<'a> {
    styled: StyledText<'a>,
    current_style: Style,
}

impl<'a> StyledTextBuilder<'a> {
    /// Create a new builder.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add plain text with the current style.
    pub fn text(mut self, text: impl Into<Cow<'a, str>>) -> Self {
        self.styled.push(Span::styled(text, self.current_style));
        self
    }

    /// Add a pre-styled span.
    pub fn span(mut self, span: Span<'a>) -> Self {
        self.styled.push(span);
        self
    }

    /// Set the foreground color for subsequent text.
    pub fn fg(mut self, color: Color) -> Self {
        self.current_style.fg = Some(color);
        self
    }

    /// Set the background color for subsequent text.
    pub fn bg(mut self, color: Color) -> Self {
        self.current_style.bg = Some(color);
        self
    }

    /// Enable bold for subsequent text.
    pub fn bold(mut self) -> Self {
        self.current_style = self.current_style.bold();
        self
    }

    /// Enable italic for subsequent text.
    pub fn italic(mut self) -> Self {
        self.current_style = self.current_style.italic();
        self
    }

    /// Enable underline for subsequent text.
    pub fn underline(mut self) -> Self {
        self.current_style = self.current_style.underline();
        self
    }

    /// Enable dim for subsequent text.
    pub fn dim(mut self) -> Self {
        self.current_style = self.current_style.dim();
        self
    }

    /// Reset the current style to default.
    pub fn reset(mut self) -> Self {
        self.current_style = Style::new();
        self
    }

    /// Build the final styled text.
    pub fn build(self) -> StyledText<'a> {
        self.styled
    }
}

// Convenience functions for creating styled spans

/// Create a bold span.
#[inline]
pub fn bold<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    Span::new(text).bold()
}

/// Create an italic span.
#[inline]
pub fn italic<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    Span::new(text).italic()
}

/// Create an underlined span.
#[inline]
pub fn underline<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    Span::new(text).underline()
}

/// Create a dim span.
#[inline]
pub fn dim<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    Span::new(text).dim()
}

/// Create a span with foreground color.
#[inline]
pub fn fg<'a>(color: Color, text: impl Into<Cow<'a, str>>) -> Span<'a> {
    Span::new(text).fg(color)
}

/// Create a span with background color.
#[inline]
pub fn bg<'a>(color: Color, text: impl Into<Cow<'a, str>>) -> Span<'a> {
    Span::new(text).bg(color)
}

/// Create a red foreground span.
#[inline]
pub fn red<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    fg(Color::RED, text)
}

/// Create a green foreground span.
#[inline]
pub fn green<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    fg(Color::GREEN, text)
}

/// Create a blue foreground span.
#[inline]
pub fn blue<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    fg(Color::BLUE, text)
}

/// Create a yellow foreground span.
#[inline]
pub fn yellow<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    fg(Color::YELLOW, text)
}

/// Create a cyan foreground span.
#[inline]
pub fn cyan<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    fg(Color::CYAN, text)
}

/// Create a magenta foreground span.
#[inline]
pub fn magenta<'a>(text: impl Into<Cow<'a, str>>) -> Span<'a> {
    fg(Color::MAGENTA, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_from_hex() {
        let red = Color::from_hex("#FF0000").unwrap();
        assert_eq!(red.to_rgb_u8(), (255, 0, 0));

        let green = Color::from_hex("00FF00").unwrap();
        assert_eq!(green.to_rgb_u8(), (0, 255, 0));

        let short = Color::from_hex("#F00").unwrap();
        assert_eq!(short.to_rgb_u8(), (255, 0, 0));
    }

    #[test]
    fn test_color_to_hex() {
        assert_eq!(Color::RED.to_hex(), "#FF0000");
        assert_eq!(Color::GREEN.to_hex(), "#00FF00");
        assert_eq!(Color::BLUE.to_hex(), "#0000FF");
    }

    #[test]
    fn test_text_attributes() {
        let attrs = TextAttributes::BOLD | TextAttributes::ITALIC;
        assert!(attrs.is_bold());
        assert!(attrs.is_italic());
        assert!(!attrs.is_underline());
    }

    #[test]
    fn test_style_builder() {
        let style = Style::new().fg(Color::RED).bold().underline();
        assert_eq!(style.fg, Some(Color::RED));
        assert!(style.attributes.is_bold());
        assert!(style.attributes.is_underline());
    }

    #[test]
    fn test_span_creation() {
        let span = Span::new("Hello").bold().fg(Color::RED);
        assert_eq!(span.text, "Hello");
        assert!(span.style.attributes.is_bold());
        assert_eq!(span.style.fg, Some(Color::RED));
    }

    #[test]
    fn test_styled_text_plain() {
        let st = StyledText::plain("Hello World");
        assert_eq!(st.plain_text(), "Hello World");
        assert_eq!(st.len(), 1);
    }

    #[test]
    fn test_styled_text_builder() {
        let st = StyledTextBuilder::new()
            .text("Hello ")
            .bold()
            .fg(Color::RED)
            .text("World")
            .reset()
            .text("!")
            .build();

        assert_eq!(st.plain_text(), "Hello World!");
        assert_eq!(st.len(), 3);
    }

    #[test]
    fn test_styled_text_concat() {
        let st1 = StyledText::plain("Hello ");
        let st2 = StyledText::plain("World");
        let st3 = st1 + st2;
        assert_eq!(st3.plain_text(), "Hello World");
    }

    #[test]
    fn test_styled_text_width() {
        let st = StyledText::plain("Hello");
        assert_eq!(st.width(), 5);

        let st = StyledText::plain("日本語");
        assert_eq!(st.width(), 6);
    }

    #[test]
    fn test_span_helpers() {
        let s = bold("Hello");
        assert!(s.style.attributes.is_bold());

        let s = red("World");
        assert_eq!(s.style.fg, Some(Color::RED));
    }

    #[test]
    fn test_styled_text_from_iter() {
        let spans = vec![Span::new("Hello "), Span::new("World").bold()];
        let st: StyledText = spans.into_iter().collect();
        assert_eq!(st.plain_text(), "Hello World");
        assert_eq!(st.len(), 2);
    }

    #[test]
    fn test_styled_text_into_owned() {
        let text = String::from("Hello World");
        let st = StyledText::plain(&text);
        let owned = st.into_owned();
        assert_eq!(owned.plain_text(), "Hello World");
    }
}
