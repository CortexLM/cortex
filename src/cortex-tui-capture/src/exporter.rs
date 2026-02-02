//! Markdown and report exporting utilities.
//!
//! This module provides utilities for generating markdown reports from
//! captured TUI sessions, with support for various formatting options.

use crate::config::CaptureConfig;
use crate::recorder::SessionReport;
use crate::types::CapturedFrame;
use chrono::{DateTime, Utc};
use std::fmt::Write;

/// Sections that can be included in a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportSection {
    /// Session header with basic info
    Header,
    /// Session metadata
    Metadata,
    /// Statistics summary
    Statistics,
    /// Timeline of events
    Timeline,
    /// Captured frames
    Frames,
    /// All actions
    Actions,
    /// Table of contents
    TableOfContents,
}

/// Markdown exporter for session reports.
pub struct MarkdownExporter {
    /// Configuration
    config: CaptureConfig,

    /// Sections to include
    sections: Vec<ReportSection>,

    /// Custom CSS (for HTML output)
    custom_css: Option<String>,

    /// Include navigation links
    include_navigation: bool,

    /// Collapse frames by default
    collapse_frames: bool,

    /// Maximum preview length for actions
    max_action_preview: usize,
}

impl Default for MarkdownExporter {
    fn default() -> Self {
        Self::new(CaptureConfig::default())
    }
}

impl MarkdownExporter {
    /// Create a new markdown exporter with the given configuration.
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            config,
            sections: vec![
                ReportSection::Header,
                ReportSection::Statistics,
                ReportSection::Timeline,
                ReportSection::Frames,
            ],
            custom_css: None,
            include_navigation: true,
            collapse_frames: false,
            max_action_preview: 100,
        }
    }

    /// Create a minimal exporter (header and frames only).
    pub fn minimal(config: CaptureConfig) -> Self {
        Self {
            config,
            sections: vec![ReportSection::Header, ReportSection::Frames],
            custom_css: None,
            include_navigation: false,
            collapse_frames: false,
            max_action_preview: 50,
        }
    }

    /// Create a verbose exporter (all sections).
    pub fn verbose(config: CaptureConfig) -> Self {
        Self {
            config,
            sections: vec![
                ReportSection::TableOfContents,
                ReportSection::Header,
                ReportSection::Metadata,
                ReportSection::Statistics,
                ReportSection::Timeline,
                ReportSection::Actions,
                ReportSection::Frames,
            ],
            custom_css: None,
            include_navigation: true,
            collapse_frames: false,
            max_action_preview: 200,
        }
    }

    /// Set the sections to include.
    pub fn with_sections(mut self, sections: Vec<ReportSection>) -> Self {
        self.sections = sections;
        self
    }

    /// Add a section.
    pub fn add_section(mut self, section: ReportSection) -> Self {
        if !self.sections.contains(&section) {
            self.sections.push(section);
        }
        self
    }

    /// Remove a section.
    pub fn remove_section(mut self, section: ReportSection) -> Self {
        self.sections.retain(|s| *s != section);
        self
    }

    /// Set custom CSS for HTML output.
    pub fn with_custom_css(mut self, css: impl Into<String>) -> Self {
        self.custom_css = Some(css.into());
        self
    }

    /// Set whether to include navigation links.
    pub fn with_navigation(mut self, include: bool) -> Self {
        self.include_navigation = include;
        self
    }

    /// Set whether to collapse frames by default.
    pub fn collapse_frames(mut self, collapse: bool) -> Self {
        self.collapse_frames = collapse;
        self
    }

    /// Set maximum action preview length.
    pub fn with_max_action_preview(mut self, max: usize) -> Self {
        self.max_action_preview = max;
        self
    }

    /// Export a session report to markdown.
    pub fn export(&self, report: &SessionReport) -> String {
        let mut output = String::new();

        for section in &self.sections {
            match section {
                ReportSection::TableOfContents => self.write_toc(&mut output, report),
                ReportSection::Header => self.write_header(&mut output, report),
                ReportSection::Metadata => self.write_metadata(&mut output, report),
                ReportSection::Statistics => self.write_statistics(&mut output, report),
                ReportSection::Timeline => self.write_timeline(&mut output, report),
                ReportSection::Actions => self.write_actions(&mut output, report),
                ReportSection::Frames => self.write_frames(&mut output, report),
            }
        }

        if self.include_navigation {
            self.write_footer(&mut output);
        }

        output
    }

    /// Write table of contents.
    fn write_toc(&self, output: &mut String, report: &SessionReport) {
        output.push_str("## Table of Contents\n\n");

        for section in &self.sections {
            if *section == ReportSection::TableOfContents {
                continue;
            }
            let name = match section {
                ReportSection::Header => "Session Information",
                ReportSection::Metadata => "Metadata",
                ReportSection::Statistics => "Statistics",
                ReportSection::Timeline => "Timeline",
                ReportSection::Actions => "Actions",
                ReportSection::Frames => "Captured Frames",
                _ => continue,
            };
            let anchor = name.to_lowercase().replace(' ', "-");
            let _ = writeln!(output, "- [{}](#{})", name, anchor);
        }

        // Add frame links
        if self.sections.contains(&ReportSection::Frames) {
            output.push_str("  - Frames:\n");
            for frame in &report.frames {
                if self.config.labeled_frames_only && frame.label.is_none() {
                    continue;
                }
                let default_label = format!("Frame {}", frame.frame_number);
                let label = frame.label.as_deref().unwrap_or(&default_label);
                let _ = writeln!(output, "    - [{}](#frame-{})", label, frame.frame_number);
            }
        }

        output.push('\n');
    }

    /// Write header section.
    fn write_header(&self, output: &mut String, report: &SessionReport) {
        let _ = writeln!(output, "# TUI Session: {}\n", report.name);

        if let Some(desc) = &report.description {
            let _ = writeln!(output, "{}\n", desc);
        }

        output.push_str("## Session Information\n\n");

        let _ = writeln!(output, "| Property | Value |");
        let _ = writeln!(output, "|----------|-------|");
        let _ = writeln!(output, "| Session ID | `{}` |", report.session_id);
        let _ = writeln!(
            output,
            "| Started | {} |",
            report.started_at.format("%Y-%m-%d %H:%M:%S UTC")
        );
        if let Some(ended) = report.ended_at {
            let _ = writeln!(
                output,
                "| Ended | {} |",
                ended.format("%Y-%m-%d %H:%M:%S UTC")
            );
        }
        let _ = writeln!(
            output,
            "| Terminal Size | {}x{} |",
            report.terminal_size.0, report.terminal_size.1
        );
        if let Some(duration) = report.stats.duration {
            let _ = writeln!(output, "| Duration | {:.2}s |", duration.as_secs_f64());
        }

        output.push('\n');
    }

    /// Write metadata section.
    fn write_metadata(&self, output: &mut String, report: &SessionReport) {
        if report.metadata.is_empty() {
            return;
        }

        output.push_str("## Metadata\n\n");

        let _ = writeln!(output, "| Key | Value |");
        let _ = writeln!(output, "|-----|-------|");
        for (key, value) in &report.metadata {
            let _ = writeln!(output, "| {} | {} |", key, value);
        }

        output.push('\n');
    }

    /// Write statistics section.
    fn write_statistics(&self, output: &mut String, report: &SessionReport) {
        output.push_str("## Statistics\n\n");

        let _ = writeln!(output, "| Metric | Count |");
        let _ = writeln!(output, "|--------|-------|");
        let _ = writeln!(output, "| Total Frames | {} |", report.stats.total_frames);
        let _ = writeln!(output, "| Total Actions | {} |", report.stats.total_actions);
        let _ = writeln!(output, "| Key Presses | {} |", report.stats.key_presses);
        let _ = writeln!(output, "| Mouse Events | {} |", report.stats.mouse_events);
        let _ = writeln!(output, "| Commands | {} |", report.stats.commands_executed);
        let _ = writeln!(output, "| Tool Calls | {} |", report.stats.tool_calls);
        let _ = writeln!(output, "| Errors | {} |", report.stats.errors);

        output.push('\n');

        // Actions by category
        if !report.stats.actions_by_category.is_empty() {
            output.push_str("### Actions by Category\n\n");
            let _ = writeln!(output, "| Category | Count |");
            let _ = writeln!(output, "|----------|-------|");
            for (category, count) in &report.stats.actions_by_category {
                let _ = writeln!(output, "| {} | {} |", category, count);
            }
            output.push('\n');
        }
    }

    /// Write timeline section.
    fn write_timeline(&self, output: &mut String, report: &SessionReport) {
        output.push_str("## Timeline\n\n");

        let start_time = report.started_at;

        for event in &report.events {
            let elapsed = (event.timestamp() - start_time)
                .to_std()
                .map(|d| format!("+{:.3}s", d.as_secs_f64()))
                .unwrap_or_default();

            let timestamp = event.timestamp().format("%H:%M:%S%.3f");

            let _ = writeln!(
                output,
                "- `{}` ({}) {}",
                timestamp,
                elapsed,
                event.description()
            );
        }

        output.push('\n');
    }

    /// Write actions section.
    fn write_actions(&self, output: &mut String, report: &SessionReport) {
        output.push_str("## Actions\n\n");

        let _ = writeln!(output, "| # | Time | Category | Action |");
        let _ = writeln!(output, "|---|------|----------|--------|");

        for action in &report.actions {
            let desc = action.action_type.description();
            let desc = if desc.len() > self.max_action_preview {
                format!("{}...", &desc[..self.max_action_preview])
            } else {
                desc
            };

            let _ = writeln!(
                output,
                "| {} | {} | {} | {} {} |",
                action.sequence,
                action.timestamp_str(),
                action.action_type.category(),
                action.action_type.icon(),
                desc
            );
        }

        output.push('\n');
    }

    /// Write frames section.
    fn write_frames(&self, output: &mut String, report: &SessionReport) {
        output.push_str("## Captured Frames\n\n");

        let _ = writeln!(
            output,
            "Total frames captured: **{}**\n",
            report.frames.len()
        );

        for frame in &report.frames {
            // Skip unlabeled frames if configured
            if self.config.labeled_frames_only && frame.label.is_none() {
                continue;
            }

            self.write_frame(output, frame);

            if self.config.add_frame_separators {
                output.push_str("---\n\n");
            }
        }
    }

    /// Write a single frame.
    fn write_frame(&self, output: &mut String, frame: &CapturedFrame) {
        // Frame header with anchor
        if let Some(label) = &frame.label {
            let _ = writeln!(
                output,
                "### <a id=\"frame-{}\"></a>Frame {} - {}\n",
                frame.frame_number, frame.frame_number, label
            );
        } else {
            let _ = writeln!(
                output,
                "### <a id=\"frame-{}\"></a>Frame {}\n",
                frame.frame_number, frame.frame_number
            );
        }

        // Timestamp
        if self.config.include_timestamps {
            let _ = writeln!(
                output,
                "**Captured at:** {}\n",
                frame.timestamp.format("%H:%M:%S%.3f")
            );
        }

        // Metadata
        if self.config.include_metadata && !frame.metadata.is_empty() {
            output.push_str("**Metadata:**\n");
            for (key, value) in &frame.metadata {
                let _ = writeln!(output, "- {}: {}", key, value);
            }
            output.push('\n');
        }

        // Preceding actions
        if self.config.include_actions && !frame.preceding_actions.is_empty() {
            output.push_str("**Preceding Actions:**\n\n");
            for action in &frame.preceding_actions {
                let _ = writeln!(
                    output,
                    "- {} `{}` {}",
                    action.action_type.icon(),
                    action.timestamp_str(),
                    action.action_type.description()
                );
            }
            output.push('\n');
        }

        // ASCII content
        if self.collapse_frames {
            output.push_str("<details>\n<summary>View Frame</summary>\n\n");
        }

        output.push_str("```\n");
        output.push_str(&frame.ascii_content);
        output.push_str("\n```\n\n");

        if self.collapse_frames {
            output.push_str("</details>\n\n");
        }

        // Navigation
        if self.include_navigation && frame.frame_number > 1 {
            let _ = writeln!(
                output,
                "[← Previous Frame](#frame-{}) | [↑ Top](#table-of-contents)",
                frame.frame_number - 1
            );
            output.push('\n');
        }
    }

    /// Write footer.
    fn write_footer(&self, output: &mut String) {
        output.push_str("---\n\n");
        output.push_str("*Generated by cortex-tui-capture*\n");
    }

    /// Export frames only (simplified output).
    pub fn export_frames_only(&self, frames: &[CapturedFrame]) -> String {
        let mut output = String::new();

        output.push_str("# TUI Frames\n\n");

        for frame in frames {
            if self.config.labeled_frames_only && frame.label.is_none() {
                continue;
            }

            if let Some(label) = &frame.label {
                let _ = writeln!(output, "## {}\n", label);
            } else {
                let _ = writeln!(output, "## Frame {}\n", frame.frame_number);
            }

            output.push_str("```\n");
            output.push_str(&frame.ascii_content);
            output.push_str("\n```\n\n");
        }

        output
    }

    /// Export a quick summary.
    pub fn export_summary(&self, report: &SessionReport) -> String {
        let mut output = String::new();

        let _ = writeln!(output, "# Session Summary: {}", report.name);
        let _ = writeln!(output);
        let _ = writeln!(output, "- {} frames captured", report.stats.total_frames);
        let _ = writeln!(output, "- {} total actions", report.stats.total_actions);

        if let Some(duration) = report.stats.duration {
            let _ = writeln!(output, "- {:.2}s duration", duration.as_secs_f64());
        }

        if report.stats.errors > 0 {
            let _ = writeln!(output, "- Warning: {} errors occurred", report.stats.errors);
        }

        output
    }
}

/// Format a timestamp relative to a start time.
#[allow(dead_code)]
pub fn format_relative_time(time: DateTime<Utc>, start: DateTime<Utc>) -> String {
    let elapsed = (time - start).to_std().unwrap_or_default();
    format!("+{:.3}s", elapsed.as_secs_f64())
}

/// Format a duration.
#[allow(dead_code)]
pub fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    if secs >= 3600 {
        format!("{}h {}m {}s", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else if secs >= 60 {
        format!("{}m {}.{:03}s", secs / 60, secs % 60, millis)
    } else {
        format!("{}.{:03}s", secs, millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ActionType, TuiAction};

    fn create_test_report() -> SessionReport {
        use crate::recorder::SessionStats;
        use std::collections::HashMap;
        use uuid::Uuid;

        SessionReport {
            session_id: Uuid::new_v4(),
            name: "Test Session".to_string(),
            description: Some("A test session".to_string()),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            terminal_size: (80, 24),
            events: vec![],
            frames: vec![
                CapturedFrame::new(1, "Hello World".to_string(), 80, 24).with_label("Initial"),
                CapturedFrame::new(2, "After Action".to_string(), 80, 24)
                    .with_label("After action"),
            ],
            actions: vec![TuiAction::new(ActionType::KeyPress("Enter".to_string()))],
            metadata: HashMap::new(),
            stats: SessionStats {
                total_frames: 2,
                total_actions: 1,
                key_presses: 1,
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_export_default() {
        let config = CaptureConfig::default();
        let exporter = MarkdownExporter::new(config);
        let report = create_test_report();

        let md = exporter.export(&report);

        assert!(md.contains("# TUI Session: Test Session"));
        assert!(md.contains("## Session Information"));
        assert!(md.contains("## Statistics"));
        assert!(md.contains("## Captured Frames"));
    }

    #[test]
    fn test_export_minimal() {
        let config = CaptureConfig::default();
        let exporter = MarkdownExporter::minimal(config);
        let report = create_test_report();

        let md = exporter.export(&report);

        assert!(md.contains("# TUI Session: Test Session"));
        assert!(md.contains("## Captured Frames"));
        assert!(!md.contains("## Timeline"));
    }

    #[test]
    fn test_export_verbose() {
        let config = CaptureConfig::default();
        let exporter = MarkdownExporter::verbose(config);
        let report = create_test_report();

        let md = exporter.export(&report);

        assert!(md.contains("## Table of Contents"));
        assert!(md.contains("## Actions"));
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(std::time::Duration::from_secs(5)), "5.000s");
        assert_eq!(
            format_duration(std::time::Duration::from_secs(65)),
            "1m 5.000s"
        );
        assert_eq!(
            format_duration(std::time::Duration::from_secs(3665)),
            "1h 1m 5s"
        );
    }
}
