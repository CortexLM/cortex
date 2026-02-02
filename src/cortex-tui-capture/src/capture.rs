//! Frame capture and buffer snapshot utilities.
//!
//! This module provides tools for capturing TUI frames as ASCII art,
//! with support for styled rendering and buffer manipulation.

use crate::config::{CaptureConfig, StyleRendering};
use crate::types::CapturedFrame;
use cortex_tui_buffer::Buffer;
use cortex_tui_core::{Color, Style, TextAttributes};
use ratatui::buffer::Buffer as RatatuiBuffer;
use uuid::Uuid;

/// A snapshot of a buffer that can be converted to ASCII.
#[derive(Debug, Clone)]
pub struct BufferSnapshot {
    /// Cells stored in row-major order
    cells: Vec<SnapshotCell>,

    /// Width in characters
    width: u16,

    /// Height in characters
    height: u16,

    /// Cursor position (if visible)
    cursor: Option<(u16, u16)>,
}

/// A cell in the snapshot.
#[derive(Debug, Clone, Default)]
pub struct SnapshotCell {
    /// The symbol (can be multi-character for grapheme clusters)
    pub symbol: String,

    /// Foreground color
    pub fg: Option<Color>,

    /// Background color
    pub bg: Option<Color>,

    /// Text attributes
    pub attributes: TextAttributes,
}

impl SnapshotCell {
    /// Create a new snapshot cell.
    pub fn new(character: char) -> Self {
        Self {
            symbol: character.to_string(),
            fg: None,
            bg: None,
            attributes: TextAttributes::empty(),
        }
    }

    /// Create a new snapshot cell from a string symbol.
    pub fn from_symbol(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            fg: None,
            bg: None,
            attributes: TextAttributes::empty(),
        }
    }

    /// Create from a style.
    pub fn with_style(character: char, style: Style) -> Self {
        Self {
            symbol: character.to_string(),
            fg: style.fg,
            bg: style.bg,
            attributes: style.attributes,
        }
    }

    /// Create from a symbol and style.
    pub fn symbol_with_style(symbol: impl Into<String>, style: Style) -> Self {
        Self {
            symbol: symbol.into(),
            fg: style.fg,
            bg: style.bg,
            attributes: style.attributes,
        }
    }

    /// Get the first character of the symbol (for backward compatibility).
    pub fn character(&self) -> char {
        self.symbol.chars().next().unwrap_or(' ')
    }
}

impl BufferSnapshot {
    /// Create a new empty snapshot.
    pub fn new(width: u16, height: u16) -> Self {
        let size = width as usize * height as usize;
        Self {
            cells: vec![SnapshotCell::new(' '); size],
            width,
            height,
            cursor: None,
        }
    }

    /// Create from a Cortex TUI buffer.
    pub fn from_cortex_tui_buffer(buffer: &Buffer) -> Self {
        let width = buffer.width();
        let height = buffer.height();
        let mut cells = Vec::with_capacity(width as usize * height as usize);

        for y in 0..height {
            if let Some(row) = buffer.row(y) {
                for cell in row {
                    cells.push(SnapshotCell {
                        symbol: cell.character.to_string(),
                        fg: Some(cell.fg),
                        bg: Some(cell.bg),
                        attributes: cell.attributes,
                    });
                }
            }
        }

        Self {
            cells,
            width,
            height,
            cursor: None,
        }
    }

    /// Create from a ratatui buffer.
    pub fn from_ratatui_buffer(buffer: &RatatuiBuffer) -> Self {
        let area = buffer.area;
        let width = area.width;
        let height = area.height;
        let mut cells = Vec::with_capacity(width as usize * height as usize);

        for y in area.y..area.y + height {
            for x in area.x..area.x + width {
                let cell = buffer.cell((x, y)).cloned().unwrap_or_default();
                // Use the full symbol to preserve multi-character graphemes and Unicode
                let symbol = cell.symbol().to_string();

                // Convert ratatui colors to our format
                let fg = ratatui_color_to_color(cell.fg);
                let bg = ratatui_color_to_color(cell.bg);

                // Convert modifiers to our attributes
                let mut attributes = TextAttributes::empty();
                if cell.modifier.contains(ratatui::style::Modifier::BOLD) {
                    attributes |= TextAttributes::BOLD;
                }
                if cell.modifier.contains(ratatui::style::Modifier::ITALIC) {
                    attributes |= TextAttributes::ITALIC;
                }
                if cell.modifier.contains(ratatui::style::Modifier::UNDERLINED) {
                    attributes |= TextAttributes::UNDERLINE;
                }
                if cell.modifier.contains(ratatui::style::Modifier::DIM) {
                    attributes |= TextAttributes::DIM;
                }

                cells.push(SnapshotCell {
                    symbol,
                    fg,
                    bg,
                    attributes,
                });
            }
        }

        Self {
            cells,
            width,
            height,
            cursor: None,
        }
    }

    /// Set cursor position.
    pub fn set_cursor(&mut self, x: u16, y: u16) {
        self.cursor = Some((x, y));
    }

    /// Get the cell at (x, y).
    pub fn get(&self, x: u16, y: u16) -> Option<&SnapshotCell> {
        if x < self.width && y < self.height {
            let idx = y as usize * self.width as usize + x as usize;
            self.cells.get(idx)
        } else {
            None
        }
    }

    /// Set the cell at (x, y).
    pub fn set(&mut self, x: u16, y: u16, cell: SnapshotCell) {
        if x < self.width && y < self.height {
            let idx = y as usize * self.width as usize + x as usize;
            if idx < self.cells.len() {
                self.cells[idx] = cell;
            }
        }
    }

    /// Convert to plain ASCII string.
    pub fn to_ascii(&self, config: &CaptureConfig) -> String {
        let mut result = String::with_capacity(
            (self.width as usize + 1) * self.height as usize, // +1 for newlines
        );

        for y in 0..self.height {
            let mut line = String::with_capacity(self.width as usize);

            for x in 0..self.width {
                if let Some(cell) = self.get(x, y) {
                    // Check if cursor is at this position
                    if config.show_cursor && self.cursor == Some((x, y)) {
                        line.push(config.cursor_char);
                    } else {
                        // Use the full symbol to preserve Unicode and graphemes
                        line.push_str(&cell.symbol);
                    }
                } else {
                    line.push(' ');
                }
            }

            // Trim trailing whitespace if configured
            if config.trim_trailing_whitespace {
                line = line.trim_end().to_string();
            }

            result.push_str(&line);
            if y < self.height - 1 {
                result.push('\n');
            }
        }

        result
    }

    /// Convert to ASCII with ANSI color codes.
    pub fn to_ansi(&self, config: &CaptureConfig) -> String {
        let mut result = String::with_capacity(
            (self.width as usize + 100) * self.height as usize, // Extra space for ANSI codes
        );

        for y in 0..self.height {
            let mut line = String::new();
            let mut last_fg: Option<Color> = None;
            let mut last_bg: Option<Color> = None;
            let mut last_attrs = TextAttributes::empty();

            for x in 0..self.width {
                if let Some(cell) = self.get(x, y) {
                    // Check if we need to change styles
                    let need_style_change =
                        cell.fg != last_fg || cell.bg != last_bg || cell.attributes != last_attrs;

                    if need_style_change {
                        // Reset and apply new style
                        line.push_str("\x1b[0m");

                        // Apply attributes
                        if cell.attributes.contains(TextAttributes::BOLD) {
                            line.push_str("\x1b[1m");
                        }
                        if cell.attributes.contains(TextAttributes::DIM) {
                            line.push_str("\x1b[2m");
                        }
                        if cell.attributes.contains(TextAttributes::ITALIC) {
                            line.push_str("\x1b[3m");
                        }
                        if cell.attributes.contains(TextAttributes::UNDERLINE) {
                            line.push_str("\x1b[4m");
                        }

                        // Apply foreground color
                        if let Some(fg) = cell.fg {
                            let (r, g, b) = fg.to_rgb_u8();
                            line.push_str(&format!("\x1b[38;2;{};{};{}m", r, g, b));
                        }

                        // Apply background color
                        if let Some(bg) = cell.bg
                            && !bg.is_transparent()
                        {
                            let (r, g, b) = bg.to_rgb_u8();
                            line.push_str(&format!("\x1b[48;2;{};{};{}m", r, g, b));
                        }

                        last_fg = cell.fg;
                        last_bg = cell.bg;
                        last_attrs = cell.attributes;
                    }

                    // Check if cursor is at this position
                    if config.show_cursor && self.cursor == Some((x, y)) {
                        line.push(config.cursor_char);
                    } else {
                        // Use the full symbol to preserve Unicode and graphemes
                        line.push_str(&cell.symbol);
                    }
                } else {
                    line.push(' ');
                }
            }

            // Reset at end of line
            line.push_str("\x1b[0m");

            // Trim trailing whitespace if configured (but keep ANSI reset)
            if config.trim_trailing_whitespace {
                // This is a simplified approach - in practice we'd want to be smarter
                let trimmed = line.trim_end_matches(' ');
                if !trimmed.ends_with("\x1b[0m") {
                    line = format!("{}\x1b[0m", trimmed.trim_end_matches("\x1b[0m"));
                }
            }

            result.push_str(&line);
            if y < self.height - 1 {
                result.push('\n');
            }
        }

        result
    }

    /// Get dimensions.
    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Width accessor.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Height accessor.
    pub fn height(&self) -> u16 {
        self.height
    }
}

/// Convert ratatui color to our Color type.
fn ratatui_color_to_color(color: ratatui::style::Color) -> Option<Color> {
    match color {
        ratatui::style::Color::Reset => None,
        ratatui::style::Color::Black => Some(Color::BLACK),
        ratatui::style::Color::Red => Some(Color::RED),
        ratatui::style::Color::Green => Some(Color::GREEN),
        ratatui::style::Color::Yellow => Some(Color::YELLOW),
        ratatui::style::Color::Blue => Some(Color::BLUE),
        ratatui::style::Color::Magenta => Some(Color::MAGENTA),
        ratatui::style::Color::Cyan => Some(Color::CYAN),
        ratatui::style::Color::Gray => Some(Color::new(0.5, 0.5, 0.5, 1.0)),
        ratatui::style::Color::DarkGray => Some(Color::new(0.25, 0.25, 0.25, 1.0)),
        ratatui::style::Color::LightRed => Some(Color::new(1.0, 0.5, 0.5, 1.0)),
        ratatui::style::Color::LightGreen => Some(Color::new(0.5, 1.0, 0.5, 1.0)),
        ratatui::style::Color::LightYellow => Some(Color::new(1.0, 1.0, 0.5, 1.0)),
        ratatui::style::Color::LightBlue => Some(Color::new(0.5, 0.5, 1.0, 1.0)),
        ratatui::style::Color::LightMagenta => Some(Color::new(1.0, 0.5, 1.0, 1.0)),
        ratatui::style::Color::LightCyan => Some(Color::new(0.5, 1.0, 1.0, 1.0)),
        ratatui::style::Color::White => Some(Color::WHITE),
        ratatui::style::Color::Rgb(r, g, b) => Some(Color::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            1.0,
        )),
        ratatui::style::Color::Indexed(idx) => {
            // Simplified 256-color palette conversion
            Some(indexed_color_to_rgb(idx))
        }
    }
}

/// Convert 256-color index to RGB color.
fn indexed_color_to_rgb(idx: u8) -> Color {
    match idx {
        // Standard colors (0-15)
        0 => Color::BLACK,
        1 => Color::new(0.5, 0.0, 0.0, 1.0),
        2 => Color::new(0.0, 0.5, 0.0, 1.0),
        3 => Color::new(0.5, 0.5, 0.0, 1.0),
        4 => Color::new(0.0, 0.0, 0.5, 1.0),
        5 => Color::new(0.5, 0.0, 0.5, 1.0),
        6 => Color::new(0.0, 0.5, 0.5, 1.0),
        7 => Color::new(0.75, 0.75, 0.75, 1.0),
        8 => Color::new(0.5, 0.5, 0.5, 1.0),
        9 => Color::RED,
        10 => Color::GREEN,
        11 => Color::YELLOW,
        12 => Color::BLUE,
        13 => Color::MAGENTA,
        14 => Color::CYAN,
        15 => Color::WHITE,
        // 216 colors (16-231): 6x6x6 color cube
        16..=231 => {
            let idx = idx - 16;
            let r = (idx / 36) % 6;
            let g = (idx / 6) % 6;
            let b = idx % 6;
            let to_float = |v: u8| {
                if v == 0 {
                    0.0
                } else {
                    (v as f32 * 40.0 + 55.0) / 255.0
                }
            };
            Color::new(to_float(r), to_float(g), to_float(b), 1.0)
        }
        // Grayscale (232-255)
        232..=255 => {
            let gray = ((idx - 232) as f32 * 10.0 + 8.0) / 255.0;
            Color::new(gray, gray, gray, 1.0)
        }
    }
}

/// Frame capture utility for rendering TUI content to ASCII.
pub struct FrameCapture {
    /// Configuration
    config: CaptureConfig,

    /// Captured frames
    frames: Vec<CapturedFrame>,

    /// Frame counter
    frame_counter: u64,

    /// Internal buffer for rendering
    buffer: BufferSnapshot,
}

impl FrameCapture {
    /// Create a new frame capture with the given configuration.
    pub fn new(config: CaptureConfig) -> Self {
        let buffer = BufferSnapshot::new(config.width, config.height);
        Self {
            config,
            frames: Vec::new(),
            frame_counter: 0,
            buffer,
        }
    }

    /// Get the configuration.
    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }

    /// Get captured frames.
    pub fn frames(&self) -> &[CapturedFrame] {
        &self.frames
    }

    /// Get the current frame count.
    pub fn frame_count(&self) -> u64 {
        self.frame_counter
    }

    /// Capture a frame from a ratatui buffer.
    pub fn capture_ratatui(&mut self, buffer: &RatatuiBuffer, label: Option<&str>) -> Uuid {
        self.buffer = BufferSnapshot::from_ratatui_buffer(buffer);
        self.capture_internal(label)
    }

    /// Capture a frame from a Cortex TUI buffer.
    pub fn capture_cortex_tui(&mut self, buffer: &Buffer, label: Option<&str>) -> Uuid {
        self.buffer = BufferSnapshot::from_cortex_tui_buffer(buffer);
        self.capture_internal(label)
    }

    /// Capture a frame from a raw buffer snapshot.
    pub fn capture_snapshot(&mut self, snapshot: BufferSnapshot, label: Option<&str>) -> Uuid {
        self.buffer = snapshot;
        self.capture_internal(label)
    }

    /// Internal capture implementation.
    fn capture_internal(&mut self, label: Option<&str>) -> Uuid {
        self.frame_counter += 1;

        let ascii_content = match self.config.style_rendering {
            StyleRendering::Plain => self.buffer.to_ascii(&self.config),
            StyleRendering::Ansi => self.buffer.to_ansi(&self.config),
            StyleRendering::Annotated => self.buffer.to_ascii(&self.config), // Fallback to ASCII (specialized rendering not yet implemented)
            StyleRendering::HtmlTags => self.buffer.to_ascii(&self.config), // Fallback to ASCII (specialized rendering not yet implemented)
        };

        let mut frame = CapturedFrame::new(
            self.frame_counter,
            ascii_content,
            self.config.width,
            self.config.height,
        );

        if let Some(l) = label {
            frame = frame.with_label(l);
        }

        let id = frame.id;
        self.frames.push(frame);
        id
    }

    /// Get the latest captured frame.
    pub fn latest_frame(&self) -> Option<&CapturedFrame> {
        self.frames.last()
    }

    /// Get a frame by ID.
    pub fn get_frame(&self, id: Uuid) -> Option<&CapturedFrame> {
        self.frames.iter().find(|f| f.id == id)
    }

    /// Get frame by number.
    pub fn get_frame_by_number(&self, number: u64) -> Option<&CapturedFrame> {
        self.frames.iter().find(|f| f.frame_number == number)
    }

    /// Clear all captured frames.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.frame_counter = 0;
    }

    /// Convert the latest frame to markdown.
    pub fn to_markdown(&self) -> String {
        if let Some(frame) = self.latest_frame() {
            frame_to_markdown(frame, &self.config)
        } else {
            "No frames captured.".to_string()
        }
    }

    /// Convert all frames to markdown.
    pub fn all_to_markdown(&self) -> String {
        let mut result = String::new();

        if let Some(title) = &self.config.title {
            result.push_str(&format!("# {}\n\n", title));
        }

        if let Some(desc) = &self.config.description {
            result.push_str(&format!("{}\n\n", desc));
        }

        if self.config.include_summary {
            result.push_str(&format!("**Total Frames:** {}\n\n", self.frames.len()));
        }

        for frame in &self.frames {
            if self.config.labeled_frames_only && frame.label.is_none() {
                continue;
            }
            result.push_str(&frame_to_markdown(frame, &self.config));
            if self.config.add_frame_separators {
                result.push_str("\n---\n\n");
            }
        }

        result
    }
}

/// Convert a captured frame to markdown format.
fn frame_to_markdown(frame: &CapturedFrame, config: &CaptureConfig) -> String {
    let mut result = String::new();

    // Frame header
    if config.include_frame_numbers {
        if let Some(label) = &frame.label {
            result.push_str(&format!("### Frame {} - {}\n\n", frame.frame_number, label));
        } else {
            result.push_str(&format!("### Frame {}\n\n", frame.frame_number));
        }
    } else if let Some(label) = &frame.label {
        result.push_str(&format!("### {}\n\n", label));
    }

    // Timestamp
    if config.include_timestamps {
        result.push_str(&format!(
            "**Timestamp:** {}\n\n",
            frame.timestamp.format("%Y-%m-%d %H:%M:%S%.3f UTC")
        ));
    }

    // Metadata
    if config.include_metadata && !frame.metadata.is_empty() {
        result.push_str("**Metadata:**\n");
        for (key, value) in &frame.metadata {
            result.push_str(&format!("- {}: {}\n", key, value));
        }
        result.push('\n');
    }

    // Preceding actions
    if config.include_actions && !frame.preceding_actions.is_empty() {
        result.push_str("**Actions:**\n");
        let actions_to_show = if frame.preceding_actions.len() > config.max_actions_per_frame {
            &frame.preceding_actions[frame.preceding_actions.len() - config.max_actions_per_frame..]
        } else {
            &frame.preceding_actions
        };
        for action in actions_to_show {
            result.push_str(&format!(
                "- {} {}\n",
                action.action_type.icon(),
                action.action_type.description()
            ));
        }
        result.push('\n');
    }

    // ASCII content
    result.push_str("```\n");
    result.push_str(&frame.ascii_content);
    result.push_str("\n```\n\n");

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_snapshot_new() {
        let snapshot = BufferSnapshot::new(80, 24);
        assert_eq!(snapshot.width(), 80);
        assert_eq!(snapshot.height(), 24);
    }

    #[test]
    fn test_buffer_snapshot_set_get() {
        let mut snapshot = BufferSnapshot::new(10, 10);
        let cell = SnapshotCell::new('X');
        snapshot.set(5, 5, cell.clone());

        let retrieved = snapshot.get(5, 5).unwrap();
        assert_eq!(retrieved.symbol, "X");
        assert_eq!(retrieved.character(), 'X');
    }

    #[test]
    fn test_buffer_snapshot_to_ascii() {
        let mut snapshot = BufferSnapshot::new(5, 3);
        for x in 0..5 {
            snapshot.set(x, 1, SnapshotCell::new('*'));
        }

        let config = CaptureConfig::minimal(5, 3);
        let ascii = snapshot.to_ascii(&config);

        assert!(ascii.contains("*****"));
    }

    #[test]
    fn test_frame_capture() {
        let config = CaptureConfig::new(10, 5).with_title("Test");
        let mut capture = FrameCapture::new(config);

        let snapshot = BufferSnapshot::new(10, 5);
        capture.capture_snapshot(snapshot, Some("Initial"));

        assert_eq!(capture.frame_count(), 1);
        assert!(capture.latest_frame().is_some());
    }

    #[test]
    fn test_frame_to_markdown() {
        let config = CaptureConfig::new(10, 5)
            .with_timestamps(true)
            .with_frame_numbers(true);
        let mut capture = FrameCapture::new(config);

        let snapshot = BufferSnapshot::new(10, 5);
        capture.capture_snapshot(snapshot, Some("Test Frame"));

        let md = capture.to_markdown();
        assert!(md.contains("Frame 1"));
        assert!(md.contains("Test Frame"));
        assert!(md.contains("```"));
    }
}
