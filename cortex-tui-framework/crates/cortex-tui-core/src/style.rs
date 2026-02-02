//! Text styling for terminal UI rendering.
//!
//! This module provides types for styling terminal text with colors and attributes.
//! The main types are:
//!
//! - [`TextAttributes`]: Bitflags for text decorations (bold, italic, underline, etc.)
//! - [`Style`]: Complete styling information including colors and attributes
//!
//! # Examples
//!
//! ```
//! use cortex_tui_core::style::{Style, TextAttributes};
//! use cortex_tui_core::color::Color;
//!
//! // Create a style with red text on black background, bold
//! let style = Style::new()
//!     .fg(Color::RED)
//!     .bg(Color::BLACK)
//!     .bold();
//!
//! // Combine styles (later style takes precedence)
//! let base = Style::new().fg(Color::WHITE);
//! let highlight = Style::new().bg(Color::YELLOW).bold();
//! let combined = base.merge(&highlight);
//! ```

use crate::color::Color;
use bitflags::bitflags;
use std::fmt;

bitflags! {
    /// Text decoration attributes as a compact bitfield.
    ///
    /// These attributes can be combined using bitwise operations:
    ///
    /// ```
    /// use cortex_tui_core::style::TextAttributes;
    ///
    /// let attrs = TextAttributes::BOLD | TextAttributes::UNDERLINE;
    /// assert!(attrs.contains(TextAttributes::BOLD));
    /// assert!(attrs.contains(TextAttributes::UNDERLINE));
    /// assert!(!attrs.contains(TextAttributes::ITALIC));
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct TextAttributes: u8 {
        /// Bold/bright text.
        const BOLD          = 0b0000_0001;
        /// Dim/faint text.
        const DIM           = 0b0000_0010;
        /// Italic text.
        const ITALIC        = 0b0000_0100;
        /// Underlined text.
        const UNDERLINE     = 0b0000_1000;
        /// Blinking text (rarely supported in modern terminals).
        const BLINK         = 0b0001_0000;
        /// Reverse/inverse video (swap fg and bg colors).
        const REVERSE       = 0b0010_0000;
        /// Hidden/invisible text.
        const HIDDEN        = 0b0100_0000;
        /// Strikethrough text.
        const STRIKETHROUGH = 0b1000_0000;
    }
}

impl TextAttributes {
    /// No attributes set (alias for `empty()`).
    pub const NONE: Self = Self::empty();

    /// Returns `true` if no attributes are set.
    #[inline]
    pub fn is_none(self) -> bool {
        self.is_empty()
    }

    /// Returns `true` if any attribute is set.
    #[inline]
    pub fn is_some(self) -> bool {
        !self.is_empty()
    }

    /// Returns the ANSI SGR codes for these attributes.
    ///
    /// Each attribute maps to its corresponding ANSI code:
    /// - Bold: 1
    /// - Dim: 2
    /// - Italic: 3
    /// - Underline: 4
    /// - Blink: 5
    /// - Reverse: 7
    /// - Hidden: 8
    /// - Strikethrough: 9
    pub fn to_ansi_codes(&self) -> smallvec::SmallVec<[u8; 8]> {
        let mut codes = smallvec::SmallVec::new();

        if self.contains(Self::BOLD) {
            codes.push(1);
        }
        if self.contains(Self::DIM) {
            codes.push(2);
        }
        if self.contains(Self::ITALIC) {
            codes.push(3);
        }
        if self.contains(Self::UNDERLINE) {
            codes.push(4);
        }
        if self.contains(Self::BLINK) {
            codes.push(5);
        }
        if self.contains(Self::REVERSE) {
            codes.push(7);
        }
        if self.contains(Self::HIDDEN) {
            codes.push(8);
        }
        if self.contains(Self::STRIKETHROUGH) {
            codes.push(9);
        }

        codes
    }

    /// Generates the ANSI escape sequences for these attributes.
    pub fn to_ansi_string(&self) -> String {
        let codes = self.to_ansi_codes();
        if codes.is_empty() {
            return String::new();
        }

        codes.iter().map(|code| format!("\x1b[{}m", code)).collect()
    }

    /// Writes the ANSI escape sequences to the provided buffer.
    pub fn write_ansi(&self, buf: &mut Vec<u8>) {
        if self.contains(Self::BOLD) {
            buf.extend_from_slice(b"\x1b[1m");
        }
        if self.contains(Self::DIM) {
            buf.extend_from_slice(b"\x1b[2m");
        }
        if self.contains(Self::ITALIC) {
            buf.extend_from_slice(b"\x1b[3m");
        }
        if self.contains(Self::UNDERLINE) {
            buf.extend_from_slice(b"\x1b[4m");
        }
        if self.contains(Self::BLINK) {
            buf.extend_from_slice(b"\x1b[5m");
        }
        if self.contains(Self::REVERSE) {
            buf.extend_from_slice(b"\x1b[7m");
        }
        if self.contains(Self::HIDDEN) {
            buf.extend_from_slice(b"\x1b[8m");
        }
        if self.contains(Self::STRIKETHROUGH) {
            buf.extend_from_slice(b"\x1b[9m");
        }
    }
}

impl fmt::Display for TextAttributes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.contains(Self::BOLD) {
            parts.push("bold");
        }
        if self.contains(Self::DIM) {
            parts.push("dim");
        }
        if self.contains(Self::ITALIC) {
            parts.push("italic");
        }
        if self.contains(Self::UNDERLINE) {
            parts.push("underline");
        }
        if self.contains(Self::BLINK) {
            parts.push("blink");
        }
        if self.contains(Self::REVERSE) {
            parts.push("reverse");
        }
        if self.contains(Self::HIDDEN) {
            parts.push("hidden");
        }
        if self.contains(Self::STRIKETHROUGH) {
            parts.push("strikethrough");
        }

        if parts.is_empty() {
            write!(f, "none")
        } else {
            write!(f, "{}", parts.join(", "))
        }
    }
}

/// Complete style information for terminal text.
///
/// A `Style` combines:
/// - Optional foreground color
/// - Optional background color
/// - Text attributes (bold, italic, etc.)
///
/// Styles can be merged, with later values overriding earlier ones.
///
/// # Examples
///
/// ```
/// use cortex_tui_core::style::Style;
/// use cortex_tui_core::color::Color;
///
/// // Create styles using builder pattern
/// let error_style = Style::new()
///     .fg(Color::RED)
///     .bold();
///
/// let warning_style = Style::new()
///     .fg(Color::YELLOW)
///     .bg(Color::BLACK);
///
/// // Apply a style patch
/// let highlighted = error_style.merge(&Style::new().underline());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Style {
    /// Foreground (text) color. `None` means use the terminal's default.
    pub fg: Option<Color>,
    /// Background color. `None` means use the terminal's default.
    pub bg: Option<Color>,
    /// Text decoration attributes.
    pub attributes: TextAttributes,
}

impl Style {
    /// Creates a new empty style with no colors and no attributes.
    #[inline]
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            attributes: TextAttributes::empty(),
        }
    }

    /// Creates a style with the given foreground and background colors.
    #[inline]
    pub const fn with_colors(fg: Color, bg: Color) -> Self {
        Self {
            fg: Some(fg),
            bg: Some(bg),
            attributes: TextAttributes::empty(),
        }
    }

    /// Creates a style with the given foreground color only.
    #[inline]
    pub const fn with_fg(fg: Color) -> Self {
        Self {
            fg: Some(fg),
            bg: None,
            attributes: TextAttributes::empty(),
        }
    }

    /// Creates a style with the given background color only.
    #[inline]
    pub const fn with_bg(bg: Color) -> Self {
        Self {
            fg: None,
            bg: Some(bg),
            attributes: TextAttributes::empty(),
        }
    }

    /// Creates a style with the given attributes only.
    #[inline]
    pub const fn with_attributes(attributes: TextAttributes) -> Self {
        Self {
            fg: None,
            bg: None,
            attributes,
        }
    }

    /// Returns `true` if this style has no colors and no attributes set.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.fg.is_none() && self.bg.is_none() && self.attributes.is_empty()
    }

    /// Returns `true` if any style property is set.
    #[inline]
    pub fn is_set(&self) -> bool {
        !self.is_empty()
    }

    // ========================================================================
    // Builder methods for colors
    // ========================================================================

    /// Sets the foreground color.
    #[inline]
    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// Sets the background color.
    #[inline]
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Clears the foreground color (use terminal default).
    #[inline]
    pub const fn clear_fg(mut self) -> Self {
        self.fg = None;
        self
    }

    /// Clears the background color (use terminal default).
    #[inline]
    pub const fn clear_bg(mut self) -> Self {
        self.bg = None;
        self
    }

    // ========================================================================
    // Builder methods for attributes
    // ========================================================================

    /// Adds the specified attributes to the style.
    #[inline]
    pub const fn add_attributes(mut self, attrs: TextAttributes) -> Self {
        self.attributes = self.attributes.union(attrs);
        self
    }

    /// Removes the specified attributes from the style.
    #[inline]
    pub const fn remove_attributes(mut self, attrs: TextAttributes) -> Self {
        self.attributes = self.attributes.difference(attrs);
        self
    }

    /// Sets the attributes, replacing any existing ones.
    #[inline]
    pub const fn set_attributes(mut self, attrs: TextAttributes) -> Self {
        self.attributes = attrs;
        self
    }

    /// Clears all attributes.
    #[inline]
    pub const fn clear_attributes(mut self) -> Self {
        self.attributes = TextAttributes::empty();
        self
    }

    /// Adds the bold attribute.
    #[inline]
    pub const fn bold(self) -> Self {
        self.add_attributes(TextAttributes::BOLD)
    }

    /// Adds the dim attribute.
    #[inline]
    pub const fn dim(self) -> Self {
        self.add_attributes(TextAttributes::DIM)
    }

    /// Adds the italic attribute.
    #[inline]
    pub const fn italic(self) -> Self {
        self.add_attributes(TextAttributes::ITALIC)
    }

    /// Adds the underline attribute.
    #[inline]
    pub const fn underline(self) -> Self {
        self.add_attributes(TextAttributes::UNDERLINE)
    }

    /// Adds the blink attribute.
    #[inline]
    pub const fn blink(self) -> Self {
        self.add_attributes(TextAttributes::BLINK)
    }

    /// Adds the reverse/inverse attribute.
    #[inline]
    pub const fn reverse(self) -> Self {
        self.add_attributes(TextAttributes::REVERSE)
    }

    /// Adds the hidden attribute.
    #[inline]
    pub const fn hidden(self) -> Self {
        self.add_attributes(TextAttributes::HIDDEN)
    }

    /// Adds the strikethrough attribute.
    #[inline]
    pub const fn strikethrough(self) -> Self {
        self.add_attributes(TextAttributes::STRIKETHROUGH)
    }

    // ========================================================================
    // Attribute queries
    // ========================================================================

    /// Returns `true` if the bold attribute is set.
    #[inline]
    pub const fn is_bold(&self) -> bool {
        self.attributes.contains(TextAttributes::BOLD)
    }

    /// Returns `true` if the dim attribute is set.
    #[inline]
    pub const fn is_dim(&self) -> bool {
        self.attributes.contains(TextAttributes::DIM)
    }

    /// Returns `true` if the italic attribute is set.
    #[inline]
    pub const fn is_italic(&self) -> bool {
        self.attributes.contains(TextAttributes::ITALIC)
    }

    /// Returns `true` if the underline attribute is set.
    #[inline]
    pub const fn is_underline(&self) -> bool {
        self.attributes.contains(TextAttributes::UNDERLINE)
    }

    /// Returns `true` if the blink attribute is set.
    #[inline]
    pub const fn is_blink(&self) -> bool {
        self.attributes.contains(TextAttributes::BLINK)
    }

    /// Returns `true` if the reverse attribute is set.
    #[inline]
    pub const fn is_reverse(&self) -> bool {
        self.attributes.contains(TextAttributes::REVERSE)
    }

    /// Returns `true` if the hidden attribute is set.
    #[inline]
    pub const fn is_hidden(&self) -> bool {
        self.attributes.contains(TextAttributes::HIDDEN)
    }

    /// Returns `true` if the strikethrough attribute is set.
    #[inline]
    pub const fn is_strikethrough(&self) -> bool {
        self.attributes.contains(TextAttributes::STRIKETHROUGH)
    }

    // ========================================================================
    // Style combination
    // ========================================================================

    /// Merges another style into this one.
    ///
    /// Values from `other` override values in `self`:
    /// - If `other` has a foreground color, it replaces `self`'s
    /// - If `other` has a background color, it replaces `self`'s
    /// - Attributes from `other` are added to `self`'s attributes
    ///
    /// # Examples
    ///
    /// ```
    /// use cortex_tui_core::style::Style;
    /// use cortex_tui_core::color::Color;
    ///
    /// let base = Style::new().fg(Color::WHITE).bg(Color::BLACK);
    /// let patch = Style::new().fg(Color::RED).bold();
    /// let merged = base.merge(&patch);
    ///
    /// assert_eq!(merged.fg, Some(Color::RED)); // From patch
    /// assert_eq!(merged.bg, Some(Color::BLACK)); // From base
    /// assert!(merged.is_bold()); // From patch
    /// ```
    #[inline]
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            fg: other.fg.or(self.fg),
            bg: other.bg.or(self.bg),
            attributes: self.attributes | other.attributes,
        }
    }

    /// Applies another style as a patch, modifying this style in place.
    #[inline]
    pub fn apply(&mut self, other: &Self) {
        if other.fg.is_some() {
            self.fg = other.fg;
        }
        if other.bg.is_some() {
            self.bg = other.bg;
        }
        self.attributes |= other.attributes;
    }

    /// Combines this style with another, preferring values from `other`.
    ///
    /// Unlike `merge`, this method replaces colors only if `other` explicitly
    /// sets them (i.e., has `Some` value).
    #[inline]
    pub fn overlay(&self, other: &Self) -> Self {
        Self {
            fg: other.fg.or(self.fg),
            bg: other.bg.or(self.bg),
            attributes: other.attributes | self.attributes,
        }
    }

    /// Returns a new style with only the non-empty values from this style.
    #[inline]
    pub fn compact(&self) -> Self {
        *self
    }

    /// Returns a style that, when merged with `base`, produces `target`.
    ///
    /// This is useful for computing style "diffs".
    pub fn diff(&self, target: &Self) -> Self {
        let mut diff = Style::new();

        if target.fg != self.fg {
            diff.fg = target.fg;
        }
        if target.bg != self.bg {
            diff.bg = target.bg;
        }

        // Attributes that need to be added
        let added_attrs = target.attributes - self.attributes;
        diff.attributes = added_attrs;

        diff
    }

    /// Returns `true` if this style would produce the same visual output as `other`.
    #[inline]
    pub fn visual_eq(&self, other: &Self) -> bool {
        let fg_eq = match (self.fg, other.fg) {
            (Some(a), Some(b)) => a.approx_eq(&b, 0.001),
            (None, None) => true,
            _ => false,
        };

        let bg_eq = match (self.bg, other.bg) {
            (Some(a), Some(b)) => a.approx_eq(&b, 0.001),
            (None, None) => true,
            _ => false,
        };

        fg_eq && bg_eq && self.attributes == other.attributes
    }

    // ========================================================================
    // ANSI escape sequence generation
    // ========================================================================

    /// Generates the complete ANSI escape sequence for this style.
    ///
    /// The generated sequence includes:
    /// - Reset code
    /// - Foreground color (if set)
    /// - Background color (if set)
    /// - Text attributes
    pub fn to_ansi(&self) -> String {
        let mut result = String::new();

        if let Some(fg) = self.fg {
            result.push_str(&fg.to_ansi_fg());
        }

        if let Some(bg) = self.bg {
            result.push_str(&bg.to_ansi_bg());
        }

        result.push_str(&self.attributes.to_ansi_string());

        result
    }

    /// Writes the ANSI escape sequences to the provided buffer.
    pub fn write_ansi(&self, buf: &mut Vec<u8>) {
        use std::io::Write;

        if let Some(fg) = self.fg {
            let (r, g, b, _) = fg.to_rgba_u8();
            let _ = write!(buf, "\x1b[38;2;{};{};{}m", r, g, b);
        }

        if let Some(bg) = self.bg {
            if bg.is_transparent() {
                buf.extend_from_slice(b"\x1b[49m");
            } else {
                let (r, g, b, _) = bg.to_rgba_u8();
                let _ = write!(buf, "\x1b[48;2;{};{};{}m", r, g, b);
            }
        }

        self.attributes.write_ansi(buf);
    }

    /// Generates the ANSI sequence needed to transition from one style to another.
    ///
    /// This is more efficient than resetting and applying the full new style
    /// when only some attributes have changed.
    pub fn transition_to(&self, target: &Style, buf: &mut Vec<u8>) {
        use std::io::Write;

        // Check if we need to reset (when removing attributes)
        let removed_attrs = self.attributes - target.attributes;
        if !removed_attrs.is_empty() {
            // We need to reset and reapply
            buf.extend_from_slice(b"\x1b[0m");
            target.write_ansi(buf);
            return;
        }

        // Check foreground color
        let fg_changed = match (self.fg, target.fg) {
            (Some(a), Some(b)) => !a.approx_eq(&b, 0.001),
            (None, Some(_)) => true,
            (Some(_), None) => true,
            (None, None) => false,
        };

        if fg_changed {
            if let Some(fg) = target.fg {
                let (r, g, b, _) = fg.to_rgba_u8();
                let _ = write!(buf, "\x1b[38;2;{};{};{}m", r, g, b);
            } else {
                buf.extend_from_slice(b"\x1b[39m"); // Default fg
            }
        }

        // Check background color
        let bg_changed = match (self.bg, target.bg) {
            (Some(a), Some(b)) => !a.approx_eq(&b, 0.001),
            (None, Some(_)) => true,
            (Some(_), None) => true,
            (None, None) => false,
        };

        if bg_changed {
            if let Some(bg) = target.bg {
                if bg.is_transparent() {
                    buf.extend_from_slice(b"\x1b[49m");
                } else {
                    let (r, g, b, _) = bg.to_rgba_u8();
                    let _ = write!(buf, "\x1b[48;2;{};{};{}m", r, g, b);
                }
            } else {
                buf.extend_from_slice(b"\x1b[49m"); // Default bg
            }
        }

        // Add new attributes (we've already handled removal above)
        let added_attrs = target.attributes - self.attributes;
        added_attrs.write_ansi(buf);
    }
}

impl fmt::Display for Style {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Style(")?;

        let mut parts = Vec::new();

        if let Some(fg) = self.fg {
            parts.push(format!("fg: {}", fg.to_hex()));
        }
        if let Some(bg) = self.bg {
            parts.push(format!("bg: {}", bg.to_hex()));
        }
        if !self.attributes.is_empty() {
            parts.push(format!("attrs: {}", self.attributes));
        }

        if parts.is_empty() {
            write!(f, "none")?;
        } else {
            write!(f, "{}", parts.join(", "))?;
        }

        write!(f, ")")
    }
}

// ============================================================================
// Common style presets
// ============================================================================

impl Style {
    /// A reset style that clears all formatting.
    pub const RESET: &'static str = "\x1b[0m";

    /// Default terminal style (no colors, no attributes).
    pub const DEFAULT: Self = Self::new();

    /// Bold text style.
    pub const BOLD: Self = Self {
        fg: None,
        bg: None,
        attributes: TextAttributes::BOLD,
    };

    /// Italic text style.
    pub const ITALIC: Self = Self {
        fg: None,
        bg: None,
        attributes: TextAttributes::ITALIC,
    };

    /// Underlined text style.
    pub const UNDERLINE: Self = Self {
        fg: None,
        bg: None,
        attributes: TextAttributes::UNDERLINE,
    };

    /// Dim text style.
    pub const DIM: Self = Self {
        fg: None,
        bg: None,
        attributes: TextAttributes::DIM,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    mod text_attributes_tests {
        use super::*;

        #[test]
        fn test_attributes_empty() {
            let attrs = TextAttributes::empty();
            assert!(attrs.is_none());
            assert!(!attrs.is_some());
        }

        #[test]
        fn test_attributes_single() {
            let attrs = TextAttributes::BOLD;
            assert!(!attrs.is_none());
            assert!(attrs.is_some());
            assert!(attrs.contains(TextAttributes::BOLD));
        }

        #[test]
        fn test_attributes_combine() {
            let attrs = TextAttributes::BOLD | TextAttributes::ITALIC | TextAttributes::UNDERLINE;
            assert!(attrs.contains(TextAttributes::BOLD));
            assert!(attrs.contains(TextAttributes::ITALIC));
            assert!(attrs.contains(TextAttributes::UNDERLINE));
            assert!(!attrs.contains(TextAttributes::DIM));
        }

        #[test]
        fn test_to_ansi_codes() {
            let attrs = TextAttributes::BOLD | TextAttributes::UNDERLINE;
            let codes = attrs.to_ansi_codes();
            assert!(codes.contains(&1)); // Bold
            assert!(codes.contains(&4)); // Underline
            assert_eq!(codes.len(), 2);
        }

        #[test]
        fn test_to_ansi_string() {
            let attrs = TextAttributes::BOLD;
            let ansi = attrs.to_ansi_string();
            assert!(ansi.contains("\x1b[1m"));
        }

        #[test]
        fn test_display() {
            let attrs = TextAttributes::BOLD | TextAttributes::ITALIC;
            let display = attrs.to_string();
            assert!(display.contains("bold"));
            assert!(display.contains("italic"));
        }

        #[test]
        fn test_display_none() {
            let attrs = TextAttributes::empty();
            assert_eq!(attrs.to_string(), "none");
        }
    }

    mod style_tests {
        use super::*;

        #[test]
        fn test_style_new() {
            let style = Style::new();
            assert!(style.is_empty());
            assert!(style.fg.is_none());
            assert!(style.bg.is_none());
            assert!(style.attributes.is_empty());
        }

        #[test]
        fn test_style_with_colors() {
            let style = Style::with_colors(Color::RED, Color::BLACK);
            assert_eq!(style.fg, Some(Color::RED));
            assert_eq!(style.bg, Some(Color::BLACK));
        }

        #[test]
        fn test_style_builder() {
            let style = Style::new()
                .fg(Color::RED)
                .bg(Color::BLACK)
                .bold()
                .underline();

            assert_eq!(style.fg, Some(Color::RED));
            assert_eq!(style.bg, Some(Color::BLACK));
            assert!(style.is_bold());
            assert!(style.is_underline());
            assert!(!style.is_italic());
        }

        #[test]
        fn test_style_merge() {
            let base = Style::new().fg(Color::WHITE).bg(Color::BLACK);
            let patch = Style::new().fg(Color::RED).bold();

            let merged = base.merge(&patch);

            assert_eq!(merged.fg, Some(Color::RED)); // From patch
            assert_eq!(merged.bg, Some(Color::BLACK)); // From base
            assert!(merged.is_bold()); // From patch
        }

        #[test]
        fn test_style_apply() {
            let mut style = Style::new().fg(Color::WHITE);
            let patch = Style::new().bg(Color::BLACK).italic();

            style.apply(&patch);

            assert_eq!(style.fg, Some(Color::WHITE)); // Unchanged
            assert_eq!(style.bg, Some(Color::BLACK)); // Applied
            assert!(style.is_italic()); // Applied
        }

        #[test]
        fn test_style_diff() {
            let base = Style::new().fg(Color::WHITE).bold();
            let target = Style::new().fg(Color::RED).bold().underline();

            let diff = base.diff(&target);

            assert_eq!(diff.fg, Some(Color::RED));
            assert!(diff.bg.is_none());
            assert!(!diff.is_bold()); // Already in base
            assert!(diff.is_underline()); // New in target
        }

        #[test]
        fn test_style_is_set() {
            assert!(!Style::new().is_set());
            assert!(Style::new().fg(Color::RED).is_set());
            assert!(Style::new().bold().is_set());
        }

        #[test]
        fn test_clear_methods() {
            let style = Style::new()
                .fg(Color::RED)
                .bg(Color::BLACK)
                .bold()
                .clear_fg()
                .clear_bg()
                .clear_attributes();

            assert!(style.fg.is_none());
            assert!(style.bg.is_none());
            assert!(style.attributes.is_empty());
        }
    }

    mod ansi_tests {
        use super::*;

        #[test]
        fn test_to_ansi() {
            let style = Style::new().fg(Color::from_rgb_u8(255, 0, 0)).bold();

            let ansi = style.to_ansi();

            assert!(ansi.contains("38;2;255;0;0")); // FG color
            assert!(ansi.contains("\x1b[1m")); // Bold
        }

        #[test]
        fn test_write_ansi() {
            let style = Style::new().fg(Color::from_rgb_u8(255, 0, 0)).bold();

            let mut buf = Vec::new();
            style.write_ansi(&mut buf);

            let result = String::from_utf8(buf).unwrap();
            assert!(result.contains("38;2;255;0;0"));
            assert!(result.contains("\x1b[1m"));
        }

        #[test]
        fn test_transition_simple() {
            let from = Style::new().fg(Color::RED);
            let to = Style::new().fg(Color::BLUE);

            let mut buf = Vec::new();
            from.transition_to(&to, &mut buf);

            let result = String::from_utf8(buf).unwrap();
            assert!(result.contains("38;2;")); // FG color change
        }

        #[test]
        fn test_transition_with_reset() {
            let from = Style::new().bold();
            let to = Style::new(); // No bold

            let mut buf = Vec::new();
            from.transition_to(&to, &mut buf);

            let result = String::from_utf8(buf).unwrap();
            assert!(result.contains("\x1b[0m")); // Reset needed
        }
    }

    mod display_tests {
        use super::*;

        #[test]
        fn test_style_display_empty() {
            let style = Style::new();
            assert!(style.to_string().contains("none"));
        }

        #[test]
        fn test_style_display_full() {
            let style = Style::new().fg(Color::RED).bg(Color::BLACK).bold();

            let display = style.to_string();
            assert!(display.contains("fg:"));
            assert!(display.contains("bg:"));
            assert!(display.contains("attrs:"));
        }
    }

    mod preset_tests {
        use super::*;

        #[test]
        fn test_preset_bold() {
            assert!(Style::BOLD.is_bold());
            assert!(Style::BOLD.fg.is_none());
            assert!(Style::BOLD.bg.is_none());
        }

        #[test]
        fn test_preset_italic() {
            assert!(Style::ITALIC.is_italic());
        }

        #[test]
        fn test_preset_underline() {
            assert!(Style::UNDERLINE.is_underline());
        }
    }
}
