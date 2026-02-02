//! Configuration for TUI capture operations.
//!
//! This module provides configuration types that control how frames are captured,
//! rendered, and exported.

use serde::{Deserialize, Serialize};

/// How to render styles in ASCII output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StyleRendering {
    /// No style information (plain ASCII)
    #[default]
    Plain,

    /// Include ANSI escape codes for colors
    Ansi,

    /// Include style markers in a separate format
    Annotated,

    /// HTML-like tags for styling
    HtmlTags,
}

/// Output format for captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputFormat {
    /// Markdown format with fenced code blocks
    #[default]
    Markdown,

    /// Plain text
    PlainText,

    /// HTML with CSS styling
    Html,

    /// JSON for programmatic access
    Json,
}

/// Configuration for TUI capture operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// Terminal width in characters
    pub width: u16,

    /// Terminal height in characters
    pub height: u16,

    /// How to render styles
    pub style_rendering: StyleRendering,

    /// Output format
    pub output_format: OutputFormat,

    /// Include timestamps in output
    pub include_timestamps: bool,

    /// Include frame numbers
    pub include_frame_numbers: bool,

    /// Include action details
    pub include_actions: bool,

    /// Maximum actions to show before each frame
    pub max_actions_per_frame: usize,

    /// Include metadata
    pub include_metadata: bool,

    /// Show box drawing characters properly
    pub preserve_box_drawing: bool,

    /// Tab width for indentation
    pub tab_width: u8,

    /// Trim trailing whitespace from lines
    pub trim_trailing_whitespace: bool,

    /// Include session summary
    pub include_summary: bool,

    /// Maximum line width (0 = no limit)
    pub max_line_width: usize,

    /// Add ruler lines between frames
    pub add_frame_separators: bool,

    /// Title for the capture
    pub title: Option<String>,

    /// Description for the capture
    pub description: Option<String>,

    /// Show only frames with labels (skip intermediate frames)
    pub labeled_frames_only: bool,

    /// Show cursor position
    pub show_cursor: bool,

    /// Cursor character when visible
    pub cursor_char: char,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
            style_rendering: StyleRendering::Plain,
            output_format: OutputFormat::Markdown,
            include_timestamps: true,
            include_frame_numbers: true,
            include_actions: true,
            max_actions_per_frame: 10,
            include_metadata: true,
            preserve_box_drawing: true,
            tab_width: 4,
            trim_trailing_whitespace: true,
            include_summary: true,
            max_line_width: 0,
            add_frame_separators: true,
            title: None,
            description: None,
            labeled_frames_only: false,
            show_cursor: true,
            cursor_char: '█',
        }
    }
}

impl CaptureConfig {
    /// Create a new configuration with specified dimensions.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            ..Default::default()
        }
    }

    /// Create a minimal configuration (no timestamps, no actions).
    pub fn minimal(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            include_timestamps: false,
            include_frame_numbers: false,
            include_actions: false,
            include_metadata: false,
            include_summary: false,
            add_frame_separators: false,
            ..Default::default()
        }
    }

    /// Create a verbose configuration (all features enabled).
    pub fn verbose(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            include_timestamps: true,
            include_frame_numbers: true,
            include_actions: true,
            max_actions_per_frame: 100,
            include_metadata: true,
            include_summary: true,
            add_frame_separators: true,
            ..Default::default()
        }
    }

    /// Builder: set width.
    pub fn with_width(mut self, width: u16) -> Self {
        self.width = width;
        self
    }

    /// Builder: set height.
    pub fn with_height(mut self, height: u16) -> Self {
        self.height = height;
        self
    }

    /// Builder: set dimensions.
    pub fn with_size(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Builder: set style rendering mode.
    pub fn with_style_rendering(mut self, rendering: StyleRendering) -> Self {
        self.style_rendering = rendering;
        self
    }

    /// Builder: set output format.
    pub fn with_output_format(mut self, format: OutputFormat) -> Self {
        self.output_format = format;
        self
    }

    /// Builder: enable/disable timestamps.
    pub fn with_timestamps(mut self, include: bool) -> Self {
        self.include_timestamps = include;
        self
    }

    /// Builder: enable/disable frame numbers.
    pub fn with_frame_numbers(mut self, include: bool) -> Self {
        self.include_frame_numbers = include;
        self
    }

    /// Builder: enable/disable actions.
    pub fn with_actions(mut self, include: bool) -> Self {
        self.include_actions = include;
        self
    }

    /// Builder: set maximum actions per frame.
    pub fn with_max_actions(mut self, max: usize) -> Self {
        self.max_actions_per_frame = max;
        self
    }

    /// Builder: enable/disable metadata.
    pub fn with_metadata(mut self, include: bool) -> Self {
        self.include_metadata = include;
        self
    }

    /// Builder: enable/disable summary.
    pub fn with_summary(mut self, include: bool) -> Self {
        self.include_summary = include;
        self
    }

    /// Builder: enable/disable frame separators.
    pub fn with_separators(mut self, include: bool) -> Self {
        self.add_frame_separators = include;
        self
    }

    /// Builder: set title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Builder: set description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Builder: show only labeled frames.
    pub fn labeled_only(mut self) -> Self {
        self.labeled_frames_only = true;
        self
    }

    /// Builder: show cursor.
    pub fn with_cursor(mut self, show: bool) -> Self {
        self.show_cursor = show;
        self
    }

    /// Builder: set cursor character.
    pub fn with_cursor_char(mut self, ch: char) -> Self {
        self.cursor_char = ch;
        self
    }

    /// Builder: trim trailing whitespace.
    pub fn trim_whitespace(mut self, trim: bool) -> Self {
        self.trim_trailing_whitespace = trim;
        self
    }

    /// Get the total number of cells.
    pub fn cell_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 {
            return Err("Width must be greater than 0".to_string());
        }
        if self.height == 0 {
            return Err("Height must be greater than 0".to_string());
        }
        if self.width > 1000 {
            return Err("Width exceeds maximum (1000)".to_string());
        }
        if self.height > 1000 {
            return Err("Height exceeds maximum (1000)".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CaptureConfig::default();
        assert_eq!(config.width, 80);
        assert_eq!(config.height, 24);
        assert!(config.include_timestamps);
    }

    #[test]
    fn test_minimal_config() {
        let config = CaptureConfig::minimal(100, 30);
        assert_eq!(config.width, 100);
        assert_eq!(config.height, 30);
        assert!(!config.include_timestamps);
        assert!(!config.include_actions);
    }

    #[test]
    fn test_builder_pattern() {
        let config = CaptureConfig::new(120, 40)
            .with_title("Test Capture")
            .with_timestamps(false)
            .with_style_rendering(StyleRendering::Ansi);

        assert_eq!(config.width, 120);
        assert_eq!(config.title, Some("Test Capture".to_string()));
        assert!(!config.include_timestamps);
        assert_eq!(config.style_rendering, StyleRendering::Ansi);
    }

    #[test]
    fn test_validation() {
        assert!(CaptureConfig::new(80, 24).validate().is_ok());
        assert!(CaptureConfig::new(0, 24).validate().is_err());
        assert!(CaptureConfig::new(80, 0).validate().is_err());
        assert!(CaptureConfig::new(2000, 24).validate().is_err());
    }
}
