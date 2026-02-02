//! Integration utilities for cortex-tui.
//!
//! This module provides helpers for integrating the capture system with
//! the Cortex TUI application, enabling automatic session recording and
//! debugging output.

use crate::capture::BufferSnapshot;
use crate::config::CaptureConfig;
use crate::recorder::SessionRecorder;
use crate::types::{ActionType, CaptureResult};
use std::path::PathBuf;
use tokio::fs;
use tracing::info;

/// Default output directory for TUI captures.
pub const DEFAULT_CAPTURE_DIR: &str = ".cortex/tui-captures";

/// Environment variable to enable TUI capture.
pub const CAPTURE_ENV_VAR: &str = "CORTEX_TUI_CAPTURE";

/// Environment variable for capture output directory.
pub const CAPTURE_DIR_ENV_VAR: &str = "CORTEX_TUI_CAPTURE_DIR";

/// Check if TUI capture is enabled via environment variable.
pub fn is_capture_enabled() -> bool {
    std::env::var(CAPTURE_ENV_VAR)
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

/// Get the capture output directory.
pub fn capture_output_dir() -> PathBuf {
    std::env::var(CAPTURE_DIR_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .map(|h| h.join(DEFAULT_CAPTURE_DIR))
                .unwrap_or_else(|| PathBuf::from(DEFAULT_CAPTURE_DIR))
        })
}

/// TUI capture manager for automatic session recording.
///
/// This manager can be embedded in the TUI application to automatically
/// capture frames and actions when debugging is enabled.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui_capture::integration::CaptureManager;
///
/// let mut manager = CaptureManager::new_if_enabled(80, 24);
///
/// // During TUI operation
/// if let Some(m) = &mut manager {
///     m.record_key_press("Enter");
///     m.capture_frame("After Enter", &buffer);
/// }
///
/// // At shutdown
/// if let Some(m) = manager {
///     m.export().await?;
/// }
/// ```
pub struct CaptureManager {
    /// Session recorder
    recorder: SessionRecorder,

    /// Output directory
    output_dir: PathBuf,

    /// Auto-capture on every Nth tick (0 = disabled)
    auto_capture_interval: u64,

    /// Tick counter
    tick_counter: u64,

    /// Whether to export JSON alongside markdown
    export_json: bool,

    /// Whether to capture unlabeled frames
    capture_all_frames: bool,
}

impl CaptureManager {
    /// Create a new capture manager.
    pub fn new(name: impl Into<String>, width: u16, height: u16) -> Self {
        let output_dir = capture_output_dir();
        let recorder = SessionRecorder::new(name, width, height);

        Self {
            recorder,
            output_dir,
            auto_capture_interval: 0,
            tick_counter: 0,
            export_json: true,
            capture_all_frames: false,
        }
    }

    /// Create a capture manager only if capture is enabled via environment.
    pub fn new_if_enabled(name: impl Into<String>, width: u16, height: u16) -> Option<Self> {
        if is_capture_enabled() {
            info!("TUI capture enabled");
            Some(Self::new(name, width, height))
        } else {
            None
        }
    }

    /// Create with a specific configuration.
    pub fn with_config(name: impl Into<String>, config: CaptureConfig) -> Self {
        let output_dir = capture_output_dir();
        let recorder = SessionRecorder::with_config(name, config);

        Self {
            recorder,
            output_dir,
            auto_capture_interval: 0,
            tick_counter: 0,
            export_json: true,
            capture_all_frames: false,
        }
    }

    /// Set the output directory.
    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = dir.into();
        self
    }

    /// Enable auto-capture every N ticks.
    pub fn with_auto_capture(mut self, interval: u64) -> Self {
        self.auto_capture_interval = interval;
        self
    }

    /// Set whether to export JSON.
    pub fn with_json_export(mut self, export: bool) -> Self {
        self.export_json = export;
        self
    }

    /// Set whether to capture all frames.
    pub fn capture_all(mut self, capture: bool) -> Self {
        self.capture_all_frames = capture;
        self
    }

    /// Add session metadata.
    pub fn add_metadata(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.recorder.add_metadata(key, value);
    }

    /// Set session description.
    pub fn set_description(&mut self, description: impl Into<String>) {
        self.recorder.set_description(description);
    }

    /// Record a key press.
    pub fn record_key_press(&mut self, key: impl Into<String>) {
        self.recorder.key_press(key);
    }

    /// Record a mouse click.
    pub fn record_mouse_click(&mut self, x: u16, y: u16, button: impl Into<String>) {
        self.recorder.mouse_click(x, y, button);
    }

    /// Record a command.
    pub fn record_command(&mut self, cmd: impl Into<String>) {
        self.recorder.command(cmd);
    }

    /// Record an error.
    pub fn record_error(&mut self, msg: impl Into<String>) {
        self.recorder.error(msg);
    }

    /// Record a custom action.
    pub fn record_action(&mut self, action_type: ActionType) {
        self.recorder.record(action_type);
    }

    /// Add a marker for debugging.
    pub fn marker(&mut self, message: impl Into<String>) {
        self.recorder.marker(message);
    }

    /// Capture a frame with a label.
    pub fn capture_frame(&mut self, label: impl Into<String>, buffer: &BufferSnapshot) {
        self.recorder.record_frame(label, buffer);
    }

    /// Capture a frame without a label.
    pub fn capture_frame_unlabeled(&mut self, buffer: &BufferSnapshot) {
        if self.capture_all_frames {
            self.recorder.record_frame_unlabeled(buffer);
        }
    }

    /// Process a tick and optionally auto-capture.
    pub fn tick(&mut self, buffer: &BufferSnapshot) {
        self.tick_counter += 1;

        if self.auto_capture_interval > 0
            && self.tick_counter.is_multiple_of(self.auto_capture_interval)
        {
            let label = format!("Auto-capture at tick {}", self.tick_counter);
            self.recorder.record_frame(label, buffer);
        }
    }

    /// Get the current statistics.
    pub fn stats(&self) -> &crate::recorder::SessionStats {
        self.recorder.stats()
    }

    /// Get the elapsed time.
    pub fn elapsed(&self) -> std::time::Duration {
        self.recorder.elapsed()
    }

    /// Get the session ID.
    pub fn session_id(&self) -> uuid::Uuid {
        self.recorder.session_id()
    }

    /// Export the session to the output directory.
    pub async fn export(self) -> CaptureResult<ExportResult> {
        // Create output directory
        fs::create_dir_all(&self.output_dir).await?;

        // Export markdown
        let md_path = self.recorder.export_markdown(&self.output_dir).await?;
        info!("Exported TUI capture to: {}", md_path);

        // Export JSON if enabled
        let json_path = if self.export_json {
            Some(self.recorder.export_json(&self.output_dir).await?)
        } else {
            None
        };

        Ok(ExportResult {
            markdown_path: md_path,
            json_path,
            stats_summary: self.recorder.stats().summary(),
        })
    }

    /// Get the recorder for direct access.
    pub fn recorder(&self) -> &SessionRecorder {
        &self.recorder
    }

    /// Get mutable recorder for direct access.
    pub fn recorder_mut(&mut self) -> &mut SessionRecorder {
        &mut self.recorder
    }
}

/// Result of exporting a capture session.
#[derive(Debug, Clone)]
pub struct ExportResult {
    /// Path to the markdown file.
    pub markdown_path: String,

    /// Path to the JSON file (if exported).
    pub json_path: Option<String>,

    /// Summary of session statistics.
    pub stats_summary: String,
}

impl std::fmt::Display for ExportResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Capture exported to: {}", self.markdown_path)?;
        if let Some(json) = &self.json_path {
            write!(f, " (JSON: {})", json)?;
        }
        write!(f, " - {}", self.stats_summary)
    }
}

/// Quick capture helper for one-off frame captures.
///
/// This is useful for capturing a single state without setting up
/// a full session recorder.
pub struct QuickCapture {
    config: CaptureConfig,
    output_dir: PathBuf,
}

impl QuickCapture {
    /// Create a new quick capture helper.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            config: CaptureConfig::new(width, height),
            output_dir: capture_output_dir(),
        }
    }

    /// Set the output directory.
    pub fn output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = dir.into();
        self
    }

    /// Set the title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.config.title = Some(title.into());
        self
    }

    /// Capture a buffer and save to a file.
    pub async fn capture(&self, label: &str, buffer: &BufferSnapshot) -> CaptureResult<String> {
        fs::create_dir_all(&self.output_dir).await?;

        let ascii = buffer.to_ascii(&self.config);
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let filename = format!(
            "{}_{}.md",
            label.replace(' ', "_").to_lowercase(),
            timestamp
        );
        let filepath = self.output_dir.join(&filename);

        let mut content = String::new();
        content.push_str(&format!("# {}\n\n", label));
        content.push_str(&format!("**Captured at:** {}\n\n", chrono::Utc::now()));
        content.push_str("```\n");
        content.push_str(&ascii);
        content.push_str("\n```\n");

        fs::write(&filepath, content).await?;

        Ok(filepath.to_string_lossy().to_string())
    }
}

/// Helpers for common TUI action recording.
pub mod actions {
    use super::*;

    /// Create an action for view change.
    pub fn view_change(from: impl Into<String>, to: impl Into<String>) -> ActionType {
        ActionType::ViewChange {
            from: from.into(),
            to: to.into(),
        }
    }

    /// Create an action for focus change.
    pub fn focus_change(target: impl Into<String>) -> ActionType {
        ActionType::FocusChange(target.into())
    }

    /// Create an action for state update.
    pub fn state_update(field: impl Into<String>, value: impl Into<String>) -> ActionType {
        ActionType::StateUpdate {
            field: field.into(),
            value: value.into(),
        }
    }

    /// Create an action for streaming start.
    pub fn streaming_start(tool: Option<String>) -> ActionType {
        ActionType::StreamingStart { tool }
    }

    /// Create an action for streaming end.
    pub fn streaming_end() -> ActionType {
        ActionType::StreamingEnd
    }

    /// Create an action for tool call.
    pub fn tool_call(name: impl Into<String>, args: impl Into<String>) -> ActionType {
        ActionType::ToolCall {
            name: name.into(),
            args: args.into(),
        }
    }

    /// Create an action for tool result.
    pub fn tool_result(name: impl Into<String>, success: bool) -> ActionType {
        ActionType::ToolResult {
            name: name.into(),
            success,
        }
    }

    /// Create an action for message added.
    pub fn message_added(role: impl Into<String>, preview: impl Into<String>) -> ActionType {
        ActionType::MessageAdded {
            role: role.into(),
            preview: preview.into(),
        }
    }

    /// Create an action for modal opened.
    pub fn modal_opened(name: impl Into<String>) -> ActionType {
        ActionType::ModalOpened(name.into())
    }

    /// Create an action for modal closed.
    pub fn modal_closed(name: impl Into<String>) -> ActionType {
        ActionType::ModalClosed(name.into())
    }

    /// Create an action for resize.
    pub fn resize(width: u16, height: u16) -> ActionType {
        ActionType::Resize { width, height }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_manager_creation() {
        let manager = CaptureManager::new("Test", 80, 24);
        assert!(manager.recorder.name() == "Test");
    }

    #[test]
    fn test_capture_enabled_check() {
        // By default, capture should be disabled
        assert!(!is_capture_enabled());
    }

    #[test]
    fn test_action_helpers() {
        let action = actions::view_change("Session", "Help");
        match action {
            ActionType::ViewChange { from, to } => {
                assert_eq!(from, "Session");
                assert_eq!(to, "Help");
            }
            _ => panic!("Wrong action type"),
        }
    }

    #[test]
    fn test_quick_capture() {
        let capture = QuickCapture::new(80, 24).title("Test Capture");
        assert_eq!(capture.config.title, Some("Test Capture".to_string()));
    }
}
