//! Terminal cell representation.
//!
//! A [`Cell`] represents a single character position in a terminal buffer,
//! containing a character, colors, attributes, and display width information.

use cortex_tui_core::{Color, Style, TextAttributes};

/// A single cell in the terminal buffer.
///
/// Each cell represents one character position, containing:
/// - A base character (grapheme base codepoint)
/// - Foreground and background colors
/// - Text attributes (bold, italic, etc.)
/// - Display width (for handling wide characters like CJK and emoji)
///
/// # Wide Characters
///
/// Characters that occupy more than one cell (like CJK characters or emoji)
/// are represented with the main character in the first cell (width > 1)
/// and continuation cells (width = 0) in subsequent positions.
///
/// # Examples
///
/// ```
/// use cortex_tui_buffer::Cell;
/// use cortex_tui_core::{Color, TextAttributes};
///
/// let cell = Cell::new('A')
///     .with_fg(Color::WHITE)
///     .with_bg(Color::BLUE)
///     .with_attributes(TextAttributes::BOLD);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    /// The character displayed in this cell.
    ///
    /// For extended grapheme clusters (multi-codepoint sequences),
    /// this holds the base character. A separate grapheme pool
    /// would be needed for full grapheme support.
    pub character: char,

    /// Foreground (text) color.
    pub fg: Color,

    /// Background color.
    pub bg: Color,

    /// Text rendering attributes (bold, italic, etc.).
    pub attributes: TextAttributes,

    /// Display width of this cell.
    ///
    /// - `1`: Normal single-width character
    /// - `2`: Wide character (CJK, emoji) - this is the first cell
    /// - `0`: Continuation cell for wide characters
    pub width: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            fg: Color::WHITE,
            bg: Color::TRANSPARENT,
            attributes: TextAttributes::empty(),
            width: 1,
        }
    }
}

impl Cell {
    /// Creates a new cell with the specified character.
    #[inline]
    pub fn new(character: char) -> Self {
        Self {
            character,
            fg: Color::WHITE,
            bg: Color::TRANSPARENT,
            attributes: TextAttributes::empty(),
            width: 1,
        }
    }

    /// Creates an empty (space) cell.
    #[inline]
    pub fn empty() -> Self {
        Self {
            character: ' ',
            fg: Color::WHITE,
            bg: Color::TRANSPARENT,
            attributes: TextAttributes::empty(),
            width: 1,
        }
    }

    /// Creates a continuation cell for wide characters.
    ///
    /// Continuation cells have width 0 and are placed after the first cell
    /// of a wide character to indicate that the position is occupied.
    #[inline]
    pub fn continuation() -> Self {
        Self {
            character: ' ',
            fg: Color::TRANSPARENT,
            bg: Color::TRANSPARENT,
            attributes: TextAttributes::empty(),
            width: 0,
        }
    }

    /// Creates a cell from a style (using space as character).
    #[inline]
    pub fn from_style(style: Style) -> Self {
        Self {
            character: ' ',
            fg: style.fg.unwrap_or(Color::WHITE),
            bg: style.bg.unwrap_or(Color::TRANSPARENT),
            attributes: style.attributes,
            width: 1,
        }
    }

    /// Creates a cell with character and style.
    #[inline]
    pub fn with_style(character: char, style: Style) -> Self {
        Self {
            character,
            fg: style.fg.unwrap_or(Color::WHITE),
            bg: style.bg.unwrap_or(Color::TRANSPARENT),
            attributes: style.attributes,
            width: 1,
        }
    }

    /// Sets the character.
    #[inline]
    pub const fn with_char(self, character: char) -> Self {
        Self { character, ..self }
    }

    /// Sets the foreground color.
    #[inline]
    pub const fn with_fg(self, fg: Color) -> Self {
        Self { fg, ..self }
    }

    /// Sets the background color.
    #[inline]
    pub const fn with_bg(self, bg: Color) -> Self {
        Self { bg, ..self }
    }

    /// Sets the text attributes.
    #[inline]
    pub const fn with_attributes(self, attributes: TextAttributes) -> Self {
        Self { attributes, ..self }
    }

    /// Sets the display width.
    #[inline]
    pub const fn with_width(self, width: u8) -> Self {
        Self { width, ..self }
    }

    /// Returns the style of this cell.
    #[inline]
    pub fn style(&self) -> Style {
        Style {
            fg: Some(self.fg),
            bg: Some(self.bg),
            attributes: self.attributes,
        }
    }

    /// Applies a style to this cell.
    #[inline]
    pub fn apply_style(&mut self, style: Style) {
        if let Some(fg) = style.fg {
            self.fg = fg;
        }
        if let Some(bg) = style.bg {
            self.bg = bg;
        }
        self.attributes = style.attributes;
    }

    /// Returns true if this is a continuation cell (part of a wide character).
    #[inline]
    pub const fn is_continuation(&self) -> bool {
        self.width == 0
    }

    /// Returns true if this is a wide character (width > 1).
    #[inline]
    pub const fn is_wide(&self) -> bool {
        self.width > 1
    }

    /// Returns true if this cell has the default empty state.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.character == ' '
            && self.fg == Color::WHITE
            && self.bg.is_transparent()
            && self.attributes.is_empty()
            && self.width == 1
    }

    /// Resets this cell to the default empty state.
    #[inline]
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Resets this cell with a specific background color.
    #[inline]
    pub fn reset_with_bg(&mut self, bg: Color) {
        *self = Self {
            character: ' ',
            fg: Color::WHITE,
            bg,
            attributes: TextAttributes::empty(),
            width: 1,
        };
    }

    /// Compares cells for equality with epsilon tolerance for colors.
    ///
    /// This is useful for buffer diffing where small color differences
    /// can be ignored to reduce unnecessary updates.
    #[inline]
    pub fn approx_eq(&self, other: &Self, color_epsilon: f32) -> bool {
        self.character == other.character
            && self.attributes == other.attributes
            && self.width == other.width
            && self.fg.approx_eq(&other.fg, color_epsilon)
            && self.bg.approx_eq(&other.bg, color_epsilon)
    }

    /// Blends this cell's colors over a base cell's colors.
    ///
    /// The character and attributes of this cell are preserved,
    /// but colors are alpha-blended with the base.
    #[inline]
    pub fn blend_over(&self, base: &Self) -> Self {
        Self {
            character: self.character,
            fg: self.fg.blend_over(base.fg),
            bg: self.bg.blend_over(base.bg),
            attributes: self.attributes,
            width: self.width,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_default() {
        let cell = Cell::default();
        assert_eq!(cell.character, ' ');
        assert_eq!(cell.fg, Color::WHITE);
        assert!(cell.bg.is_transparent());
        assert_eq!(cell.attributes, TextAttributes::empty());
        assert_eq!(cell.width, 1);
    }

    #[test]
    fn test_cell_builder() {
        let cell = Cell::new('X')
            .with_fg(Color::RED)
            .with_bg(Color::BLUE)
            .with_attributes(TextAttributes::BOLD)
            .with_width(1);

        assert_eq!(cell.character, 'X');
        assert_eq!(cell.fg, Color::RED);
        assert_eq!(cell.bg, Color::BLUE);
        assert!(cell.attributes.contains(TextAttributes::BOLD));
    }

    #[test]
    fn test_cell_continuation() {
        let cont = Cell::continuation();
        assert!(cont.is_continuation());
        assert_eq!(cont.width, 0);
    }

    #[test]
    fn test_cell_wide() {
        let wide = Cell::new('漢').with_width(2);
        assert!(wide.is_wide());
        assert_eq!(wide.width, 2);
    }

    #[test]
    fn test_cell_approx_eq() {
        let a = Cell::new('A').with_fg(Color::new(0.5, 0.5, 0.5, 1.0));
        let b = Cell::new('A').with_fg(Color::new(0.500001, 0.5, 0.5, 1.0));

        assert!(a.approx_eq(&b, 0.001));
        assert!(!a.approx_eq(&Cell::new('B'), 0.001));
    }

    #[test]
    fn test_cell_style() {
        let style = Style::new().fg(Color::GREEN).bg(Color::BLACK).bold();

        let cell = Cell::with_style('Y', style);
        assert_eq!(cell.fg, Color::GREEN);
        assert_eq!(cell.bg, Color::BLACK);
        assert!(cell.attributes.contains(TextAttributes::BOLD));

        let extracted_style = cell.style();
        assert_eq!(extracted_style.fg, Some(Color::GREEN));
    }

    #[test]
    fn test_cell_reset() {
        let mut cell = Cell::new('X')
            .with_fg(Color::RED)
            .with_bg(Color::BLUE)
            .with_attributes(TextAttributes::BOLD);

        cell.reset();
        assert_eq!(cell, Cell::default());
    }
}
