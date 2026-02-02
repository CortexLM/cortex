//! Terminal buffer for rendering widgets.

use crate::types::{Rect, Style};
pub use cortex_tui_buffer::{Buffer, Cell};

/// RAII guard for setting a clip region and restoring it on drop.
pub struct ClipGuard<'a> {
    buffer: &'a mut Buffer,
    has_scissor: bool,
}

impl<'a> ClipGuard<'a> {
    /// Creates a new clip guard, setting the given clip region.
    pub fn new(buffer: &'a mut Buffer, clip: Rect) -> Self {
        buffer.push_scissor(clip);
        Self {
            buffer,
            has_scissor: true,
        }
    }

    /// Returns a mutable reference to the underlying buffer.
    pub fn buffer(&mut self) -> &mut Buffer {
        self.buffer
    }
}

impl Drop for ClipGuard<'_> {
    fn drop(&mut self) {
        if self.has_scissor {
            self.buffer.pop_scissor();
        }
    }
}

/// Extension trait for Buffer to provide compatibility with old cortex-tui-widgets API.
pub trait BufferExt {
    /// Sets a character at the given position with the given style.
    fn set_char(&mut self, x: i32, y: i32, ch: char, style: Style) -> bool;

    /// Sets a string at the given position with the given style.
    fn set_string(&mut self, x: i32, y: i32, s: &str, style: Style) -> u16;

    /// Returns the clipping region.
    fn clip(&self) -> Rect;

    /// Returns the area of the buffer.
    fn area(&self) -> Rect;

    /// Draws a horizontal line at the given position.
    fn draw_horizontal_line(&mut self, x: i32, y: i32, width: u16, ch: char, style: Style);

    /// Draws a vertical line at the given position.
    fn draw_vertical_line(&mut self, x: i32, y: i32, height: u16, ch: char, style: Style);
}

impl BufferExt for Buffer {
    fn set_char(&mut self, x: i32, y: i32, ch: char, style: Style) -> bool {
        if x < 0 || y < 0 {
            return false;
        }
        self.draw_char(x as u16, y as u16, ch, style)
    }

    fn set_string(&mut self, x: i32, y: i32, s: &str, style: Style) -> u16 {
        if x < 0 || y < 0 {
            return 0;
        }
        self.draw_str(x as u16, y as u16, s, style)
    }

    fn clip(&self) -> Rect {
        self.current_scissor().unwrap_or_else(|| self.bounds())
    }

    fn area(&self) -> Rect {
        self.bounds()
    }

    fn draw_horizontal_line(&mut self, x: i32, y: i32, width: u16, ch: char, style: Style) {
        if y < 0 || y >= self.height() as i32 {
            return;
        }
        let y_u16 = y as u16;
        for i in 0..width {
            let current_x = x + i as i32;
            if current_x >= 0 && current_x < self.width() as i32 {
                self.draw_char(current_x as u16, y_u16, ch, style);
            }
        }
    }

    fn draw_vertical_line(&mut self, x: i32, y: i32, height: u16, ch: char, style: Style) {
        if x < 0 || x >= self.width() as i32 {
            return;
        }
        let x_u16 = x as u16;
        for i in 0..height {
            let current_y = y + i as i32;
            if current_y >= 0 && current_y < self.height() as i32 {
                self.draw_char(x_u16, current_y as u16, ch, style);
            }
        }
    }
}
