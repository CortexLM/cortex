use crate::buffer::{Buffer, BufferExt};
use crate::types::{Color, Rect, Style};

/// Border style variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    /// No border.
    #[default]
    None,
    /// Single-line border using box-drawing characters.
    /// ```text
    /// ┌───┐
    /// │   │
    /// └───┘
    /// ```
    Single,
    /// Double-line border using box-drawing characters.
    /// ```text
    /// ╔═══╗
    /// ║   ║
    /// ╚═══╝
    /// ```
    Double,
    /// Rounded border with curved corners.
    /// ```text
    /// ╭───╮
    /// │   │
    /// ╰───╯
    /// ```
    Rounded,
    /// Heavy/bold border using thick box-drawing characters.
    /// ```text
    /// ┏━━━┓
    /// ┃   ┃
    /// ┗━━━┛
    /// ```
    Heavy,
    /// ASCII border using basic characters.
    /// ```text
    /// +---+
    /// |   |
    /// +---+
    /// ```
    Ascii,
    /// Custom border with user-defined characters.
    Custom(BorderChars),
}

impl BorderStyle {
    /// Returns the border characters for this style.
    pub fn chars(&self) -> BorderChars {
        match self {
            Self::None => BorderChars::EMPTY,
            Self::Single => BorderChars::SINGLE,
            Self::Double => BorderChars::DOUBLE,
            Self::Rounded => BorderChars::ROUNDED,
            Self::Heavy => BorderChars::HEAVY,
            Self::Ascii => BorderChars::ASCII,
            Self::Custom(chars) => *chars,
        }
    }

    /// Returns true if this style has no border.
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// Characters used to draw a border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorderChars {
    /// Top-left corner character.
    pub top_left: char,
    /// Top-right corner character.
    pub top_right: char,
    /// Bottom-left corner character.
    pub bottom_left: char,
    /// Bottom-right corner character.
    pub bottom_right: char,
    /// Horizontal edge character.
    pub horizontal: char,
    /// Vertical edge character.
    pub vertical: char,
}

impl BorderChars {
    /// Empty (space) border characters.
    pub const EMPTY: Self = Self {
        top_left: ' ',
        top_right: ' ',
        bottom_left: ' ',
        bottom_right: ' ',
        horizontal: ' ',
        vertical: ' ',
    };

    /// Single-line box-drawing characters.
    pub const SINGLE: Self = Self {
        top_left: '┌',
        top_right: '┐',
        bottom_left: '└',
        bottom_right: '┘',
        horizontal: '─',
        vertical: '│',
    };

    /// Double-line box-drawing characters.
    pub const DOUBLE: Self = Self {
        top_left: '╔',
        top_right: '╗',
        bottom_left: '╚',
        bottom_right: '╝',
        horizontal: '═',
        vertical: '║',
    };

    /// Rounded box-drawing characters.
    pub const ROUNDED: Self = Self {
        top_left: '╭',
        top_right: '╮',
        bottom_left: '╰',
        bottom_right: '╯',
        horizontal: '─',
        vertical: '│',
    };

    /// Heavy (bold) box-drawing characters.
    pub const HEAVY: Self = Self {
        top_left: '┏',
        top_right: '┓',
        bottom_left: '┗',
        bottom_right: '┛',
        horizontal: '━',
        vertical: '┃',
    };

    /// ASCII border characters.
    pub const ASCII: Self = Self {
        top_left: '+',
        top_right: '+',
        bottom_left: '+',
        bottom_right: '+',
        horizontal: '-',
        vertical: '|',
    };

    /// Creates custom border characters.
    pub const fn new(
        top_left: char,
        top_right: char,
        bottom_left: char,
        bottom_right: char,
        horizontal: char,
        vertical: char,
    ) -> Self {
        Self {
            top_left,
            top_right,
            bottom_left,
            bottom_right,
            horizontal,
            vertical,
        }
    }
}

impl Default for BorderChars {
    fn default() -> Self {
        Self::SINGLE
    }
}

/// Which sides of a border to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorderSides {
    /// Draw top edge.
    pub top: bool,
    /// Draw right edge.
    pub right: bool,
    /// Draw bottom edge.
    pub bottom: bool,
    /// Draw left edge.
    pub left: bool,
}

impl BorderSides {
    /// All sides enabled.
    pub const ALL: Self = Self {
        top: true,
        right: true,
        bottom: true,
        left: true,
    };

    /// No sides enabled.
    pub const NONE: Self = Self {
        top: false,
        right: false,
        bottom: false,
        left: false,
    };

    /// Only top side.
    pub const TOP: Self = Self {
        top: true,
        right: false,
        bottom: false,
        left: false,
    };

    /// Only bottom side.
    pub const BOTTOM: Self = Self {
        top: false,
        right: false,
        bottom: true,
        left: false,
    };

    /// Only left side.
    pub const LEFT: Self = Self {
        top: false,
        right: false,
        bottom: false,
        left: true,
    };

    /// Only right side.
    pub const RIGHT: Self = Self {
        top: false,
        right: true,
        bottom: false,
        left: false,
    };

    /// Horizontal sides (top and bottom).
    pub const HORIZONTAL: Self = Self {
        top: true,
        right: false,
        bottom: true,
        left: false,
    };

    /// Vertical sides (left and right).
    pub const VERTICAL: Self = Self {
        top: false,
        right: true,
        bottom: false,
        left: true,
    };

    /// Creates a new BorderSides configuration.
    pub const fn new(top: bool, right: bool, bottom: bool, left: bool) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Returns true if any side is enabled.
    pub const fn any(&self) -> bool {
        self.top || self.right || self.bottom || self.left
    }

    /// Returns true if all sides are enabled.
    pub const fn all(&self) -> bool {
        self.top && self.right && self.bottom && self.left
    }

    /// Returns true if no sides are enabled.
    pub const fn none(&self) -> bool {
        !self.any()
    }
}

impl Default for BorderSides {
    fn default() -> Self {
        Self::ALL
    }
}

/// Parameters for drawing a border.
pub struct DrawBorderParams {
    /// The rectangle to draw the border around.
    pub rect: Rect,
    /// The border style to use.
    pub style: BorderStyle,
    /// Which sides to draw.
    pub sides: BorderSides,
    /// The color of the border.
    pub color: Option<Color>,
    /// Optional title to display in the top border.
    pub title: Option<String>,
    /// Title alignment.
    pub title_alignment: TitleAlignment,
    /// Title style (if different from border).
    pub title_style: Option<Style>,
}

impl DrawBorderParams {
    /// Creates new parameters with defaults.
    pub fn new(rect: Rect) -> Self {
        Self {
            rect,
            style: BorderStyle::Single,
            sides: BorderSides::ALL,
            color: None,
            title: None,
            title_alignment: TitleAlignment::Left,
            title_style: None,
        }
    }

    /// Sets the border style.
    pub fn style(mut self, style: BorderStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets which sides to draw.
    pub fn sides(mut self, sides: BorderSides) -> Self {
        self.sides = sides;
        self
    }

    /// Sets the border color.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Sets the title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the title alignment.
    pub fn title_alignment(mut self, alignment: TitleAlignment) -> Self {
        self.title_alignment = alignment;
        self
    }
}

/// Title alignment within the border.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleAlignment {
    /// Title aligned to the left.
    #[default]
    Left,
    /// Title centered.
    Center,
    /// Title aligned to the right.
    Right,
}

/// Draws a border on the buffer with the given parameters.
pub fn draw_border(buffer: &mut Buffer, params: &DrawBorderParams) {
    let rect = params.rect;
    if rect.width < 2 || rect.height < 2 {
        return;
    }

    if params.style.is_none() || params.sides.none() {
        return;
    }

    let chars = params.style.chars();
    let mut style = Style::new();
    if let Some(color) = params.color {
        style = style.fg(color);
    }

    let x1 = rect.x;
    let y1 = rect.y;
    let x2 = rect.right().saturating_sub(1);
    let y2 = rect.bottom().saturating_sub(1);

    // Draw corners (only if both adjacent sides are enabled)
    if params.sides.top && params.sides.left {
        buffer.set_char(x1, y1, chars.top_left, style);
    }
    if params.sides.top && params.sides.right {
        buffer.set_char(x2, y1, chars.top_right, style);
    }
    if params.sides.bottom && params.sides.left {
        buffer.set_char(x1, y2, chars.bottom_left, style);
    }
    if params.sides.bottom && params.sides.right {
        buffer.set_char(x2, y2, chars.bottom_right, style);
    }

    // Draw horizontal edges
    if params.sides.top && rect.width > 2 {
        for x in (x1 + 1)..x2 {
            buffer.set_char(x, y1, chars.horizontal, style);
        }
    }
    if params.sides.bottom && rect.width > 2 {
        for x in (x1 + 1)..x2 {
            buffer.set_char(x, y2, chars.horizontal, style);
        }
    }

    // Draw vertical edges
    if params.sides.left && rect.height > 2 {
        for y in (y1 + 1)..y2 {
            buffer.set_char(x1, y, chars.vertical, style);
        }
    }
    if params.sides.right && rect.height > 2 {
        for y in (y1 + 1)..y2 {
            buffer.set_char(x2, y, chars.vertical, style);
        }
    }

    // Draw title if present
    if let Some(title) = &params.title {
        if params.sides.top && rect.width > 4 {
            draw_title(
                buffer,
                rect,
                title,
                params.title_alignment,
                params.title_style.unwrap_or(style),
            );
        }
    }
}

/// Draws a title in the top border.
fn draw_title(
    buffer: &mut Buffer,
    rect: Rect,
    title: &str,
    alignment: TitleAlignment,
    style: Style,
) {
    let available_width = rect.width.saturating_sub(4) as usize; // Leave space for corners and padding
    if available_width == 0 {
        return;
    }

    // Truncate title if necessary
    let display_title = if title.len() > available_width {
        let truncated = &title[..available_width.saturating_sub(1)];
        format!("{}…", truncated)
    } else {
        title.to_string()
    };

    let title_len = display_title.chars().count();
    let x = match alignment {
        TitleAlignment::Left => rect.x + 2,
        TitleAlignment::Center => rect.x + (rect.width.saturating_sub(title_len as u16) as i32) / 2,
        TitleAlignment::Right => rect.right().saturating_sub(title_len as i32 + 2),
    };

    buffer.set_string(x, rect.y, &display_title, style);
}

/// Calculates the inner rectangle after accounting for borders.
pub fn inner_rect(rect: Rect, sides: BorderSides) -> Rect {
    let left = if sides.left { 1i32 } else { 0 };
    let right = if sides.right { 1i32 } else { 0 };
    let top = if sides.top { 1i32 } else { 0 };
    let bottom = if sides.bottom { 1i32 } else { 0 };

    Rect {
        x: rect.x.saturating_add(left),
        y: rect.y.saturating_add(top),
        width: rect.width.saturating_sub((left + right) as u16),
        height: rect.height.saturating_sub((top + bottom) as u16),
    }
}

/// Helper struct for building borders fluently.
pub struct BorderBuilder {
    params: DrawBorderParams,
}

impl BorderBuilder {
    /// Creates a new border builder for the given rectangle.
    pub fn new(rect: Rect) -> Self {
        Self {
            params: DrawBorderParams::new(rect),
        }
    }

    /// Sets the border style.
    pub fn style(mut self, style: BorderStyle) -> Self {
        self.params.style = style;
        self
    }

    /// Sets single-line border style.
    pub fn single(self) -> Self {
        self.style(BorderStyle::Single)
    }

    /// Sets double-line border style.
    pub fn double(self) -> Self {
        self.style(BorderStyle::Double)
    }

    /// Sets rounded border style.
    pub fn rounded(self) -> Self {
        self.style(BorderStyle::Rounded)
    }

    /// Sets heavy border style.
    pub fn heavy(self) -> Self {
        self.style(BorderStyle::Heavy)
    }

    /// Sets ASCII border style.
    pub fn ascii(self) -> Self {
        self.style(BorderStyle::Ascii)
    }

    /// Sets custom border characters.
    pub fn custom(self, chars: BorderChars) -> Self {
        self.style(BorderStyle::Custom(chars))
    }

    /// Sets which sides to draw.
    pub fn sides(mut self, sides: BorderSides) -> Self {
        self.params.sides = sides;
        self
    }

    /// Draws all sides.
    pub fn all_sides(self) -> Self {
        self.sides(BorderSides::ALL)
    }

    /// Sets the border color.
    pub fn color(mut self, color: Color) -> Self {
        self.params.color = Some(color);
        self
    }

    /// Sets the title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.params.title = Some(title.into());
        self
    }

    /// Sets the title alignment.
    pub fn title_alignment(mut self, alignment: TitleAlignment) -> Self {
        self.params.title_alignment = alignment;
        self
    }

    /// Sets the title style.
    pub fn title_style(mut self, style: Style) -> Self {
        self.params.title_style = Some(style);
        self
    }

    /// Draws the border to the buffer.
    pub fn draw(self, buffer: &mut Buffer) {
        draw_border(buffer, &self.params);
    }

    /// Returns the parameters without drawing.
    pub fn build(self) -> DrawBorderParams {
        self.params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_border_style_chars() {
        let chars = BorderStyle::Single.chars();
        assert_eq!(chars.top_left, '┌');
        assert_eq!(chars.horizontal, '─');

        let chars = BorderStyle::Double.chars();
        assert_eq!(chars.top_left, '╔');
        assert_eq!(chars.horizontal, '═');
    }

    #[test]
    fn test_border_sides() {
        assert!(BorderSides::ALL.all());
        assert!(BorderSides::NONE.none());
        assert!(BorderSides::TOP.any());
        assert!(!BorderSides::TOP.all());
    }

    #[test]
    fn test_inner_rect() {
        let outer = Rect::new(0, 0, 10, 10);
        let inner = inner_rect(outer, BorderSides::ALL);
        assert_eq!(inner, Rect::new(1, 1, 8, 8));

        let inner = inner_rect(outer, BorderSides::TOP);
        assert_eq!(inner, Rect::new(0, 1, 10, 9));
    }
}
