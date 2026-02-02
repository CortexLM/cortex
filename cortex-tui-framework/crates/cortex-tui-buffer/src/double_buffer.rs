//! Double-buffering for flicker-free terminal rendering.
//!
//! Double-buffering is a technique where rendering happens to a "back" buffer
//! while the "front" buffer represents what's currently displayed. After
//! rendering is complete, the buffers are swapped, and only the differences
//! are sent to the terminal.

use crate::{diff, Buffer, BufferDiff, Cell, DiffOptions};

use cortex_tui_core::Color;

/// Double-buffered terminal rendering.
///
/// [`DoubleBuffer`] manages two buffers:
/// - **Front buffer**: Represents what's currently displayed on the terminal
/// - **Back buffer**: Where new content is rendered before display
///
/// After rendering to the back buffer, call [`diff`](Self::diff) to compute
/// the changes, then [`swap`](Self::swap) to make the back buffer current.
///
/// # Examples
///
/// ```
/// use cortex_tui_buffer::{DoubleBuffer, diff::DiffOptions};
/// use cortex_tui_core::{Color, Style};
///
/// let mut db = DoubleBuffer::new(80, 24);
///
/// // Render to back buffer
/// db.back_mut().draw_str(10, 5, "Hello, World!", Style::default());
///
/// // Compute differences
/// let diff = db.diff(&DiffOptions::default());
///
/// // In real code, you would send `diff` to the terminal here
///
/// // Swap buffers
/// db.swap();
/// ```
pub struct DoubleBuffer {
    /// The front buffer (currently displayed).
    front: Buffer,
    /// The back buffer (being rendered to).
    back: Buffer,
    /// Whether a full redraw is needed (e.g., after resize).
    force_full_redraw: bool,
}

impl DoubleBuffer {
    /// Creates a new double buffer with the specified dimensions.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            front: Buffer::new(width, height),
            back: Buffer::new(width, height),
            force_full_redraw: true,
        }
    }

    /// Returns the buffer width.
    #[inline]
    pub fn width(&self) -> u16 {
        self.back.width()
    }

    /// Returns the buffer height.
    #[inline]
    pub fn height(&self) -> u16 {
        self.back.height()
    }

    /// Returns the buffer dimensions as (width, height).
    #[inline]
    pub fn size(&self) -> (u16, u16) {
        self.back.size()
    }

    /// Returns a reference to the front buffer (currently displayed).
    #[inline]
    pub fn front(&self) -> &Buffer {
        &self.front
    }

    /// Returns a reference to the back buffer (being rendered to).
    #[inline]
    pub fn back(&self) -> &Buffer {
        &self.back
    }

    /// Returns a mutable reference to the back buffer for rendering.
    #[inline]
    pub fn back_mut(&mut self) -> &mut Buffer {
        &mut self.back
    }

    /// Swaps the front and back buffers.
    ///
    /// After calling this, the back buffer becomes the new front buffer
    /// (representing what's displayed), and the old front buffer becomes
    /// the new back buffer (ready for the next frame).
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.front, &mut self.back);
        self.force_full_redraw = false;
    }

    /// Computes the difference between front and back buffers.
    ///
    /// Returns a [`BufferDiff`] containing the changes needed to update
    /// the display from the front buffer to the back buffer.
    ///
    /// If [`force_full_redraw`](Self::force_full_redraw) is set (e.g., after
    /// resize), this returns a full-buffer diff.
    pub fn diff(&self, options: &DiffOptions) -> BufferDiff {
        if self.force_full_redraw {
            diff::compute_full(&self.back)
        } else {
            diff::compute(&self.front, &self.back, options)
        }
    }

    /// Computes the difference with default options.
    pub fn diff_default(&self) -> BufferDiff {
        self.diff(&DiffOptions::default())
    }

    /// Resizes both buffers to new dimensions.
    ///
    /// This clears both buffers and sets the force_full_redraw flag,
    /// so the next [`diff`](Self::diff) call will return a full-buffer diff.
    pub fn resize(&mut self, width: u16, height: u16) {
        if width == self.width() && height == self.height() {
            return;
        }

        self.front.resize_and_clear(width, height);
        self.back.resize_and_clear(width, height);
        self.force_full_redraw = true;
    }

    /// Clears the back buffer with the default cell.
    pub fn clear(&mut self) {
        self.back.clear();
    }

    /// Clears the back buffer with a specific background color.
    pub fn clear_with_bg(&mut self, bg: Color) {
        self.back.clear_with_bg(bg);
    }

    /// Forces a full redraw on the next [`diff`](Self::diff) call.
    ///
    /// Use this after terminal capabilities change or when you need
    /// to ensure the entire screen is redrawn.
    pub fn force_redraw(&mut self) {
        self.force_full_redraw = true;
    }

    /// Returns true if a full redraw will be performed on next diff.
    #[inline]
    pub fn needs_full_redraw(&self) -> bool {
        self.force_full_redraw
    }

    /// Synchronizes the front buffer to match the back buffer.
    ///
    /// This copies all cells from back to front without generating a diff.
    /// Useful after directly sending the back buffer contents to the terminal.
    pub fn sync_front_to_back(&mut self) {
        for y in 0..self.back.height() {
            for x in 0..self.back.width() {
                if let Some(cell) = self.back.get(x, y) {
                    self.front.set_raw(x, y, *cell);
                }
            }
        }
        self.force_full_redraw = false;
    }

    /// Updates the front buffer with the given diff.
    ///
    /// Call this after successfully rendering the diff to the terminal
    /// to keep the front buffer in sync with the display.
    pub fn apply_diff_to_front(&mut self, diff: &BufferDiff) {
        diff::apply(diff, &mut self.front);
        self.force_full_redraw = false;
    }

    /// Sets a cell in the back buffer.
    #[inline]
    pub fn set(&mut self, x: u16, y: u16, cell: Cell) -> bool {
        self.back.set(x, y, cell)
    }

    /// Gets a cell from the back buffer.
    #[inline]
    pub fn get(&self, x: u16, y: u16) -> Option<&Cell> {
        self.back.get(x, y)
    }

    /// Checks if a cell has changed between front and back buffers.
    ///
    /// Returns `true` if the cell at (x, y) is different between buffers,
    /// or `None` if coordinates are out of bounds.
    pub fn cell_changed(&self, x: u16, y: u16) -> Option<bool> {
        let front_cell = self.front.get(x, y)?;
        let back_cell = self.back.get(x, y)?;
        Some(front_cell != back_cell)
    }

    /// Counts the total number of changed cells.
    pub fn count_changes(&self) -> usize {
        let mut count = 0;
        for y in 0..self.height() {
            for x in 0..self.width() {
                if self.cell_changed(x, y) == Some(true) {
                    count += 1;
                }
            }
        }
        count
    }

    /// Returns the percentage of cells that have changed.
    pub fn change_percentage(&self) -> f32 {
        let total = self.width() as usize * self.height() as usize;
        if total == 0 {
            return 0.0;
        }
        (self.count_changes() as f32 / total as f32) * 100.0
    }
}

impl Clone for DoubleBuffer {
    fn clone(&self) -> Self {
        Self {
            front: self.front.clone(),
            back: self.back.clone(),
            force_full_redraw: self.force_full_redraw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_double_buffer_creation() {
        let db = DoubleBuffer::new(80, 24);
        assert_eq!(db.width(), 80);
        assert_eq!(db.height(), 24);
        assert!(db.needs_full_redraw());
    }

    #[test]
    fn test_double_buffer_render_and_swap() {
        let mut db = DoubleBuffer::new(20, 10);

        // Render to back buffer
        db.back_mut().draw_str_default(0, 0, "Hello");

        // Back buffer should have content
        assert_eq!(db.back().get(0, 0).unwrap().character, 'H');

        // Front buffer should be empty
        assert_eq!(db.front().get(0, 0).unwrap().character, ' ');

        // Swap
        db.swap();

        // Now front should have content
        assert_eq!(db.front().get(0, 0).unwrap().character, 'H');

        // Back should be the old front (empty)
        assert_eq!(db.back().get(0, 0).unwrap().character, ' ');
    }

    #[test]
    fn test_double_buffer_diff() {
        let mut db = DoubleBuffer::new(20, 10);

        // First diff should be full redraw
        let diff1 = db.diff_default();
        assert_eq!(diff1.runs.len(), 10); // One run per row

        // Swap and clear full redraw flag
        db.swap();
        assert!(!db.needs_full_redraw());

        // Render some content
        db.back_mut().draw_str_default(5, 3, "Test");

        // Second diff should only contain changes
        let diff2 = db.diff_default();
        assert_eq!(diff2.changed_count, 4);
        assert_eq!(diff2.runs.len(), 1);
    }

    #[test]
    fn test_double_buffer_resize() {
        let mut db = DoubleBuffer::new(10, 10);
        db.swap(); // Clear initial full redraw flag

        db.resize(20, 20);

        assert_eq!(db.width(), 20);
        assert_eq!(db.height(), 20);
        assert!(db.needs_full_redraw());
    }

    #[test]
    fn test_double_buffer_force_redraw() {
        let mut db = DoubleBuffer::new(10, 10);
        db.swap();

        assert!(!db.needs_full_redraw());

        db.force_redraw();

        assert!(db.needs_full_redraw());
    }

    #[test]
    fn test_double_buffer_cell_changed() {
        let mut db = DoubleBuffer::new(10, 10);
        db.swap(); // Make front match back (both empty)

        // No changes yet
        assert_eq!(db.cell_changed(5, 5), Some(false));

        // Make a change to back
        db.back_mut().set_raw(5, 5, Cell::new('X'));

        // Now it should be changed
        assert_eq!(db.cell_changed(5, 5), Some(true));
    }

    #[test]
    fn test_double_buffer_count_changes() {
        let mut db = DoubleBuffer::new(10, 10);
        db.swap();

        assert_eq!(db.count_changes(), 0);

        db.back_mut().draw_str_default(0, 0, "Hello");

        assert_eq!(db.count_changes(), 5);
    }

    #[test]
    fn test_double_buffer_change_percentage() {
        let mut db = DoubleBuffer::new(10, 10);
        db.swap();

        // 10x10 = 100 cells, 10 changes = 10%
        db.back_mut().draw_str_default(0, 0, "1234567890");

        assert!((db.change_percentage() - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_sync_front_to_back() {
        let mut db = DoubleBuffer::new(10, 10);

        db.back_mut().draw_str_default(0, 0, "Test");
        db.sync_front_to_back();

        // Front should now match back
        assert_eq!(db.front().get(0, 0).unwrap().character, 'T');
        assert!(!db.needs_full_redraw());
    }

    #[test]
    fn test_apply_diff_to_front() {
        let mut db = DoubleBuffer::new(10, 10);
        db.swap();

        db.back_mut().draw_str_default(0, 0, "XYZ");

        let diff = db.diff_default();
        db.apply_diff_to_front(&diff);

        // Front should now have the changes
        assert_eq!(db.front().get(0, 0).unwrap().character, 'X');
        assert_eq!(db.front().get(1, 0).unwrap().character, 'Y');
        assert_eq!(db.front().get(2, 0).unwrap().character, 'Z');
    }

    #[test]
    fn test_clear_with_bg() {
        let mut db = DoubleBuffer::new(10, 10);

        db.clear_with_bg(Color::BLUE);

        assert_eq!(db.back().get(5, 5).unwrap().bg, Color::BLUE);
    }
}
