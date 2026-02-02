//! ASCII box-drawing border characters for table rendering.

/// Top-left corner: ┌
pub const TOP_LEFT: char = '\u{250C}';
/// Top-right corner: ┐
pub const TOP_RIGHT: char = '\u{2510}';
/// Bottom-left corner: └
pub const BOTTOM_LEFT: char = '\u{2514}';
/// Bottom-right corner: ┘
pub const BOTTOM_RIGHT: char = '\u{2518}';
/// Horizontal line: ─
pub const HORIZONTAL: char = '\u{2500}';
/// Vertical line: │
pub const VERTICAL: char = '\u{2502}';
/// Cross intersection: ┼
pub const CROSS: char = '\u{253C}';
/// T-down (top tee): ┬
pub const T_DOWN: char = '\u{252C}';
/// T-up (bottom tee): ┴
pub const T_UP: char = '\u{2534}';
/// T-right (left tee): ├
pub const T_RIGHT: char = '\u{251C}';
/// T-left (right tee): ┤
pub const T_LEFT: char = '\u{2524}';
