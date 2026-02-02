//! Core types for TUI capture and recording.
//!
//! This module defines the fundamental types used throughout the capture system,
//! including actions, events, frames, and error types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

/// Error types for TUI capture operations.
#[derive(Error, Debug)]
pub enum CaptureError {
    /// Failed to render frame
    #[error("Failed to render frame: {0}")]
    RenderError(String),

    /// Failed to export capture
    #[error("Failed to export capture: {0}")]
    ExportError(String),

    /// IO error during file operations
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    /// Session not found
    #[error("Session not found: {0}")]
    SessionNotFound(String),
}

/// Result type for capture operations.
pub type CaptureResult<T> = Result<T, CaptureError>;

/// Types of actions that can occur in a TUI session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ActionType {
    /// A key was pressed
    KeyPress(String),

    /// A mouse click occurred at (x, y)
    MouseClick { x: u16, y: u16, button: String },

    /// Mouse scroll event
    MouseScroll { direction: String, delta: i32 },

    /// Mouse move event
    MouseMove { x: u16, y: u16 },

    /// Text was pasted
    Paste(String),

    /// Terminal was resized
    Resize { width: u16, height: u16 },

    /// A command was executed
    Command(String),

    /// Focus changed to a new target
    FocusChange(String),

    /// View changed
    ViewChange { from: String, to: String },

    /// State updated
    StateUpdate { field: String, value: String },

    /// Streaming started
    StreamingStart { tool: Option<String> },

    /// Streaming ended
    StreamingEnd,

    /// Tool call initiated
    ToolCall { name: String, args: String },

    /// Tool result received
    ToolResult { name: String, success: bool },

    /// Message added to chat
    MessageAdded { role: String, preview: String },

    /// Modal opened
    ModalOpened(String),

    /// Modal closed
    ModalClosed(String),

    /// Error occurred
    Error(String),

    /// Custom action with arbitrary data
    Custom { name: String, data: String },
}

impl ActionType {
    /// Get a human-readable description of the action.
    pub fn description(&self) -> String {
        match self {
            Self::KeyPress(key) => format!("Key press: {}", key),
            Self::MouseClick { x, y, button } => {
                format!("Mouse {} click at ({}, {})", button, x, y)
            }
            Self::MouseScroll { direction, delta } => {
                format!("Mouse scroll {} by {}", direction, delta)
            }
            Self::MouseMove { x, y } => format!("Mouse move to ({}, {})", x, y),
            Self::Paste(text) => {
                let preview = if text.len() > 50 {
                    format!("{}...", &text[..50])
                } else {
                    text.clone()
                };
                format!("Paste: \"{}\"", preview)
            }
            Self::Resize { width, height } => format!("Resize to {}x{}", width, height),
            Self::Command(cmd) => format!("Command: {}", cmd),
            Self::FocusChange(target) => format!("Focus changed to: {}", target),
            Self::ViewChange { from, to } => format!("View changed: {} → {}", from, to),
            Self::StateUpdate { field, value } => format!("State update: {} = {}", field, value),
            Self::StreamingStart { tool } => {
                if let Some(t) = tool {
                    format!("Streaming started (tool: {})", t)
                } else {
                    "Streaming started".to_string()
                }
            }
            Self::StreamingEnd => "Streaming ended".to_string(),
            Self::ToolCall { name, args } => {
                let args_preview = if args.len() > 100 {
                    format!("{}...", &args[..100])
                } else {
                    args.clone()
                };
                format!("Tool call: {}({})", name, args_preview)
            }
            Self::ToolResult { name, success } => {
                let status = if *success { "✓" } else { "✗" };
                format!("Tool result: {} {}", name, status)
            }
            Self::MessageAdded { role, preview } => format!("[{}] {}", role, preview),
            Self::ModalOpened(name) => format!("Modal opened: {}", name),
            Self::ModalClosed(name) => format!("Modal closed: {}", name),
            Self::Error(msg) => format!("Error: {}", msg),
            Self::Custom { name, data } => format!("Custom({}): {}", name, data),
        }
    }

    /// Get the category of this action for grouping.
    pub fn category(&self) -> &'static str {
        match self {
            Self::KeyPress(_) | Self::Paste(_) => "Input",
            Self::MouseClick { .. } | Self::MouseScroll { .. } | Self::MouseMove { .. } => "Mouse",
            Self::Resize { .. } => "Terminal",
            Self::Command(_) => "Command",
            Self::FocusChange(_) | Self::ViewChange { .. } => "Navigation",
            Self::StateUpdate { .. } => "State",
            Self::StreamingStart { .. }
            | Self::StreamingEnd
            | Self::ToolCall { .. }
            | Self::ToolResult { .. } => "Streaming",
            Self::MessageAdded { .. } => "Chat",
            Self::ModalOpened(_) | Self::ModalClosed(_) => "Modal",
            Self::Error(_) => "Error",
            Self::Custom { .. } => "Custom",
        }
    }

    /// Get an icon for the action category.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::KeyPress(_) => "[KEY]",
            Self::Paste(_) => "[PASTE]",
            Self::MouseClick { .. } => "[CLICK]",
            Self::MouseScroll { .. } => "[SCROLL]",
            Self::MouseMove { .. } => "[MOVE]",
            Self::Resize { .. } => "[RESIZE]",
            Self::Command(_) => "[CMD]",
            Self::FocusChange(_) => "[FOCUS]",
            Self::ViewChange { .. } => "[VIEW]",
            Self::StateUpdate { .. } => "[STATE]",
            Self::StreamingStart { .. } => "[>]",
            Self::StreamingEnd => "[x]",
            Self::ToolCall { .. } => "[TOOL]",
            Self::ToolResult { .. } => "[OK]",
            Self::MessageAdded { .. } => "[MSG]",
            Self::ModalOpened(_) => "[OPEN]",
            Self::ModalClosed(_) => "[CLOSE]",
            Self::Error(_) => "[ERR]",
            Self::Custom { .. } => "[*]",
        }
    }
}

/// A single TUI action with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiAction {
    /// Unique identifier for this action
    pub id: Uuid,

    /// Type of action
    pub action_type: ActionType,

    /// When the action occurred
    pub timestamp: DateTime<Utc>,

    /// Duration of the action (if applicable)
    pub duration: Option<Duration>,

    /// Additional context/notes
    pub context: Option<String>,

    /// Sequence number in the session
    pub sequence: u64,
}

impl TuiAction {
    /// Create a new TUI action with the current timestamp.
    pub fn new(action_type: ActionType) -> Self {
        Self {
            id: Uuid::new_v4(),
            action_type,
            timestamp: Utc::now(),
            duration: None,
            context: None,
            sequence: 0,
        }
    }

    /// Create with a specific timestamp.
    pub fn with_timestamp(action_type: ActionType, timestamp: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            action_type,
            timestamp,
            duration: None,
            context: None,
            sequence: 0,
        }
    }

    /// Add duration to the action.
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Add context to the action.
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Set the sequence number.
    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = sequence;
        self
    }

    /// Format the timestamp as a string.
    pub fn timestamp_str(&self) -> String {
        self.timestamp.format("%H:%M:%S%.3f").to_string()
    }

    /// Get the elapsed time since a reference point.
    pub fn elapsed_since(&self, start: DateTime<Utc>) -> Duration {
        (self.timestamp - start).to_std().unwrap_or(Duration::ZERO)
    }
}

/// TUI events that can be captured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TuiEvent {
    /// Frame was rendered
    FrameRendered {
        frame_number: u64,
        timestamp: DateTime<Utc>,
    },

    /// Action occurred
    Action(TuiAction),

    /// Session started
    SessionStarted {
        session_id: Uuid,
        timestamp: DateTime<Utc>,
        width: u16,
        height: u16,
    },

    /// Session ended
    SessionEnded {
        session_id: Uuid,
        timestamp: DateTime<Utc>,
        total_frames: u64,
        total_actions: u64,
    },

    /// Snapshot was taken
    SnapshotTaken {
        snapshot_id: Uuid,
        timestamp: DateTime<Utc>,
        label: String,
    },

    /// Marker event for debugging
    Marker {
        timestamp: DateTime<Utc>,
        message: String,
    },
}

impl TuiEvent {
    /// Get the timestamp of this event.
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::FrameRendered { timestamp, .. } => *timestamp,
            Self::Action(action) => action.timestamp,
            Self::SessionStarted { timestamp, .. } => *timestamp,
            Self::SessionEnded { timestamp, .. } => *timestamp,
            Self::SnapshotTaken { timestamp, .. } => *timestamp,
            Self::Marker { timestamp, .. } => *timestamp,
        }
    }

    /// Get a description of this event.
    pub fn description(&self) -> String {
        match self {
            Self::FrameRendered { frame_number, .. } => format!("Frame #{} rendered", frame_number),
            Self::Action(action) => action.action_type.description(),
            Self::SessionStarted { width, height, .. } => {
                format!("Session started ({}x{})", width, height)
            }
            Self::SessionEnded {
                total_frames,
                total_actions,
                ..
            } => {
                format!(
                    "Session ended ({} frames, {} actions)",
                    total_frames, total_actions
                )
            }
            Self::SnapshotTaken { label, .. } => format!("Snapshot: {}", label),
            Self::Marker { message, .. } => format!("Marker: {}", message),
        }
    }
}

/// A captured frame with its ASCII representation and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedFrame {
    /// Unique identifier
    pub id: Uuid,

    /// Frame number in the session
    pub frame_number: u64,

    /// When the frame was captured
    pub timestamp: DateTime<Utc>,

    /// ASCII representation of the frame
    pub ascii_content: String,

    /// Label/description for this frame
    pub label: Option<String>,

    /// Width of the frame in characters
    pub width: u16,

    /// Height of the frame in characters
    pub height: u16,

    /// Actions that occurred before this frame
    pub preceding_actions: Vec<TuiAction>,

    /// Additional metadata
    pub metadata: std::collections::HashMap<String, String>,
}

impl CapturedFrame {
    /// Create a new captured frame.
    pub fn new(frame_number: u64, ascii_content: String, width: u16, height: u16) -> Self {
        Self {
            id: Uuid::new_v4(),
            frame_number,
            timestamp: Utc::now(),
            ascii_content,
            label: None,
            width,
            height,
            preceding_actions: Vec::new(),
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Add a label to the frame.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Add preceding actions.
    pub fn with_actions(mut self, actions: Vec<TuiAction>) -> Self {
        self.preceding_actions = actions;
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Get the line count of the ASCII content.
    pub fn line_count(&self) -> usize {
        self.ascii_content.lines().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_type_description() {
        let action = ActionType::KeyPress("Enter".to_string());
        assert_eq!(action.description(), "Key press: Enter");

        let action = ActionType::ToolCall {
            name: "read_file".to_string(),
            args: "{\"path\": \"/test\"}".to_string(),
        };
        assert!(action.description().contains("read_file"));
    }

    #[test]
    fn test_tui_action_creation() {
        let action = TuiAction::new(ActionType::KeyPress("A".to_string()))
            .with_context("Test context")
            .with_sequence(42);

        assert_eq!(action.sequence, 42);
        assert!(action.context.is_some());
    }

    #[test]
    fn test_captured_frame() {
        let frame = CapturedFrame::new(1, "Hello\nWorld".to_string(), 80, 24)
            .with_label("Test frame")
            .with_metadata("key", "value");

        assert_eq!(frame.frame_number, 1);
        assert_eq!(frame.label, Some("Test frame".to_string()));
        assert_eq!(frame.metadata.get("key"), Some(&"value".to_string()));
        assert_eq!(frame.line_count(), 2);
    }
}
