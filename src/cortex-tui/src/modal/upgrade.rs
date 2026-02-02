//! Upgrade Modal - Self-update interface for Cortex CLI
//!
//! This modal provides a user-friendly interface for checking and installing
//! updates to the Cortex CLI, with progress display and status feedback.

use super::{CancelBehavior, Modal, ModalAction, ModalResult};
use crate::widgets::ActionBar;
use cortex_core::style::{
    CYAN_PRIMARY, GREEN, RED, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED, WARNING,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Padding, Widget},
};

/// State of the upgrade process
#[derive(Debug, Clone, PartialEq)]
pub enum UpgradeState {
    /// Checking for updates
    Checking,
    /// No update available
    UpToDate { version: String },
    /// Update available, waiting for confirmation
    Available {
        current_version: String,
        new_version: String,
        changelog: Option<String>,
    },
    /// Downloading update
    Downloading { progress: f64, total_bytes: u64 },
    /// Verifying download
    Verifying,
    /// Installing update
    Installing,
    /// Update complete, restart required
    Complete { new_version: String },
    /// Update failed
    Failed { error: String },
}

/// Modal for self-update functionality
pub struct UpgradeModal {
    /// Current state of upgrade
    pub state: UpgradeState,
    /// Whether user has confirmed the update
    confirmed: bool,
    /// Animation frame for loading indicators
    animation_frame: usize,
}

impl UpgradeModal {
    /// Create a new upgrade modal in checking state
    pub fn new() -> Self {
        Self {
            state: UpgradeState::Checking,
            confirmed: false,
            animation_frame: 0,
        }
    }

    /// Create with a specific initial state
    pub fn with_state(state: UpgradeState) -> Self {
        Self {
            state,
            confirmed: false,
            animation_frame: 0,
        }
    }

    /// Update the state
    pub fn set_state(&mut self, state: UpgradeState) {
        self.state = state;
    }

    /// Set download progress
    pub fn set_progress(&mut self, progress: f64, total_bytes: u64) {
        self.state = UpgradeState::Downloading {
            progress,
            total_bytes,
        };
    }

    /// Set completion state
    pub fn set_complete(&mut self, new_version: String) {
        self.state = UpgradeState::Complete { new_version };
    }

    /// Set failure state
    pub fn set_failed(&mut self, error: String) {
        self.state = UpgradeState::Failed { error };
    }

    /// Advance animation frame
    pub fn tick(&mut self) {
        self.animation_frame = (self.animation_frame + 1) % 4;
    }

    /// Get loading indicator
    fn loading_indicator(&self) -> &'static str {
        match self.animation_frame {
            0 => "|",
            1 => "/",
            2 => "-",
            3 => "\\",
            _ => "|",
        }
    }
}

impl Default for UpgradeModal {
    fn default() -> Self {
        Self::new()
    }
}

impl Modal for UpgradeModal {
    fn title(&self) -> &str {
        "Cortex Update"
    }

    fn desired_height(&self, _max_height: u16, _width: u16) -> u16 {
        match &self.state {
            UpgradeState::Available {
                changelog: Some(_), ..
            } => 16,
            _ => 12,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Clear background
        Clear.render(area, buf);

        // Border color based on state
        let border_color = match &self.state {
            UpgradeState::Complete { .. } => GREEN,
            UpgradeState::Failed { .. } => RED,
            UpgradeState::Available { .. } => WARNING,
            _ => CYAN_PRIMARY,
        };

        let block = Block::default()
            .title(" Cortex Update ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(SURFACE_1))
            .padding(Padding::horizontal(2));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 6 || inner.width < 30 {
            return;
        }

        match &self.state {
            UpgradeState::Checking => self.render_checking(inner, buf),
            UpgradeState::UpToDate { version } => self.render_up_to_date(inner, buf, version),
            UpgradeState::Available {
                current_version,
                new_version,
                changelog,
            } => {
                self.render_available(
                    inner,
                    buf,
                    current_version,
                    new_version,
                    changelog.as_deref(),
                );
            }
            UpgradeState::Downloading {
                progress,
                total_bytes,
            } => {
                self.render_downloading(inner, buf, *progress, *total_bytes);
            }
            UpgradeState::Verifying => self.render_verifying(inner, buf),
            UpgradeState::Installing => self.render_installing(inner, buf),
            UpgradeState::Complete { new_version } => self.render_complete(inner, buf, new_version),
            UpgradeState::Failed { error } => self.render_failed(inner, buf, error),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match &self.state {
            UpgradeState::Available { .. } => match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirmed = true;
                    self.state = UpgradeState::Downloading {
                        progress: 0.0,
                        total_bytes: 0,
                    };
                    ModalResult::Action(ModalAction::Custom("upgrade:start".to_string()))
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => ModalResult::Close,
                _ => ModalResult::Continue,
            },
            UpgradeState::Complete { .. } => match key.code {
                KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('R') => {
                    ModalResult::Action(ModalAction::Custom("upgrade:restart".to_string()))
                }
                KeyCode::Esc => ModalResult::Close,
                _ => ModalResult::Continue,
            },
            UpgradeState::Failed { .. } | UpgradeState::UpToDate { .. } => {
                // Any key closes
                ModalResult::Close
            }
            UpgradeState::Checking
            | UpgradeState::Downloading { .. }
            | UpgradeState::Verifying
            | UpgradeState::Installing => match key.code {
                KeyCode::Esc => {
                    ModalResult::Action(ModalAction::Custom("upgrade:cancel".to_string()))
                }
                _ => ModalResult::Continue,
            },
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        match &self.state {
            UpgradeState::Available { .. } => {
                vec![("Enter/y", "install"), ("Esc/n", "cancel")]
            }
            UpgradeState::Complete { .. } => {
                vec![("r", "restart"), ("Esc", "close")]
            }
            UpgradeState::Checking
            | UpgradeState::Downloading { .. }
            | UpgradeState::Verifying
            | UpgradeState::Installing => {
                vec![("Esc", "cancel")]
            }
            _ => vec![("Enter", "close")],
        }
    }

    fn on_cancel(&mut self) -> CancelBehavior {
        CancelBehavior::Close
    }
}

impl UpgradeModal {
    fn render_checking(&self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(area);

        let msg = Line::from(vec![
            Span::styled(self.loading_indicator(), Style::default().fg(CYAN_PRIMARY)),
            Span::styled(" Checking for updates...", Style::default().fg(TEXT)),
        ]);
        let x = chunks[1].x + (chunks[1].width.saturating_sub(msg.width() as u16)) / 2;
        buf.set_line(x, chunks[1].y, &msg, chunks[1].width);
    }

    fn render_up_to_date(&self, area: Rect, buf: &mut Buffer, version: &str) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(area);

        let msg1 = Line::from(vec![Span::styled(
            "[+] Cortex is up to date!",
            Style::default().fg(GREEN),
        )]);
        let x1 = chunks[1].x + (chunks[1].width.saturating_sub(msg1.width() as u16)) / 2;
        buf.set_line(x1, chunks[1].y, &msg1, chunks[1].width);

        let msg2 = Line::from(vec![Span::styled(
            format!("Current version: {}", version),
            Style::default().fg(TEXT_DIM),
        )]);
        let x2 = chunks[2].x + (chunks[2].width.saturating_sub(msg2.width() as u16)) / 2;
        buf.set_line(x2, chunks[2].y, &msg2, chunks[2].width);
    }

    fn render_available(
        &self,
        area: Rect,
        buf: &mut Buffer,
        current: &str,
        new: &str,
        changelog: Option<&str>,
    ) {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

        // Header
        let header = Line::from(vec![Span::styled(
            "Update Available!",
            Style::default().fg(WARNING),
        )]);
        let x = chunks[1].x + (chunks[1].width.saturating_sub(header.width() as u16)) / 2;
        buf.set_line(x, chunks[1].y, &header, chunks[1].width);

        // Version info
        let version_line = Line::from(vec![
            Span::styled(current, Style::default().fg(TEXT_DIM)),
            Span::styled(" → ", Style::default().fg(TEXT_MUTED)),
            Span::styled(new, Style::default().fg(GREEN)),
        ]);
        let vx = chunks[3].x + (chunks[3].width.saturating_sub(version_line.width() as u16)) / 2;
        buf.set_line(vx, chunks[3].y, &version_line, chunks[3].width);

        // Changelog preview (if available)
        if let Some(log) = changelog {
            let preview: String = log.chars().take(chunks[5].width as usize * 2).collect();
            buf.set_string(
                chunks[5].x,
                chunks[5].y,
                &preview,
                Style::default().fg(TEXT_MUTED),
            );
        }

        // Action bar
        let bar = ActionBar::new()
            .hint("Enter/y", "install")
            .hint("Esc/n", "cancel");
        bar.render(chunks[6], buf);
    }

    fn render_downloading(&self, area: Rect, buf: &mut Buffer, progress: f64, total_bytes: u64) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(area);

        // Message
        let msg = Line::from(vec![Span::styled(
            "Downloading update...",
            Style::default().fg(TEXT),
        )]);
        let x = chunks[1].x + (chunks[1].width.saturating_sub(msg.width() as u16)) / 2;
        buf.set_line(x, chunks[1].y, &msg, chunks[1].width);

        // Progress bar area
        if chunks[3].width > 10 {
            let gauge_area = Rect {
                x: chunks[3].x + 2,
                y: chunks[3].y,
                width: chunks[3].width.saturating_sub(4),
                height: 1,
            };

            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(CYAN_PRIMARY).bg(SURFACE_1))
                .ratio(progress.clamp(0.0, 1.0));
            gauge.render(gauge_area, buf);

            // Percentage and size
            let percent = (progress * 100.0) as u32;
            let size_mb = total_bytes as f64 / 1_000_000.0;
            let info = if total_bytes > 0 {
                format!("{}% ({:.1} MB)", percent, size_mb)
            } else {
                format!("{}%", percent)
            };
            let info_line = Line::from(vec![Span::styled(info, Style::default().fg(TEXT_DIM))]);
            let ix = chunks[3].x + (chunks[3].width.saturating_sub(info_line.width() as u16)) / 2;
            buf.set_line(ix, chunks[3].y + 1, &info_line, chunks[3].width);
        }
    }

    fn render_verifying(&self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(area);

        let msg = Line::from(vec![
            Span::styled(self.loading_indicator(), Style::default().fg(CYAN_PRIMARY)),
            Span::styled(" Verifying download...", Style::default().fg(TEXT)),
        ]);
        let x = chunks[1].x + (chunks[1].width.saturating_sub(msg.width() as u16)) / 2;
        buf.set_line(x, chunks[1].y, &msg, chunks[1].width);
    }

    fn render_installing(&self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(area);

        let msg = Line::from(vec![
            Span::styled(self.loading_indicator(), Style::default().fg(CYAN_PRIMARY)),
            Span::styled(" Installing update...", Style::default().fg(TEXT)),
        ]);
        let x = chunks[1].x + (chunks[1].width.saturating_sub(msg.width() as u16)) / 2;
        buf.set_line(x, chunks[1].y, &msg, chunks[1].width);
    }

    fn render_complete(&self, area: Rect, buf: &mut Buffer, new_version: &str) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

        let msg1 = Line::from(vec![Span::styled(
            "[+] Update installed successfully!",
            Style::default().fg(GREEN),
        )]);
        let x1 = chunks[1].x + (chunks[1].width.saturating_sub(msg1.width() as u16)) / 2;
        buf.set_line(x1, chunks[1].y, &msg1, chunks[1].width);

        let msg2 = Line::from(vec![Span::styled(
            format!("New version: {}", new_version),
            Style::default().fg(TEXT),
        )]);
        let x2 = chunks[2].x + (chunks[2].width.saturating_sub(msg2.width() as u16)) / 2;
        buf.set_line(x2, chunks[2].y, &msg2, chunks[2].width);

        let msg3 = Line::from(vec![Span::styled(
            "Restart Cortex to use the new version.",
            Style::default().fg(TEXT_DIM),
        )]);
        let x3 = chunks[3].x + (chunks[3].width.saturating_sub(msg3.width() as u16)) / 2;
        buf.set_line(x3, chunks[3].y, &msg3, chunks[3].width);

        // Action bar
        let bar = ActionBar::new()
            .hint("r", "restart now")
            .hint("Esc", "close");
        bar.render(chunks[5], buf);
    }

    fn render_failed(&self, area: Rect, buf: &mut Buffer, error: &str) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(area);

        let msg1 = Line::from(vec![Span::styled(
            "[!] Update failed",
            Style::default().fg(RED),
        )]);
        let x1 = chunks[1].x + (chunks[1].width.saturating_sub(msg1.width() as u16)) / 2;
        buf.set_line(x1, chunks[1].y, &msg1, chunks[1].width);

        // Truncate error if needed
        let max_len = chunks[2].width as usize - 4;
        let error_display = if error.len() > max_len {
            format!("{}...", &error[..max_len - 3])
        } else {
            error.to_string()
        };
        let msg2 = Line::from(vec![Span::styled(
            error_display,
            Style::default().fg(TEXT_DIM),
        )]);
        let x2 = chunks[2].x + (chunks[2].width.saturating_sub(msg2.width() as u16)) / 2;
        buf.set_line(x2, chunks[2].y, &msg2, chunks[2].width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upgrade_modal_creation() {
        let modal = UpgradeModal::new();
        assert!(matches!(modal.state, UpgradeState::Checking));
    }

    #[test]
    fn test_state_transitions() {
        let mut modal = UpgradeModal::new();

        modal.set_state(UpgradeState::Available {
            current_version: "0.1.0".to_string(),
            new_version: "0.2.0".to_string(),
            changelog: None,
        });
        assert!(matches!(modal.state, UpgradeState::Available { .. }));

        modal.set_progress(0.5, 1_000_000);
        assert!(matches!(modal.state, UpgradeState::Downloading { .. }));

        modal.set_complete("0.2.0".to_string());
        assert!(matches!(modal.state, UpgradeState::Complete { .. }));
    }

    #[test]
    fn test_key_handling_available() {
        let mut modal = UpgradeModal::with_state(UpgradeState::Available {
            current_version: "0.1.0".to_string(),
            new_version: "0.2.0".to_string(),
            changelog: None,
        });

        // Enter should start upgrade
        let result = modal.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(
            matches!(result, ModalResult::Action(ModalAction::Custom(ref s)) if s == "upgrade:start")
        );
    }

    #[test]
    fn test_key_handling_complete() {
        let mut modal = UpgradeModal::with_state(UpgradeState::Complete {
            new_version: "0.2.0".to_string(),
        });

        // R should trigger restart
        let result = modal.handle_key(KeyEvent::from(KeyCode::Char('r')));
        assert!(
            matches!(result, ModalResult::Action(ModalAction::Custom(ref s)) if s == "upgrade:restart")
        );
    }

    #[test]
    fn test_loading_indicator() {
        let mut modal = UpgradeModal::new();
        assert_eq!(modal.loading_indicator(), "|");
        modal.tick();
        assert_eq!(modal.loading_indicator(), "/");
    }
}
