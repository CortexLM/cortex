//! Mock terminal backend for headless TUI testing.
//!
//! This module provides a mock terminal that captures all rendering operations
//! without needing an actual terminal. It's designed for testing and debugging
//! TUI applications in CI/CD environments or automated test suites.

use crate::capture::BufferSnapshot;
use crate::config::CaptureConfig;
use crate::types::{ActionType, CapturedFrame, TuiAction};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer as RatatuiBuffer;
use ratatui::layout::{Position, Rect, Size};
use std::io;
use uuid::Uuid;

/// A mock backend that captures all rendering to an internal buffer.
///
/// This backend doesn't actually write to any terminal - it stores all
/// rendered content in memory for inspection and testing.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui_capture::{MockBackend, MockTerminal};
/// use ratatui::widgets::Paragraph;
///
/// let backend = MockBackend::new(80, 24);
/// let mut terminal = MockTerminal::new(backend)?;
///
/// terminal.draw(|frame| {
///     let widget = Paragraph::new("Hello!");
///     frame.render_widget(widget, frame.area());
/// })?;
///
/// let snapshot = terminal.backend().snapshot();
/// println!("{}", snapshot.to_ascii(&Default::default()));
/// ```
#[derive(Debug, Clone)]
pub struct MockBackend {
    /// Internal buffer
    buffer: RatatuiBuffer,

    /// Cursor position
    cursor: Option<(u16, u16)>,

    /// Cursor visibility
    cursor_visible: bool,

    /// Width
    width: u16,

    /// Height
    height: u16,

    /// Number of flush operations
    flush_count: u64,

    /// Number of draw operations
    draw_count: u64,

    /// History of rendered buffers (for debugging)
    buffer_history: Vec<RatatuiBuffer>,

    /// Maximum history size
    max_history: usize,
}

impl MockBackend {
    /// Create a new mock backend with the specified dimensions.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            buffer: RatatuiBuffer::empty(Rect::new(0, 0, width, height)),
            cursor: None,
            cursor_visible: true,
            width,
            height,
            flush_count: 0,
            draw_count: 0,
            buffer_history: Vec::new(),
            max_history: 100,
        }
    }

    /// Create from a capture configuration.
    pub fn from_config(config: &CaptureConfig) -> Self {
        Self::new(config.width, config.height)
    }

    /// Get the current buffer.
    pub fn buffer(&self) -> &RatatuiBuffer {
        &self.buffer
    }

    /// Get the cursor position.
    pub fn cursor(&self) -> Option<(u16, u16)> {
        self.cursor
    }

    /// Check if cursor is visible.
    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// Get the number of flush operations.
    pub fn flush_count(&self) -> u64 {
        self.flush_count
    }

    /// Get the number of draw operations.
    pub fn draw_count(&self) -> u64 {
        self.draw_count
    }

    /// Get the buffer history.
    pub fn buffer_history(&self) -> &[RatatuiBuffer] {
        &self.buffer_history
    }

    /// Set the maximum history size.
    pub fn set_max_history(&mut self, max: usize) {
        self.max_history = max;
        while self.buffer_history.len() > max {
            self.buffer_history.remove(0);
        }
    }

    /// Clear the buffer history.
    pub fn clear_history(&mut self) {
        self.buffer_history.clear();
    }

    /// Create a snapshot of the current buffer.
    pub fn snapshot(&self) -> BufferSnapshot {
        let mut snapshot = BufferSnapshot::from_ratatui_buffer(&self.buffer);
        if let Some((x, y)) = self.cursor
            && self.cursor_visible
        {
            snapshot.set_cursor(x, y);
        }
        snapshot
    }

    /// Resize the backend.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.buffer = RatatuiBuffer::empty(Rect::new(0, 0, width, height));
    }

    /// Get the cell at a specific position.
    pub fn cell(&self, x: u16, y: u16) -> Option<&ratatui::buffer::Cell> {
        self.buffer.cell((x, y))
    }

    /// Get the character at a specific position.
    pub fn char_at(&self, x: u16, y: u16) -> Option<char> {
        self.buffer
            .cell((x, y))
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
    }

    /// Get a string of characters from a row.
    pub fn row_string(&self, y: u16) -> String {
        let mut result = String::with_capacity(self.width as usize);
        for x in 0..self.width {
            result.push(self.char_at(x, y).unwrap_or(' '));
        }
        result.trim_end().to_string()
    }

    /// Get all rows as strings.
    pub fn all_rows(&self) -> Vec<String> {
        (0..self.height).map(|y| self.row_string(y)).collect()
    }

    /// Check if the buffer contains a specific string.
    pub fn contains(&self, text: &str) -> bool {
        self.all_rows().iter().any(|row| row.contains(text))
    }

    /// Find the position of a string in the buffer.
    pub fn find(&self, text: &str) -> Option<(u16, u16)> {
        for y in 0..self.height {
            let row = self.row_string(y);
            if let Some(x) = row.find(text) {
                return Some((x as u16, y));
            }
        }
        None
    }
}

impl Backend for MockBackend {
    type Error = io::Error;

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> io::Result<()> {
        use ratatui::backend::ClearType;
        match clear_type {
            ClearType::All => {
                self.buffer = RatatuiBuffer::empty(Rect::new(0, 0, self.width, self.height));
            }

            ClearType::AfterCursor => {
                if let Some((cx, cy)) = self.cursor {
                    for x in cx..self.width {
                        if let Some(cell) = self.buffer.cell_mut((x, cy)) {
                            cell.reset();
                        }
                    }
                    for y in (cy + 1)..self.height {
                        for x in 0..self.width {
                            if let Some(cell) = self.buffer.cell_mut((x, y)) {
                                cell.reset();
                            }
                        }
                    }
                }
            }
            ClearType::BeforeCursor => {
                if let Some((cx, cy)) = self.cursor {
                    for y in 0..cy {
                        for x in 0..self.width {
                            if let Some(cell) = self.buffer.cell_mut((x, y)) {
                                cell.reset();
                            }
                        }
                    }
                    for x in 0..=cx {
                        if let Some(cell) = self.buffer.cell_mut((x, cy)) {
                            cell.reset();
                        }
                    }
                }
            }
            ClearType::CurrentLine => {
                if let Some((_, cy)) = self.cursor {
                    for x in 0..self.width {
                        if let Some(cell) = self.buffer.cell_mut((x, cy)) {
                            cell.reset();
                        }
                    }
                }
            }
            ClearType::UntilNewLine => {
                if let Some((cx, cy)) = self.cursor {
                    for x in cx..self.width {
                        if let Some(cell) = self.buffer.cell_mut((x, cy)) {
                            cell.reset();
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        for (x, y, cell) in content {
            if x < self.width && y < self.height {
                let buf_cell = self.buffer.cell_mut((x, y));
                if let Some(c) = buf_cell {
                    *c = cell.clone();
                }
            }
        }
        self.draw_count += 1;

        // Save to history
        if self.buffer_history.len() >= self.max_history {
            self.buffer_history.remove(0);
        }
        self.buffer_history.push(self.buffer.clone());

        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = true;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        let (x, y) = self.cursor.unwrap_or((0, 0));
        Ok(Position::new(x, y))
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let pos = position.into();
        self.cursor = Some((pos.x, pos.y));
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.buffer = RatatuiBuffer::empty(Rect::new(0, 0, self.width, self.height));
        Ok(())
    }

    fn size(&self) -> io::Result<Size> {
        Ok(Size::new(self.width, self.height))
    }

    fn window_size(&mut self) -> io::Result<ratatui::backend::WindowSize> {
        Ok(ratatui::backend::WindowSize {
            columns_rows: Size::new(self.width, self.height),
            pixels: Size::new(self.width * 8, self.height * 16), // Assume 8x16 cell size
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_count += 1;
        Ok(())
    }

    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        // Simulate scrolling up by shifting rows
        let amount = amount.min(region.end - region.start);
        if amount == 0 {
            return Ok(());
        }

        for y in region.start..(region.end - amount) {
            for x in 0..self.width {
                if let Some(src_cell) = self.buffer.cell((x, y + amount)).cloned()
                    && let Some(dst_cell) = self.buffer.cell_mut((x, y))
                {
                    *dst_cell = src_cell;
                }
            }
        }

        // Clear the newly exposed rows at the bottom
        for y in (region.end - amount)..region.end {
            for x in 0..self.width {
                if let Some(cell) = self.buffer.cell_mut((x, y)) {
                    cell.reset();
                }
            }
        }

        Ok(())
    }

    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        // Simulate scrolling down by shifting rows
        let amount = amount.min(region.end - region.start);
        if amount == 0 {
            return Ok(());
        }

        for y in (region.start + amount..region.end).rev() {
            for x in 0..self.width {
                if let Some(src_cell) = self.buffer.cell((x, y - amount)).cloned()
                    && let Some(dst_cell) = self.buffer.cell_mut((x, y))
                {
                    *dst_cell = src_cell;
                }
            }
        }

        // Clear the newly exposed rows at the top
        for y in region.start..(region.start + amount) {
            for x in 0..self.width {
                if let Some(cell) = self.buffer.cell_mut((x, y)) {
                    cell.reset();
                }
            }
        }

        Ok(())
    }
}

/// A mock terminal for headless TUI testing.
///
/// This wraps a `MockBackend` in a ratatui `Terminal` and provides
/// additional utilities for testing and debugging.
pub struct MockTerminal {
    /// The underlying terminal
    terminal: Terminal<MockBackend>,

    /// Captured frames
    frames: Vec<CapturedFrame>,

    /// Frame counter
    frame_counter: u64,

    /// Actions recorded
    actions: Vec<TuiAction>,

    /// Action counter
    action_counter: u64,

    /// Configuration
    config: CaptureConfig,
}

impl MockTerminal {
    /// Create a new mock terminal with the specified backend.
    pub fn new(backend: MockBackend) -> io::Result<Self> {
        let config = CaptureConfig::new(backend.width, backend.height);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            frames: Vec::new(),
            frame_counter: 0,
            actions: Vec::new(),
            action_counter: 0,
            config,
        })
    }

    /// Create a new mock terminal with specific dimensions.
    pub fn with_size(width: u16, height: u16) -> io::Result<Self> {
        let backend = MockBackend::new(width, height);
        Self::new(backend)
    }

    /// Create from configuration.
    pub fn from_config(config: CaptureConfig) -> io::Result<Self> {
        let backend = MockBackend::from_config(&config);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            frames: Vec::new(),
            frame_counter: 0,
            actions: Vec::new(),
            action_counter: 0,
            config,
        })
    }

    /// Get the underlying terminal.
    pub fn terminal(&self) -> &Terminal<MockBackend> {
        &self.terminal
    }

    /// Get mutable access to the underlying terminal.
    pub fn terminal_mut(&mut self) -> &mut Terminal<MockBackend> {
        &mut self.terminal
    }

    /// Get the backend.
    pub fn backend(&self) -> &MockBackend {
        self.terminal.backend()
    }

    /// Get mutable access to the backend.
    pub fn backend_mut(&mut self) -> &mut MockBackend {
        self.terminal.backend_mut()
    }

    /// Get the configuration.
    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }

    /// Draw a frame using a closure.
    pub fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        self.terminal.draw(f)?;
        Ok(())
    }

    /// Draw a frame and capture it.
    pub fn draw_and_capture<F>(&mut self, f: F, label: Option<&str>) -> io::Result<Uuid>
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        self.terminal.draw(f)?;
        Ok(self.capture_frame(label))
    }

    /// Capture the current frame.
    pub fn capture_frame(&mut self, label: Option<&str>) -> Uuid {
        self.frame_counter += 1;

        let snapshot = self.backend().snapshot();
        let ascii_content = snapshot.to_ascii(&self.config);

        let mut frame = CapturedFrame::new(
            self.frame_counter,
            ascii_content,
            self.config.width,
            self.config.height,
        );

        if let Some(l) = label {
            frame = frame.with_label(l);
        }

        // Attach recent actions to this frame
        let recent_actions: Vec<TuiAction> = self.actions.clone();
        frame = frame.with_actions(recent_actions);

        let id = frame.id;
        self.frames.push(frame);
        id
    }

    /// Record an action.
    pub fn record_action(&mut self, action_type: ActionType) {
        self.action_counter += 1;
        let action = TuiAction::new(action_type).with_sequence(self.action_counter);
        self.actions.push(action);
    }

    /// Record a key press.
    pub fn key_press(&mut self, key: &str) {
        self.record_action(ActionType::KeyPress(key.to_string()));
    }

    /// Record a mouse click.
    pub fn mouse_click(&mut self, x: u16, y: u16, button: &str) {
        self.record_action(ActionType::MouseClick {
            x,
            y,
            button: button.to_string(),
        });
    }

    /// Get all captured frames.
    pub fn frames(&self) -> &[CapturedFrame] {
        &self.frames
    }

    /// Get all recorded actions.
    pub fn actions(&self) -> &[TuiAction] {
        &self.actions
    }

    /// Get the latest captured frame.
    pub fn latest_frame(&self) -> Option<&CapturedFrame> {
        self.frames.last()
    }

    /// Clear all captured data.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.actions.clear();
        self.frame_counter = 0;
        self.action_counter = 0;
    }

    /// Resize the terminal.
    pub fn resize(&mut self, width: u16, height: u16) -> io::Result<()> {
        self.backend_mut().resize(width, height);
        self.config.width = width;
        self.config.height = height;
        Ok(())
    }

    /// Get terminal size.
    pub fn size(&self) -> (u16, u16) {
        (self.config.width, self.config.height)
    }

    /// Get the current buffer snapshot.
    pub fn snapshot(&self) -> BufferSnapshot {
        self.backend().snapshot()
    }

    /// Check if the terminal contains a specific string.
    pub fn contains(&self, text: &str) -> bool {
        self.backend().contains(text)
    }

    /// Find a string in the terminal.
    pub fn find(&self, text: &str) -> Option<(u16, u16)> {
        self.backend().find(text)
    }

    /// Assert that the terminal contains a specific string.
    pub fn assert_contains(&self, text: &str) {
        assert!(
            self.contains(text),
            "Expected terminal to contain '{}', but it doesn't.\nTerminal content:\n{}",
            text,
            self.snapshot().to_ascii(&self.config)
        );
    }

    /// Assert that the terminal does not contain a specific string.
    pub fn assert_not_contains(&self, text: &str) {
        assert!(
            !self.contains(text),
            "Expected terminal NOT to contain '{}', but it does.\nTerminal content:\n{}",
            text,
            self.snapshot().to_ascii(&self.config)
        );
    }

    /// Generate a markdown report of all captured frames.
    pub fn to_markdown(&self) -> String {
        let mut result = String::new();

        if let Some(title) = &self.config.title {
            result.push_str(&format!("# {}\n\n", title));
        } else {
            result.push_str("# TUI Capture Report\n\n");
        }

        result.push_str(&format!(
            "**Terminal Size:** {}x{}\n",
            self.config.width, self.config.height
        ));
        result.push_str(&format!("**Total Frames:** {}\n", self.frames.len()));
        result.push_str(&format!("**Total Actions:** {}\n\n", self.actions.len()));

        for frame in &self.frames {
            if self.config.labeled_frames_only && frame.label.is_none() {
                continue;
            }

            if let Some(label) = &frame.label {
                result.push_str(&format!("## Frame {} - {}\n\n", frame.frame_number, label));
            } else {
                result.push_str(&format!("## Frame {}\n\n", frame.frame_number));
            }

            if self.config.include_timestamps {
                result.push_str(&format!(
                    "**Timestamp:** {}\n\n",
                    frame.timestamp.format("%H:%M:%S%.3f")
                ));
            }

            if self.config.include_actions && !frame.preceding_actions.is_empty() {
                result.push_str("**Actions:**\n");
                for action in &frame.preceding_actions {
                    result.push_str(&format!(
                        "- {} {}\n",
                        action.action_type.icon(),
                        action.action_type.description()
                    ));
                }
                result.push('\n');
            }

            result.push_str("```\n");
            result.push_str(&frame.ascii_content);
            result.push_str("\n```\n\n");

            if self.config.add_frame_separators {
                result.push_str("---\n\n");
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::Paragraph;

    #[test]
    fn test_mock_backend_new() {
        let backend = MockBackend::new(80, 24);
        assert_eq!(backend.width, 80);
        assert_eq!(backend.height, 24);
    }

    #[test]
    fn test_mock_backend_resize() {
        let mut backend = MockBackend::new(80, 24);
        backend.resize(100, 30);
        assert_eq!(backend.width, 100);
        assert_eq!(backend.height, 30);
    }

    #[test]
    fn test_mock_terminal_draw() {
        let mut terminal = MockTerminal::with_size(80, 24).unwrap();

        terminal
            .draw(|frame| {
                let widget = Paragraph::new("Hello, TUI!");
                frame.render_widget(widget, frame.area());
            })
            .unwrap();

        assert!(terminal.contains("Hello, TUI!"));
    }

    #[test]
    fn test_mock_terminal_capture() {
        let mut terminal = MockTerminal::with_size(80, 24).unwrap();

        terminal
            .draw(|frame| {
                let widget = Paragraph::new("Test Content");
                frame.render_widget(widget, frame.area());
            })
            .unwrap();

        terminal.capture_frame(Some("Initial state"));

        assert_eq!(terminal.frames().len(), 1);
        assert!(terminal.latest_frame().unwrap().label.is_some());
    }

    #[test]
    fn test_mock_terminal_actions() {
        let mut terminal = MockTerminal::with_size(80, 24).unwrap();

        terminal.key_press("Enter");
        terminal.key_press("a");
        terminal.mouse_click(10, 5, "left");

        assert_eq!(terminal.actions().len(), 3);
    }

    #[test]
    fn test_mock_terminal_markdown() {
        let config = CaptureConfig::new(40, 10)
            .with_title("Test Report")
            .with_timestamps(true);
        let mut terminal = MockTerminal::from_config(config).unwrap();

        terminal
            .draw(|frame| {
                let widget = Paragraph::new("Hello");
                frame.render_widget(widget, frame.area());
            })
            .unwrap();

        terminal.key_press("Enter");
        terminal.capture_frame(Some("After Enter"));

        let md = terminal.to_markdown();
        assert!(md.contains("# Test Report"));
        assert!(md.contains("After Enter"));
        assert!(md.contains("Hello"));
    }
}
