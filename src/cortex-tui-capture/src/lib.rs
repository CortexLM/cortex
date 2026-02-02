//! # Cortex TUI Capture
//!
//! A comprehensive TUI capture and snapshot testing framework for debugging
//! terminal user interfaces in the Cortex CLI ecosystem.
//!
//! ## Overview
//!
//! This crate provides tools for:
//! - Capturing TUI frames as ASCII art snapshots
//! - Recording TUI sessions with all actions and state changes
//! - Generating markdown reports for debugging
//! - Creating test harnesses for TUI components
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    TUI Capture System                           │
//! │                                                                 │
//! │  ┌─────────────┐    ┌──────────────┐    ┌─────────────────┐   │
//! │  │ FrameCapture│───▶│SessionRecorder│───▶│MarkdownExporter │   │
//! │  │             │    │              │    │                 │   │
//! │  │ - ASCII art │    │ - Events     │    │ - .md reports   │   │
//! │  │ - Metadata  │    │ - Frames     │    │ - ASCII blocks  │   │
//! │  │ - Timing    │    │ - Actions    │    │ - Timestamps    │   │
//! │  └─────────────┘    └──────────────┘    └─────────────────┘   │
//! │                                                                 │
//! │  ┌─────────────────────────────────────────────────────────┐   │
//! │  │                    MockTerminal                          │   │
//! │  │  - Simulates terminal for headless testing               │   │
//! │  │  - Captures all rendering operations                     │   │
//! │  │  - Provides frame-by-frame inspection                    │   │
//! │  └─────────────────────────────────────────────────────────┘   │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Quick Start
//!
//! ### Capturing a single frame
//!
//! ```rust,ignore
//! use cortex_tui_capture::{FrameCapture, CaptureConfig};
//! use ratatui::widgets::Paragraph;
//!
//! let config = CaptureConfig::new(80, 24);
//! let mut capture = FrameCapture::new(config);
//!
//! // Render a widget
//! capture.render(|frame| {
//!     let widget = Paragraph::new("Hello, TUI!");
//!     frame.render_widget(widget, frame.area());
//! });
//!
//! // Export to markdown
//! let md = capture.to_markdown();
//! println!("{}", md);
//! ```
//!
//! ### Recording a session
//!
//! ```rust,ignore
//! use cortex_tui_capture::{SessionRecorder, TuiAction, ActionType};
//!
//! let mut recorder = SessionRecorder::new("my_session", 80, 24);
//!
//! // Record actions
//! recorder.record_action(TuiAction::new(ActionType::KeyPress("Enter".into())));
//! recorder.record_frame("Initial state", &buffer);
//!
//! // Export session report
//! recorder.export_markdown("./debug_output").await?;
//! ```
//!
//! ## Output Format
//!
//! The markdown output includes:
//! - Session metadata (timestamp, terminal size)
//! - Chronological list of actions with timestamps
//! - ASCII captures of TUI state at key moments
//! - Event details with formatted parameters

mod capture;
mod config;
mod exporter;
pub mod integration;
mod mock_terminal;
mod recorder;
pub mod screenshot_generator;
mod types;

pub use capture::{BufferSnapshot, FrameCapture, SnapshotCell};
pub use config::{CaptureConfig, OutputFormat, StyleRendering};
pub use exporter::{MarkdownExporter, ReportSection};
pub use integration::{CaptureManager, ExportResult, QuickCapture};
pub use mock_terminal::{MockBackend, MockTerminal};
pub use recorder::{SessionRecorder, SessionReport, SessionStats};
pub use screenshot_generator::{
    DEFAULT_OUTPUT_DIR, GeneratorConfig, GeneratorResult, ScreenshotGenerator, ScreenshotScenario,
    generate_all_screenshots, generate_screenshots_to,
};
pub use types::{ActionType, CaptureError, CaptureResult, CapturedFrame, TuiAction, TuiEvent};

/// Cortex TUI Capture version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Convenience function to create a default capture configuration.
#[inline]
pub fn default_config() -> CaptureConfig {
    CaptureConfig::default()
}

/// Convenience function to create a capture configuration with specific dimensions.
#[inline]
pub fn config_with_size(width: u16, height: u16) -> CaptureConfig {
    CaptureConfig::new(width, height)
}
