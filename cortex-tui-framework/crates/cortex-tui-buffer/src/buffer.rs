//! Terminal buffer for cell storage and manipulation.
//!
//! The [`Buffer`] struct provides a 2D grid of cells with support for:
//! - Cell access and modification
//! - Scissor/clip rectangle stacking
//! - Opacity stacking for transparency effects
//! - Text rendering with style support

use smallvec::SmallVec;

use cortex_tui_core::{Color, Rect, Style};

use crate::Cell;

/// A 2D buffer of terminal cells.
///
/// The buffer stores cells in row-major order and provides methods for
/// cell access, text rendering, and clipping operations.
///
/// # Coordinate System
///
/// - (0, 0) is the top-left corner
/// - X increases to the right (columns)
/// - Y increases downward (rows)
///
/// # Clipping (Scissor Rectangles)
///
/// The buffer supports a stack of scissor rectangles that restrict
/// rendering operations to a specific region. Nested scissor rectangles
/// are intersected with their parents.
///
/// # Examples
///
/// ```
/// use cortex_tui_buffer::Buffer;
/// use cortex_tui_core::{Color, Rect, Style};
///
/// let mut buffer = Buffer::new(80, 24);
///
/// // Draw some text
/// buffer.draw_str(10, 5, "Hello, World!", Style::new().fg(Color::GREEN));
///
/// // Use scissor to clip rendering
/// buffer.push_scissor(Rect::new(0, 0, 40, 12));
/// buffer.draw_str(35, 5, "Clipped!", Style::default());
/// buffer.pop_scissor();
/// ```
pub struct Buffer {
    /// Cell storage in row-major order.
    cells: Vec<Cell>,

    /// Buffer width in cells/columns.
    width: u16,

    /// Buffer height in cells/rows.
    height: u16,

    /// Stack of scissor rectangles for clipping.
    /// Using SmallVec as scissor stacks are typically shallow.
    scissor_stack: SmallVec<[Rect; 8]>,

    /// Stack of opacity multipliers.
    opacity_stack: SmallVec<[f32; 8]>,
}

impl Buffer {
    /// Creates a new buffer with the specified dimensions.
    ///
    /// All cells are initialized to the default empty state.
    pub fn new(width: u16, height: u16) -> Self {
        let size = width as usize * height as usize;
        Self {
            cells: vec![Cell::default(); size],
            width,
            height,
            scissor_stack: SmallVec::new(),
            opacity_stack: SmallVec::new(),
        }
    }

    /// Returns the buffer width.
    #[inline]
    pub const fn width(&self) -> u16 {
        self.width
    }

    /// Returns the buffer height.
    #[inline]
    pub const fn height(&self) -> u16 {
        self.height
    }

    /// Returns the buffer dimensions as (width, height).
    #[inline]
    pub const fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Returns the total number of cells.
    #[inline]
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Returns true if the buffer has no cells.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Returns a rectangle covering the entire buffer.
    #[inline]
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Converts (x, y) coordinates to a linear index.
    ///
    /// Returns `None` if coordinates are out of bounds.
    #[inline]
    fn index(&self, x: u16, y: u16) -> Option<usize> {
        if x < self.width && y < self.height {
            Some(y as usize * self.width as usize + x as usize)
        } else {
            None
        }
    }

    /// Gets a reference to the cell at (x, y).
    ///
    /// Returns `None` if coordinates are out of bounds.
    #[inline]
    pub fn get(&self, x: u16, y: u16) -> Option<&Cell> {
        self.index(x, y).map(|i| &self.cells[i])
    }

    /// Gets a mutable reference to the cell at (x, y).
    ///
    /// Returns `None` if coordinates are out of bounds.
    #[inline]
    pub fn get_mut(&mut self, x: u16, y: u16) -> Option<&mut Cell> {
        self.index(x, y).map(|i| &mut self.cells[i])
    }

    /// Gets a reference to the cell at (x, y) without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller must ensure `x < width` and `y < height`.
    #[inline]
    pub unsafe fn get_unchecked(&self, x: u16, y: u16) -> &Cell {
        let index = y as usize * self.width as usize + x as usize;
        self.cells.get_unchecked(index)
    }

    /// Gets a mutable reference to the cell at (x, y) without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller must ensure `x < width` and `y < height`.
    #[inline]
    pub unsafe fn get_unchecked_mut(&mut self, x: u16, y: u16) -> &mut Cell {
        let index = y as usize * self.width as usize + x as usize;
        self.cells.get_unchecked_mut(index)
    }

    /// Sets the cell at (x, y), respecting scissor and opacity stacks.
    ///
    /// Returns `true` if the cell was set, `false` if clipped or out of bounds.
    pub fn set(&mut self, x: u16, y: u16, cell: Cell) -> bool {
        // Check scissor first
        if !self.is_point_visible(x as i32, y as i32) {
            return false;
        }

        if let Some(idx) = self.index(x, y) {
            let mut cell = cell;

            // Apply opacity stack
            if let Some(&opacity) = self.opacity_stack.last() {
                cell.fg = cell.fg.multiply_alpha(opacity);
                cell.bg = cell.bg.multiply_alpha(opacity);
            }

            self.cells[idx] = cell;
            true
        } else {
            false
        }
    }

    /// Sets the cell at (x, y) without any clipping checks.
    ///
    /// This bypasses scissor and opacity stacks for performance.
    pub fn set_raw(&mut self, x: u16, y: u16, cell: Cell) -> bool {
        if let Some(idx) = self.index(x, y) {
            self.cells[idx] = cell;
            true
        } else {
            false
        }
    }

    /// Returns a slice of the underlying cell storage.
    #[inline]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Returns a mutable slice of the underlying cell storage.
    #[inline]
    pub fn cells_mut(&mut self) -> &mut [Cell] {
        &mut self.cells
    }

    /// Returns a slice of cells for a specific row.
    ///
    /// Returns `None` if the row is out of bounds.
    pub fn row(&self, y: u16) -> Option<&[Cell]> {
        if y < self.height {
            let start = y as usize * self.width as usize;
            let end = start + self.width as usize;
            Some(&self.cells[start..end])
        } else {
            None
        }
    }

    /// Returns a mutable slice of cells for a specific row.
    ///
    /// Returns `None` if the row is out of bounds.
    pub fn row_mut(&mut self, y: u16) -> Option<&mut [Cell]> {
        if y < self.height {
            let start = y as usize * self.width as usize;
            let end = start + self.width as usize;
            Some(&mut self.cells[start..end])
        } else {
            None
        }
    }

    // ========================================================================
    // Scissor (Clipping) Operations
    // ========================================================================

    /// Pushes a scissor rectangle onto the stack.
    ///
    /// The new scissor is intersected with the current scissor (if any),
    /// so nested scissors can only make the clipping region smaller.
    pub fn push_scissor(&mut self, rect: Rect) {
        let effective_rect = if let Some(current) = self.scissor_stack.last() {
            // Intersect with current scissor
            rect.intersection(*current).unwrap_or(Rect::ZERO)
        } else {
            // Clamp to buffer bounds
            rect.clamp_to(self.bounds())
        };
        self.scissor_stack.push(effective_rect);
    }

    /// Pops the top scissor rectangle from the stack.
    ///
    /// Returns the popped rectangle, or `None` if the stack was empty.
    pub fn pop_scissor(&mut self) -> Option<Rect> {
        self.scissor_stack.pop()
    }

    /// Returns the current active scissor rectangle.
    ///
    /// Returns `None` if no scissor is active (entire buffer is visible).
    #[inline]
    pub fn current_scissor(&self) -> Option<Rect> {
        self.scissor_stack.last().copied()
    }

    /// Clears all scissor rectangles from the stack.
    pub fn clear_scissors(&mut self) {
        self.scissor_stack.clear();
    }

    /// Checks if a point is visible under the current scissor.
    #[inline]
    pub fn is_point_visible(&self, x: i32, y: i32) -> bool {
        if let Some(scissor) = self.scissor_stack.last() {
            scissor.contains_xy(x, y)
        } else {
            x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32
        }
    }

    // ========================================================================
    // Opacity Operations
    // ========================================================================

    /// Pushes an opacity multiplier onto the stack.
    ///
    /// The new opacity is multiplied with the current opacity,
    /// so nested opacity pushes compound the effect.
    pub fn push_opacity(&mut self, opacity: f32) {
        let current = self.opacity_stack.last().copied().unwrap_or(1.0);
        self.opacity_stack.push((current * opacity).clamp(0.0, 1.0));
    }

    /// Pops the top opacity from the stack.
    ///
    /// Returns the popped opacity, or `None` if the stack was empty.
    pub fn pop_opacity(&mut self) -> Option<f32> {
        self.opacity_stack.pop()
    }

    /// Returns the current active opacity multiplier.
    #[inline]
    pub fn current_opacity(&self) -> f32 {
        self.opacity_stack.last().copied().unwrap_or(1.0)
    }

    /// Clears all opacity values from the stack.
    pub fn clear_opacity(&mut self) {
        self.opacity_stack.clear();
    }

    // ========================================================================
    // Clear and Fill Operations
    // ========================================================================

    /// Clears the buffer, resetting all cells to the default state.
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = Cell::default();
        }
    }

    /// Clears the buffer with a specific background color.
    pub fn clear_with_bg(&mut self, bg: Color) {
        for cell in &mut self.cells {
            cell.reset_with_bg(bg);
        }
    }

    /// Fills a rectangular region with a cell.
    ///
    /// Respects the current scissor rectangle.
    pub fn fill_rect(&mut self, rect: Rect, cell: Cell) {
        // Calculate effective bounds
        let effective_rect = if let Some(scissor) = self.scissor_stack.last() {
            match rect.intersection(*scissor) {
                Some(r) => r,
                None => return, // Fully clipped
            }
        } else {
            rect.clamp_to(self.bounds())
        };

        if effective_rect.is_empty() {
            return;
        }

        let mut cell = cell;

        // Apply opacity
        if let Some(&opacity) = self.opacity_stack.last() {
            cell.fg = cell.fg.multiply_alpha(opacity);
            cell.bg = cell.bg.multiply_alpha(opacity);
        }

        let x_start = effective_rect.x as u16;
        let x_end = effective_rect.right() as u16;
        let y_start = effective_rect.y as u16;
        let y_end = effective_rect.bottom() as u16;

        for y in y_start..y_end {
            if let Some(row) = self.row_mut(y) {
                for x in x_start..x_end {
                    if (x as usize) < row.len() {
                        row[x as usize] = cell;
                    }
                }
            }
        }
    }

    /// Fills a rectangular region with a specific character and style.
    pub fn fill(&mut self, rect: Rect, character: char, style: Style) {
        self.fill_rect(rect, Cell::with_style(character, style));
    }

    /// Fills the entire buffer with a cell.
    pub fn fill_all(&mut self, cell: Cell) {
        self.fill_rect(self.bounds(), cell);
    }

    // ========================================================================
    // Resize Operations
    // ========================================================================

    /// Resizes the buffer to new dimensions.
    ///
    /// Existing content is preserved where possible. New cells are
    /// initialized to the default state.
    pub fn resize(&mut self, new_width: u16, new_height: u16) {
        if new_width == self.width && new_height == self.height {
            return;
        }

        let new_size = new_width as usize * new_height as usize;
        let mut new_cells = vec![Cell::default(); new_size];

        // Copy existing content
        let copy_width = self.width.min(new_width) as usize;
        let copy_height = self.height.min(new_height) as usize;

        for y in 0..copy_height {
            let src_start = y * self.width as usize;
            let dst_start = y * new_width as usize;

            new_cells[dst_start..dst_start + copy_width]
                .copy_from_slice(&self.cells[src_start..src_start + copy_width]);
        }

        self.cells = new_cells;
        self.width = new_width;
        self.height = new_height;

        // Clear stacks that may have invalid coordinates
        self.scissor_stack.clear();
    }

    /// Resizes the buffer, clearing all existing content.
    pub fn resize_and_clear(&mut self, new_width: u16, new_height: u16) {
        let new_size = new_width as usize * new_height as usize;
        self.cells = vec![Cell::default(); new_size];
        self.width = new_width;
        self.height = new_height;
        self.scissor_stack.clear();
        self.opacity_stack.clear();
    }

    // ========================================================================
    // Text Drawing Operations
    // ========================================================================

    /// Draws a string at the specified position with style.
    ///
    /// Characters are placed left-to-right starting at (x, y).
    /// Wide characters (CJK, emoji) will occupy multiple cells.
    /// Respects the current scissor rectangle.
    ///
    /// Returns the number of cells written (including wide character cells).
    pub fn draw_str(&mut self, x: u16, y: u16, text: &str, style: Style) -> u16 {
        let mut cursor_x = x;
        let mut cells_written = 0u16;

        for ch in text.chars() {
            if cursor_x >= self.width {
                break;
            }

            // Calculate character width using unicode-width crate behavior:
            // - Control characters: 0
            // - Most ASCII: 1
            // - CJK: 2
            // - Emoji: varies (simplified to 2 for common emoji)
            let char_width = Self::char_width(ch);

            if char_width == 0 {
                // Skip zero-width characters (combining marks, etc.)
                continue;
            }

            // Check if character fits
            if cursor_x + char_width as u16 > self.width {
                break;
            }

            // Create the main cell
            let cell = Cell::with_style(ch, style).with_width(char_width);

            if self.set(cursor_x, y, cell) {
                cells_written += 1;

                // Add continuation cells for wide characters
                for i in 1..char_width {
                    if self.set(cursor_x + i as u16, y, Cell::continuation()) {
                        cells_written += 1;
                    }
                }
            }

            cursor_x += char_width as u16;
        }

        cells_written
    }

    /// Draws a string at the specified position with default style.
    pub fn draw_str_default(&mut self, x: u16, y: u16, text: &str) -> u16 {
        self.draw_str(x, y, text, Style::default())
    }

    /// Draws a single character at the specified position.
    ///
    /// Handles wide characters by adding continuation cells.
    /// Returns true if the character was drawn.
    pub fn draw_char(&mut self, x: u16, y: u16, ch: char, style: Style) -> bool {
        let char_width = Self::char_width(ch);

        if char_width == 0 {
            return false;
        }

        // Check if character fits
        if x + char_width as u16 > self.width {
            return false;
        }

        let cell = Cell::with_style(ch, style).with_width(char_width);

        if self.set(x, y, cell) {
            // Add continuation cells for wide characters
            for i in 1..char_width {
                self.set(x + i as u16, y, Cell::continuation());
            }
            true
        } else {
            false
        }
    }

    /// Calculates the display width of a character.
    ///
    /// This is a simplified implementation. For full Unicode support,
    /// use the `unicode-width` crate.
    fn char_width(ch: char) -> u8 {
        // Control characters
        if ch.is_control() {
            return 0;
        }

        // ASCII printable
        if ch.is_ascii() {
            return 1;
        }

        // CJK characters (simplified range check)
        let cp = ch as u32;

        // CJK Unified Ideographs and related blocks
        if (0x4E00..=0x9FFF).contains(&cp)       // CJK Unified Ideographs
            || (0x3400..=0x4DBF).contains(&cp)   // CJK Extension A
            || (0x20000..=0x2A6DF).contains(&cp) // CJK Extension B
            || (0xF900..=0xFAFF).contains(&cp)   // CJK Compatibility Ideographs
            || (0x2F800..=0x2FA1F).contains(&cp) // CJK Compatibility Supplement
            || (0x3000..=0x303F).contains(&cp)   // CJK Symbols and Punctuation
            || (0xFF00..=0xFFEF).contains(&cp)   // Halfwidth and Fullwidth Forms
            || (0xAC00..=0xD7AF).contains(&cp)
        // Hangul Syllables
        {
            return 2;
        }

        // Common emoji ranges (simplified)
        if (0x1F300..=0x1F9FF).contains(&cp) // Miscellaneous Symbols and Pictographs to Supplemental Symbols
            || (0x2600..=0x26FF).contains(&cp) // Miscellaneous Symbols
            || (0x2700..=0x27BF).contains(&cp)
        // Dingbats
        {
            return 2;
        }

        // Default to single width
        1
    }

    // ========================================================================
    // Copy Operations
    // ========================================================================

    /// Copies a rectangular region from another buffer.
    ///
    /// - `src`: Source buffer
    /// - `src_rect`: Rectangle in source buffer to copy
    /// - `dst_x`, `dst_y`: Destination position in this buffer
    ///
    /// Respects the current scissor rectangle.
    pub fn copy_from(&mut self, src: &Buffer, src_rect: Rect, dst_x: u16, dst_y: u16) {
        let src_bounds = src.bounds();
        let src_rect = match src_rect.intersection(src_bounds) {
            Some(r) => r,
            None => return,
        };

        for row in 0..src_rect.height {
            let src_y = (src_rect.y + row as i32) as u16;
            let dst_y = dst_y + row;

            if dst_y >= self.height {
                break;
            }

            for col in 0..src_rect.width {
                let src_x = (src_rect.x + col as i32) as u16;
                let dst_x = dst_x + col;

                if dst_x >= self.width {
                    break;
                }

                if let Some(cell) = src.get(src_x, src_y) {
                    self.set(dst_x, dst_y, *cell);
                }
            }
        }
    }

    /// Copies this entire buffer's content to another buffer.
    pub fn copy_to(&self, dst: &mut Buffer, dst_x: u16, dst_y: u16) {
        dst.copy_from(self, self.bounds(), dst_x, dst_y);
    }
}

impl Clone for Buffer {
    fn clone(&self) -> Self {
        Self {
            cells: self.cells.clone(),
            width: self.width,
            height: self.height,
            scissor_stack: self.scissor_stack.clone(),
            opacity_stack: self.opacity_stack.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_tui_core::TextAttributes;

    #[test]
    fn test_buffer_creation() {
        let buffer = Buffer::new(80, 24);
        assert_eq!(buffer.width(), 80);
        assert_eq!(buffer.height(), 24);
        assert_eq!(buffer.len(), 80 * 24);
    }

    #[test]
    fn test_buffer_get_set() {
        let mut buffer = Buffer::new(10, 10);

        let cell = Cell::new('A').with_fg(Color::RED);
        assert!(buffer.set(5, 5, cell));

        let retrieved = buffer.get(5, 5).unwrap();
        assert_eq!(retrieved.character, 'A');
        assert_eq!(retrieved.fg, Color::RED);
    }

    #[test]
    fn test_buffer_bounds_check() {
        let buffer = Buffer::new(10, 10);

        assert!(buffer.get(0, 0).is_some());
        assert!(buffer.get(9, 9).is_some());
        assert!(buffer.get(10, 0).is_none());
        assert!(buffer.get(0, 10).is_none());
    }

    #[test]
    fn test_buffer_scissor() {
        let mut buffer = Buffer::new(20, 20);

        // Push scissor
        buffer.push_scissor(Rect::new(5, 5, 10, 10));

        // Cell inside scissor should be set
        assert!(buffer.set(7, 7, Cell::new('X')));
        assert_eq!(buffer.get(7, 7).unwrap().character, 'X');

        // Cell outside scissor should not be set
        assert!(!buffer.set(0, 0, Cell::new('Y')));
        assert_eq!(buffer.get(0, 0).unwrap().character, ' ');

        // Pop scissor
        buffer.pop_scissor();

        // Now cell outside should be settable
        assert!(buffer.set(0, 0, Cell::new('Z')));
        assert_eq!(buffer.get(0, 0).unwrap().character, 'Z');
    }

    #[test]
    fn test_buffer_nested_scissor() {
        let mut buffer = Buffer::new(30, 30);

        buffer.push_scissor(Rect::new(5, 5, 20, 20));
        buffer.push_scissor(Rect::new(10, 10, 20, 20)); // Should intersect to (10, 10, 15, 15)

        // Inside both scissors
        assert!(buffer.set(12, 12, Cell::new('A')));

        // Outside inner scissor but inside outer
        assert!(!buffer.set(6, 6, Cell::new('B')));

        // Outside both
        assert!(!buffer.set(0, 0, Cell::new('C')));
    }

    #[test]
    fn test_buffer_draw_str() {
        let mut buffer = Buffer::new(80, 24);

        let style = Style::new().fg(Color::GREEN).bold();
        let cells = buffer.draw_str(10, 5, "Hello", style);

        assert_eq!(cells, 5);
        assert_eq!(buffer.get(10, 5).unwrap().character, 'H');
        assert_eq!(buffer.get(14, 5).unwrap().character, 'o');
        assert_eq!(buffer.get(10, 5).unwrap().fg, Color::GREEN);
        assert!(buffer
            .get(10, 5)
            .unwrap()
            .attributes
            .contains(TextAttributes::BOLD));
    }

    #[test]
    fn test_buffer_fill_rect() {
        let mut buffer = Buffer::new(20, 20);

        let cell = Cell::new('#').with_bg(Color::BLUE);
        buffer.fill_rect(Rect::new(5, 5, 5, 5), cell);

        assert_eq!(buffer.get(5, 5).unwrap().character, '#');
        assert_eq!(buffer.get(9, 9).unwrap().character, '#');
        assert_eq!(buffer.get(4, 5).unwrap().character, ' '); // Outside rect
    }

    #[test]
    fn test_buffer_resize() {
        let mut buffer = Buffer::new(10, 10);
        buffer.set(5, 5, Cell::new('X'));

        buffer.resize(20, 20);

        assert_eq!(buffer.width(), 20);
        assert_eq!(buffer.height(), 20);
        assert_eq!(buffer.get(5, 5).unwrap().character, 'X'); // Preserved
        assert_eq!(buffer.get(15, 15).unwrap().character, ' '); // New default
    }

    #[test]
    fn test_buffer_clear() {
        let mut buffer = Buffer::new(10, 10);
        buffer.set(5, 5, Cell::new('X'));

        buffer.clear();

        assert_eq!(buffer.get(5, 5).unwrap().character, ' ');
    }

    #[test]
    fn test_buffer_clear_with_bg() {
        let mut buffer = Buffer::new(10, 10);

        buffer.clear_with_bg(Color::BLUE);

        assert_eq!(buffer.get(0, 0).unwrap().bg, Color::BLUE);
        assert_eq!(buffer.get(5, 5).unwrap().bg, Color::BLUE);
    }

    #[test]
    fn test_buffer_row() {
        let mut buffer = Buffer::new(10, 10);
        buffer.set(5, 3, Cell::new('A'));

        let row = buffer.row(3).unwrap();
        assert_eq!(row.len(), 10);
        assert_eq!(row[5].character, 'A');
    }

    #[test]
    fn test_buffer_opacity() {
        let mut buffer = Buffer::new(10, 10);

        buffer.push_opacity(0.5);

        let cell = Cell::new('X').with_fg(Color::rgba(1.0, 1.0, 1.0, 1.0));
        buffer.set(5, 5, cell);

        // Alpha should be multiplied by 0.5
        let retrieved = buffer.get(5, 5).unwrap();
        assert!((retrieved.fg.a - 0.5).abs() < 0.001);
    }
}
