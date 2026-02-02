//! TUI capture integration for debugging.
//!
//! This module provides integration with `cortex-tui-capture` to enable
//! frame-by-frame debugging of the TUI. When enabled via the `CORTEX_TUI_CAPTURE=1`
//! environment variable, all renders are captured and exported to markdown
//! files for analysis.
//!
//! ## Usage
//!
//! ```bash
//! # Enable capture mode
//! CORTEX_TUI_CAPTURE=1 cargo run --bin cortex
//!
//! # With custom output directory
//! CORTEX_TUI_CAPTURE=1 CORTEX_TUI_CAPTURE_DIR=/tmp/captures cargo run --bin cortex
//! ```
//!
//! ## Output
//!
//! Captures are written to `~/.cortex/tui-captures/` by default, with:
//! - A markdown file containing ASCII art frames with timestamps
//! - A JSON file with structured session data
//!
//! This is useful for:
//! - Debugging rendering issues
//! - Understanding TUI state at specific moments
//! - Letting AI agents "see" what the TUI rendered

use cortex_tui_capture::{
    ActionType, BufferSnapshot, CaptureManager as InnerCaptureManager, CaptureResult,
    integration::is_capture_enabled,
};
use ratatui::buffer::Buffer as RatatuiBuffer;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::{debug, info};

/// Minimum interval between frame captures when capture_all is enabled (1 second)
const MIN_CAPTURE_INTERVAL_SECS: u64 = 1;

/// Static frame counter for unique frame labeling
static FRAME_COUNTER: AtomicU64 = AtomicU64::new(0);

/// TUI capture wrapper that integrates with the event loop.
///
/// This wrapper handles initialization, frame capture, and export.
/// It only activates when `CORTEX_TUI_CAPTURE=1` is set.
pub struct TuiCapture {
    /// Inner capture manager (None if capture is disabled)
    inner: Option<InnerCaptureManager>,
    /// Whether to capture every frame (vs. only labeled events)
    capture_all: bool,
    /// Current view for labeling (public for event loop access)
    pub current_view: String,
    /// Whether an autocomplete popup is visible
    autocomplete_visible: bool,
    /// Whether a modal is open
    modal_open: bool,
    /// Last captured event description
    last_event: String,
    /// Last time a frame was captured (for rate limiting capture_all mode)
    last_capture_time: Option<Instant>,
}

impl TuiCapture {
    /// Create a new TUI capture wrapper.
    ///
    /// Capture is only enabled if `CORTEX_TUI_CAPTURE=1` is set.
    pub fn new(width: u16, height: u16) -> Self {
        let inner = if is_capture_enabled() {
            info!("TUI capture enabled - frames will be recorded to ~/.cortex/tui-captures/");
            let mut manager = InnerCaptureManager::new("cortex-session", width, height);
            manager.add_metadata("app", "cortex-tui");
            manager.add_metadata("version", env!("CARGO_PKG_VERSION"));
            Some(manager)
        } else {
            None
        };

        Self {
            inner,
            capture_all: std::env::var("CORTEX_TUI_CAPTURE_ALL")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            current_view: "Session".to_string(),
            autocomplete_visible: false,
            modal_open: false,
            last_event: String::new(),
            last_capture_time: None,
        }
    }

    /// Check if capture is enabled.
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Record a key press event.
    pub fn record_key(&mut self, key: &str) {
        if let Some(ref mut manager) = self.inner {
            manager.record_key_press(key);
            self.last_event = format!("Key: {}", key);
        }
    }

    /// Record a command execution.
    pub fn record_command(&mut self, cmd: &str) {
        if let Some(ref mut manager) = self.inner {
            manager.record_command(cmd);
            self.last_event = format!("Command: {}", cmd);
        }
    }

    /// Record a view change.
    pub fn record_view_change(&mut self, from: &str, to: &str) {
        if let Some(ref mut manager) = self.inner {
            manager.record_action(ActionType::ViewChange {
                from: from.to_string(),
                to: to.to_string(),
            });
            self.current_view = to.to_string();
            self.last_event = format!("View: {} -> {}", from, to);
        }
    }

    /// Record autocomplete popup visibility change.
    pub fn record_autocomplete(&mut self, visible: bool, query: &str) {
        if let Some(ref mut manager) = self.inner
            && visible != self.autocomplete_visible
        {
            self.autocomplete_visible = visible;
            if visible {
                manager.record_action(ActionType::Custom {
                    name: "autocomplete_show".to_string(),
                    data: format!("query: {}", query),
                });
                self.last_event = format!("Autocomplete: show ({})", query);
            } else {
                manager.record_action(ActionType::Custom {
                    name: "autocomplete_hide".to_string(),
                    data: String::new(),
                });
                self.last_event = "Autocomplete: hide".to_string();
            }
        }
    }

    /// Record modal open/close.
    pub fn record_modal(&mut self, open: bool, name: &str) {
        if let Some(ref mut manager) = self.inner
            && (open != self.modal_open || open)
        {
            self.modal_open = open;
            if open {
                manager.record_action(ActionType::ModalOpened(name.to_string()));
                self.last_event = format!("Modal: open ({})", name);
            } else {
                manager.record_action(ActionType::ModalClosed(name.to_string()));
                self.last_event = format!("Modal: close ({})", name);
            }
        }
    }

    /// Record streaming start/stop.
    pub fn record_streaming(&mut self, started: bool, tool: Option<&str>) {
        if let Some(ref mut manager) = self.inner {
            if started {
                manager.record_action(ActionType::StreamingStart {
                    tool: tool.map(String::from),
                });
                self.last_event = format!("Streaming: start (tool: {:?})", tool);
            } else {
                manager.record_action(ActionType::StreamingEnd);
                self.last_event = "Streaming: end".to_string();
            }
        }
    }

    /// Record a tool call.
    pub fn record_tool_call(&mut self, name: &str, args: &str) {
        if let Some(ref mut manager) = self.inner {
            manager.record_action(ActionType::ToolCall {
                name: name.to_string(),
                args: args.to_string(),
            });
            self.last_event = format!("Tool: {} called", name);
        }
    }

    /// Record an error.
    pub fn record_error(&mut self, msg: &str) {
        if let Some(ref mut manager) = self.inner {
            manager.record_error(msg);
            self.last_event = format!("Error: {}", msg);
        }
    }

    /// Add a debug marker.
    pub fn marker(&mut self, message: &str) {
        if let Some(ref mut manager) = self.inner {
            manager.marker(message);
            self.last_event = format!("Marker: {}", message);
        }
    }

    /// Capture a frame from the terminal buffer.
    ///
    /// This should be called after each render. It captures the current
    /// buffer state as ASCII art.
    ///
    /// When `capture_all` is enabled, frames are rate-limited to at most
    /// one per second to avoid excessive logging and storage.
    pub fn capture_frame(&mut self, buffer: &RatatuiBuffer, label: Option<&str>) {
        if let Some(ref mut manager) = self.inner {
            let snapshot = BufferSnapshot::from_ratatui_buffer(buffer);

            // Generate a label if not provided
            let frame_label = if let Some(l) = label {
                l.to_string()
            } else if !self.last_event.is_empty() {
                let event = std::mem::take(&mut self.last_event);
                format!("{} - {}", self.current_view, event)
            } else if self.capture_all {
                // Rate limit capture_all frames to max 1 per second
                let now = Instant::now();
                if let Some(last_time) = self.last_capture_time
                    && now.duration_since(last_time).as_secs() < MIN_CAPTURE_INTERVAL_SECS
                {
                    // Skip this frame - not enough time has passed
                    return;
                }
                self.last_capture_time = Some(now);
                let frame_num = FRAME_COUNTER.fetch_add(1, Ordering::SeqCst);
                format!("{} - frame {}", self.current_view, frame_num)
            } else {
                // Skip unlabeled frames unless capture_all is set
                return;
            };

            manager.capture_frame(&frame_label, &snapshot);
            debug!("Captured frame: {}", frame_label);
        }
    }

    /// Capture a frame with automatic labeling based on current state.
    ///
    /// This is a convenience method that uses the current view and any
    /// pending event as the label.
    pub fn capture_auto(&mut self, buffer: &RatatuiBuffer) {
        self.capture_frame(buffer, None);
    }

    /// Export the captured session to files.
    ///
    /// This should be called when the TUI exits to write all captured
    /// frames to disk.
    pub async fn export(self) -> CaptureResult<Option<String>> {
        if let Some(manager) = self.inner {
            let result = manager.export().await?;
            info!("TUI capture exported: {}", result.markdown_path);
            Ok(Some(result.markdown_path))
        } else {
            Ok(None)
        }
    }

    /// Update terminal dimensions (e.g., on resize).
    pub fn update_dimensions(&mut self, width: u16, height: u16) {
        if let Some(ref mut manager) = self.inner {
            manager.record_action(ActionType::Resize { width, height });
        }
    }
}

impl Default for TuiCapture {
    fn default() -> Self {
        // Default to 80x24, will be updated on first render
        Self::new(80, 24)
    }
}

/// Convenience function to check if capture is enabled.
#[inline]
pub fn capture_enabled() -> bool {
    is_capture_enabled()
}
