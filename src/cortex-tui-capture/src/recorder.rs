//! Session recorder for capturing TUI sessions over time.
//!
//! This module provides a comprehensive session recording system that tracks
//! all actions, state changes, and frames throughout a TUI session.

use crate::capture::{BufferSnapshot, FrameCapture};
use crate::config::CaptureConfig;
use crate::types::{ActionType, CaptureError, CaptureResult, CapturedFrame, TuiAction, TuiEvent};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::fs;
use uuid::Uuid;

/// A complete session report containing all recorded data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReport {
    /// Unique session ID
    pub session_id: Uuid,

    /// Session name/title
    pub name: String,

    /// Session description
    pub description: Option<String>,

    /// When the session started
    pub started_at: DateTime<Utc>,

    /// When the session ended
    pub ended_at: Option<DateTime<Utc>>,

    /// Terminal dimensions
    pub terminal_size: (u16, u16),

    /// All events in chronological order
    pub events: Vec<TuiEvent>,

    /// All captured frames
    pub frames: Vec<CapturedFrame>,

    /// All recorded actions
    pub actions: Vec<TuiAction>,

    /// Session metadata
    pub metadata: HashMap<String, String>,

    /// Statistics
    pub stats: SessionStats,
}

/// Session statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionStats {
    /// Total number of frames captured
    pub total_frames: u64,

    /// Total number of actions recorded
    pub total_actions: u64,

    /// Total number of key presses
    pub key_presses: u64,

    /// Total number of mouse events
    pub mouse_events: u64,

    /// Total number of commands executed
    pub commands_executed: u64,

    /// Total number of tool calls
    pub tool_calls: u64,

    /// Total number of errors
    pub errors: u64,

    /// Session duration
    pub duration: Option<Duration>,

    /// Actions per category
    pub actions_by_category: HashMap<String, u64>,
}

impl SessionStats {
    /// Create new empty stats.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an action.
    pub fn record_action(&mut self, action: &TuiAction) {
        self.total_actions += 1;

        let category = action.action_type.category().to_string();
        *self.actions_by_category.entry(category).or_insert(0) += 1;

        match &action.action_type {
            ActionType::KeyPress(_) | ActionType::Paste(_) => self.key_presses += 1,
            ActionType::MouseClick { .. }
            | ActionType::MouseScroll { .. }
            | ActionType::MouseMove { .. } => self.mouse_events += 1,
            ActionType::Command(_) => self.commands_executed += 1,
            ActionType::ToolCall { .. } => self.tool_calls += 1,
            ActionType::Error(_) => self.errors += 1,
            _ => {}
        }
    }

    /// Record a frame.
    pub fn record_frame(&mut self) {
        self.total_frames += 1;
    }

    /// Set the session duration.
    pub fn set_duration(&mut self, duration: Duration) {
        self.duration = Some(duration);
    }

    /// Get a summary string.
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("{} frames", self.total_frames),
            format!("{} actions", self.total_actions),
        ];

        if self.key_presses > 0 {
            parts.push(format!("{} key presses", self.key_presses));
        }
        if self.mouse_events > 0 {
            parts.push(format!("{} mouse events", self.mouse_events));
        }
        if self.commands_executed > 0 {
            parts.push(format!("{} commands", self.commands_executed));
        }
        if self.tool_calls > 0 {
            parts.push(format!("{} tool calls", self.tool_calls));
        }
        if self.errors > 0 {
            parts.push(format!("{} errors", self.errors));
        }

        if let Some(d) = self.duration {
            parts.push(format!("{:.2}s duration", d.as_secs_f64()));
        }

        parts.join(", ")
    }
}

/// Session recorder for capturing TUI sessions.
pub struct SessionRecorder {
    /// Session ID
    session_id: Uuid,

    /// Session name
    name: String,

    /// Description
    description: Option<String>,

    /// When the session started
    started_at: DateTime<Utc>,

    /// Start instant for duration tracking
    start_instant: Instant,

    /// Terminal dimensions
    terminal_size: (u16, u16),

    /// Configuration
    config: CaptureConfig,

    /// Events
    events: Vec<TuiEvent>,

    /// Frames
    frames: Vec<CapturedFrame>,

    /// Actions
    actions: Vec<TuiAction>,

    /// Pending actions (to be attached to next frame)
    pending_actions: Vec<TuiAction>,

    /// Frame counter
    frame_counter: u64,

    /// Action counter
    action_counter: u64,

    /// Metadata
    metadata: HashMap<String, String>,

    /// Statistics
    stats: SessionStats,

    /// Frame capture helper (for future use with direct terminal capture)
    #[allow(dead_code)]
    frame_capture: FrameCapture,
}

impl SessionRecorder {
    /// Create a new session recorder.
    pub fn new(name: impl Into<String>, width: u16, height: u16) -> Self {
        let config = CaptureConfig::new(width, height);
        Self::with_config(name, config)
    }

    /// Create with a specific configuration.
    pub fn with_config(name: impl Into<String>, config: CaptureConfig) -> Self {
        let session_id = Uuid::new_v4();
        let started_at = Utc::now();
        let terminal_size = (config.width, config.height);
        let frame_capture = FrameCapture::new(config.clone());

        Self {
            session_id,
            name: name.into(),
            description: None,
            started_at,
            start_instant: Instant::now(),
            terminal_size,
            config,
            events: vec![TuiEvent::SessionStarted {
                session_id,
                timestamp: started_at,
                width: terminal_size.0,
                height: terminal_size.1,
            }],
            frames: Vec::new(),
            actions: Vec::new(),
            pending_actions: Vec::new(),
            frame_counter: 0,
            action_counter: 0,
            metadata: HashMap::new(),
            stats: SessionStats::new(),
            frame_capture,
        }
    }

    /// Get the session ID.
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    /// Get the session name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set the session description.
    pub fn set_description(&mut self, description: impl Into<String>) {
        self.description = Some(description.into());
    }

    /// Add metadata.
    pub fn add_metadata(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(key.into(), value.into());
    }

    /// Get the configuration.
    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }

    /// Get the terminal size.
    pub fn terminal_size(&self) -> (u16, u16) {
        self.terminal_size
    }

    /// Get the elapsed time since session start.
    pub fn elapsed(&self) -> Duration {
        self.start_instant.elapsed()
    }

    /// Get the statistics.
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }

    /// Record an action.
    pub fn record_action(&mut self, action: TuiAction) {
        let action = action.with_sequence(self.action_counter);
        self.action_counter += 1;

        // Add to events
        self.events.push(TuiEvent::Action(action.clone()));

        // Update stats
        self.stats.record_action(&action);

        // Store in pending actions for next frame
        self.pending_actions.push(action.clone());

        // Store in all actions
        self.actions.push(action);
    }

    /// Record an action by type.
    pub fn record(&mut self, action_type: ActionType) {
        self.record_action(TuiAction::new(action_type));
    }

    /// Record a key press.
    pub fn key_press(&mut self, key: impl Into<String>) {
        self.record(ActionType::KeyPress(key.into()));
    }

    /// Record a mouse click.
    pub fn mouse_click(&mut self, x: u16, y: u16, button: impl Into<String>) {
        self.record(ActionType::MouseClick {
            x,
            y,
            button: button.into(),
        });
    }

    /// Record a command.
    pub fn command(&mut self, cmd: impl Into<String>) {
        self.record(ActionType::Command(cmd.into()));
    }

    /// Record an error.
    pub fn error(&mut self, msg: impl Into<String>) {
        self.record(ActionType::Error(msg.into()));
    }

    /// Record a marker event (useful for debugging).
    pub fn marker(&mut self, message: impl Into<String>) {
        self.events.push(TuiEvent::Marker {
            timestamp: Utc::now(),
            message: message.into(),
        });
    }

    /// Record a frame from a buffer snapshot.
    pub fn record_frame(&mut self, label: impl Into<String>, buffer: &BufferSnapshot) -> Uuid {
        self.record_frame_internal(Some(label.into()), buffer)
    }

    /// Record a frame without a label.
    pub fn record_frame_unlabeled(&mut self, buffer: &BufferSnapshot) -> Uuid {
        self.record_frame_internal(None, buffer)
    }

    /// Internal frame recording.
    fn record_frame_internal(&mut self, label: Option<String>, buffer: &BufferSnapshot) -> Uuid {
        self.frame_counter += 1;
        self.stats.record_frame();

        let ascii_content = buffer.to_ascii(&self.config);

        let mut frame = CapturedFrame::new(
            self.frame_counter,
            ascii_content,
            self.terminal_size.0,
            self.terminal_size.1,
        );

        if let Some(l) = label.clone() {
            frame = frame.with_label(l);
        }

        // Attach pending actions
        frame = frame.with_actions(std::mem::take(&mut self.pending_actions));

        let id = frame.id;

        // Add frame event
        self.events.push(TuiEvent::FrameRendered {
            frame_number: self.frame_counter,
            timestamp: frame.timestamp,
        });

        // Add snapshot event if labeled
        if let Some(l) = label {
            self.events.push(TuiEvent::SnapshotTaken {
                snapshot_id: id,
                timestamp: frame.timestamp,
                label: l,
            });
        }

        self.frames.push(frame);
        id
    }

    /// Get all frames.
    pub fn frames(&self) -> &[CapturedFrame] {
        &self.frames
    }

    /// Get all actions.
    pub fn actions(&self) -> &[TuiAction] {
        &self.actions
    }

    /// Get all events.
    pub fn events(&self) -> &[TuiEvent] {
        &self.events
    }

    /// Get frame by ID.
    pub fn get_frame(&self, id: Uuid) -> Option<&CapturedFrame> {
        self.frames.iter().find(|f| f.id == id)
    }

    /// Get frame by number.
    pub fn get_frame_by_number(&self, number: u64) -> Option<&CapturedFrame> {
        self.frames.iter().find(|f| f.frame_number == number)
    }

    /// Get the latest frame.
    pub fn latest_frame(&self) -> Option<&CapturedFrame> {
        self.frames.last()
    }

    /// End the session and generate a report.
    pub fn end_session(mut self) -> SessionReport {
        let ended_at = Utc::now();
        let duration = self.start_instant.elapsed();
        self.stats.set_duration(duration);

        // Add session ended event
        self.events.push(TuiEvent::SessionEnded {
            session_id: self.session_id,
            timestamp: ended_at,
            total_frames: self.frame_counter,
            total_actions: self.action_counter,
        });

        SessionReport {
            session_id: self.session_id,
            name: self.name,
            description: self.description,
            started_at: self.started_at,
            ended_at: Some(ended_at),
            terminal_size: self.terminal_size,
            events: self.events,
            frames: self.frames,
            actions: self.actions,
            metadata: self.metadata,
            stats: self.stats,
        }
    }

    /// Generate markdown report without ending the session.
    pub fn to_markdown(&self) -> String {
        let mut result = String::new();

        // Header
        result.push_str(&format!("# TUI Session: {}\n\n", self.name));

        if let Some(desc) = &self.description {
            result.push_str(&format!("{}\n\n", desc));
        }

        // Session info
        result.push_str("## Session Information\n\n");
        result.push_str(&format!("- **Session ID:** `{}`\n", self.session_id));
        result.push_str(&format!(
            "- **Started:** {}\n",
            self.started_at.format("%Y-%m-%d %H:%M:%S UTC")
        ));
        result.push_str(&format!(
            "- **Terminal Size:** {}x{}\n",
            self.terminal_size.0, self.terminal_size.1
        ));
        result.push_str(&format!(
            "- **Elapsed:** {:.2}s\n",
            self.elapsed().as_secs_f64()
        ));
        result.push('\n');

        // Statistics
        result.push_str("## Statistics\n\n");
        result.push_str(&format!(
            "- **Total Frames:** {}\n",
            self.stats.total_frames
        ));
        result.push_str(&format!(
            "- **Total Actions:** {}\n",
            self.stats.total_actions
        ));
        result.push_str(&format!("- **Key Presses:** {}\n", self.stats.key_presses));
        result.push_str(&format!(
            "- **Mouse Events:** {}\n",
            self.stats.mouse_events
        ));
        result.push_str(&format!(
            "- **Commands:** {}\n",
            self.stats.commands_executed
        ));
        result.push_str(&format!("- **Tool Calls:** {}\n", self.stats.tool_calls));
        result.push_str(&format!("- **Errors:** {}\n", self.stats.errors));
        result.push('\n');

        // Metadata
        if !self.metadata.is_empty() {
            result.push_str("## Metadata\n\n");
            for (key, value) in &self.metadata {
                result.push_str(&format!("- **{}:** {}\n", key, value));
            }
            result.push('\n');
        }

        // Timeline
        result.push_str("## Timeline\n\n");
        for event in &self.events {
            let timestamp = event.timestamp().format("%H:%M:%S%.3f");
            result.push_str(&format!("- `{}` {}\n", timestamp, event.description()));
        }
        result.push('\n');

        // Frames
        result.push_str("## Captured Frames\n\n");
        for frame in &self.frames {
            // Skip unlabeled frames if configured
            if self.config.labeled_frames_only && frame.label.is_none() {
                continue;
            }

            if let Some(label) = &frame.label {
                result.push_str(&format!("### Frame {} - {}\n\n", frame.frame_number, label));
            } else {
                result.push_str(&format!("### Frame {}\n\n", frame.frame_number));
            }

            if self.config.include_timestamps {
                result.push_str(&format!(
                    "**Captured at:** {}\n\n",
                    frame.timestamp.format("%H:%M:%S%.3f")
                ));
            }

            // Actions leading to this frame
            if self.config.include_actions && !frame.preceding_actions.is_empty() {
                result.push_str("**Preceding Actions:**\n");
                for action in &frame.preceding_actions {
                    result.push_str(&format!(
                        "- {} `{}` {}\n",
                        action.action_type.icon(),
                        action.timestamp_str(),
                        action.action_type.description()
                    ));
                }
                result.push('\n');
            }

            // ASCII content
            result.push_str("```\n");
            result.push_str(&frame.ascii_content);
            result.push_str("\n```\n\n");

            if self.config.add_frame_separators {
                result.push_str("---\n\n");
            }
        }

        result
    }

    /// Export the session to a markdown file.
    pub async fn export_markdown(&self, output_dir: impl AsRef<Path>) -> CaptureResult<String> {
        let output_dir = output_dir.as_ref();

        // Create output directory if it doesn't exist
        fs::create_dir_all(output_dir)
            .await
            .map_err(CaptureError::IoError)?;

        // Generate filename
        let filename = format!(
            "tui_session_{}_{}.md",
            self.name.replace(' ', "_").to_lowercase(),
            self.started_at.format("%Y%m%d_%H%M%S")
        );
        let filepath = output_dir.join(&filename);

        // Generate markdown content
        let content = self.to_markdown();

        // Write to file
        fs::write(&filepath, content)
            .await
            .map_err(CaptureError::IoError)?;

        Ok(filepath.to_string_lossy().to_string())
    }

    /// Export the session to JSON.
    pub async fn export_json(&self, output_dir: impl AsRef<Path>) -> CaptureResult<String> {
        let output_dir = output_dir.as_ref();

        // Create output directory
        fs::create_dir_all(output_dir)
            .await
            .map_err(CaptureError::IoError)?;

        // Generate filename
        let filename = format!(
            "tui_session_{}_{}.json",
            self.name.replace(' ', "_").to_lowercase(),
            self.started_at.format("%Y%m%d_%H%M%S")
        );
        let filepath = output_dir.join(&filename);

        // Generate report
        let report = SessionReport {
            session_id: self.session_id,
            name: self.name.clone(),
            description: self.description.clone(),
            started_at: self.started_at,
            ended_at: None,
            terminal_size: self.terminal_size,
            events: self.events.clone(),
            frames: self.frames.clone(),
            actions: self.actions.clone(),
            metadata: self.metadata.clone(),
            stats: self.stats.clone(),
        };

        // Serialize to JSON
        let content = serde_json::to_string_pretty(&report)
            .map_err(|e| CaptureError::SerializationError(e.to_string()))?;

        // Write to file
        fs::write(&filepath, content)
            .await
            .map_err(CaptureError::IoError)?;

        Ok(filepath.to_string_lossy().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_recorder_new() {
        let recorder = SessionRecorder::new("Test Session", 80, 24);
        assert_eq!(recorder.name(), "Test Session");
        assert_eq!(recorder.terminal_size(), (80, 24));
    }

    #[test]
    fn test_record_action() {
        let mut recorder = SessionRecorder::new("Test", 80, 24);

        recorder.key_press("Enter");
        recorder.key_press("a");
        recorder.command("/help");

        assert_eq!(recorder.actions().len(), 3);
        assert_eq!(recorder.stats().key_presses, 2);
        assert_eq!(recorder.stats().commands_executed, 1);
    }

    #[test]
    fn test_record_frame() {
        let mut recorder = SessionRecorder::new("Test", 80, 24);

        let buffer = BufferSnapshot::new(80, 24);
        let id = recorder.record_frame("Initial", &buffer);

        assert_eq!(recorder.frames().len(), 1);
        assert!(recorder.get_frame(id).is_some());
    }

    #[test]
    fn test_frame_with_actions() {
        let mut recorder = SessionRecorder::new("Test", 80, 24);

        recorder.key_press("a");
        recorder.key_press("b");

        let buffer = BufferSnapshot::new(80, 24);
        recorder.record_frame("After keys", &buffer);

        let frame = recorder.latest_frame().unwrap();
        assert_eq!(frame.preceding_actions.len(), 2);
    }

    #[test]
    fn test_session_stats() {
        let mut stats = SessionStats::new();

        stats.record_action(&TuiAction::new(ActionType::KeyPress("a".into())));
        stats.record_action(&TuiAction::new(ActionType::MouseClick {
            x: 10,
            y: 5,
            button: "left".into(),
        }));
        stats.record_action(&TuiAction::new(ActionType::Command("/help".into())));
        stats.record_frame();

        assert_eq!(stats.total_actions, 3);
        assert_eq!(stats.key_presses, 1);
        assert_eq!(stats.mouse_events, 1);
        assert_eq!(stats.commands_executed, 1);
        assert_eq!(stats.total_frames, 1);
    }

    #[test]
    fn test_session_report_generation() {
        let mut recorder = SessionRecorder::new("Test Session", 80, 24);
        recorder.set_description("A test session");
        recorder.add_metadata("version", "1.0.0");

        recorder.key_press("Enter");
        let buffer = BufferSnapshot::new(80, 24);
        recorder.record_frame("Initial", &buffer);

        let report = recorder.end_session();

        assert_eq!(report.name, "Test Session");
        assert!(report.description.is_some());
        assert!(report.ended_at.is_some());
        assert_eq!(report.frames.len(), 1);
        assert_eq!(report.actions.len(), 1);
    }

    #[test]
    fn test_markdown_generation() {
        let mut recorder = SessionRecorder::new("Test", 40, 10);

        recorder.key_press("Enter");
        let buffer = BufferSnapshot::new(40, 10);
        recorder.record_frame("Initial", &buffer);

        let md = recorder.to_markdown();

        assert!(md.contains("# TUI Session: Test"));
        assert!(md.contains("## Session Information"));
        assert!(md.contains("## Statistics"));
        assert!(md.contains("## Captured Frames"));
        assert!(md.contains("### Frame 1"));
    }
}
