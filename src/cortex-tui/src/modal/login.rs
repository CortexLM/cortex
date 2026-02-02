//! Login Modal - Device Code OAuth flow with formatted display
//!
//! This modal provides a user-friendly interface for the device code authentication
//! flow, displaying the verification URL and user code prominently.

use super::{CancelBehavior, Modal, ModalAction, ModalResult};
use crate::widgets::ActionBar;
use cortex_core::style::{BORDER, CYAN_PRIMARY, GREEN, RED, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Widget},
};

/// State of the login process
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginState {
    /// Waiting for user to enter code on website
    Pending,
    /// Polling for authentication completion
    Polling,
    /// Login completed successfully
    Success(String), // username
    /// Login failed with error
    Failed(String), // error message
    /// Login was cancelled
    Cancelled,
}

/// Modal for device code authentication flow
pub struct LoginModal {
    /// The verification URL to visit
    pub verification_url: String,
    /// The user code to enter
    pub user_code: String,
    /// Seconds until code expires
    pub expires_in_secs: u64,
    /// Current state of login
    pub state: LoginState,
    /// Animation frame for loading indicator
    animation_frame: usize,
}

impl LoginModal {
    /// Create a new login modal with device code information
    pub fn new(verification_url: String, user_code: String, expires_in_secs: u64) -> Self {
        Self {
            verification_url,
            user_code,
            expires_in_secs,
            state: LoginState::Pending,
            animation_frame: 0,
        }
    }

    /// Update the state to success
    pub fn set_success(&mut self, username: String) {
        self.state = LoginState::Success(username);
    }

    /// Update the state to failed
    pub fn set_failed(&mut self, error: String) {
        self.state = LoginState::Failed(error);
    }

    /// Update the state to polling
    pub fn set_polling(&mut self) {
        self.state = LoginState::Polling;
    }

    /// Advance animation frame
    pub fn tick(&mut self) {
        self.animation_frame = (self.animation_frame + 1) % 4;
    }

    /// Get the loading indicator character
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

impl Modal for LoginModal {
    fn title(&self) -> &str {
        "Login to Cortex"
    }

    fn desired_height(&self, _max_height: u16, _width: u16) -> u16 {
        14
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Clear background
        Clear.render(area, buf);

        // Border color based on state
        let border_color = match &self.state {
            LoginState::Success(_) => GREEN,
            LoginState::Failed(_) => RED,
            _ => CYAN_PRIMARY,
        };

        let block = Block::default()
            .title(" Login to Cortex ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(SURFACE_1))
            .padding(Padding::horizontal(2));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 8 || inner.width < 30 {
            return;
        }

        // Layout
        let chunks = Layout::vertical([
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // URL label
            Constraint::Length(1), // URL
            Constraint::Length(1), // Spacer
            Constraint::Length(3), // Code box
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Status
            Constraint::Length(1), // Spacer
            Constraint::Min(1),    // Action bar
        ])
        .split(inner);

        // Render based on state
        match &self.state {
            LoginState::Pending | LoginState::Polling => {
                self.render_pending(chunks, buf);
            }
            LoginState::Success(username) => {
                self.render_success(chunks, buf, username);
            }
            LoginState::Failed(error) => {
                self.render_failed(chunks, buf, error);
            }
            LoginState::Cancelled => {
                // Just close
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match &self.state {
            LoginState::Success(_) => {
                // Any key closes on success
                ModalResult::Close
            }
            LoginState::Failed(_) => {
                // Any key closes on failure
                ModalResult::Close
            }
            LoginState::Pending | LoginState::Polling => match key.code {
                KeyCode::Esc => {
                    self.state = LoginState::Cancelled;
                    ModalResult::Action(ModalAction::Custom("login:cancel".to_string()))
                }
                KeyCode::Enter | KeyCode::Char('c') | KeyCode::Char('C') => {
                    // Copy code to clipboard
                    ModalResult::Action(ModalAction::Custom(format!(
                        "clipboard:{}",
                        self.user_code
                    )))
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    // Open browser
                    ModalResult::Action(ModalAction::Custom(format!(
                        "open_url:{}",
                        self.verification_url
                    )))
                }
                _ => ModalResult::Continue,
            },
            LoginState::Cancelled => ModalResult::Close,
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        match &self.state {
            LoginState::Pending | LoginState::Polling => {
                vec![
                    ("Enter/c", "copy code"),
                    ("o", "open browser"),
                    ("Esc", "cancel"),
                ]
            }
            LoginState::Success(_) | LoginState::Failed(_) => {
                vec![("Enter", "close")]
            }
            LoginState::Cancelled => vec![],
        }
    }

    fn on_cancel(&mut self) -> CancelBehavior {
        self.state = LoginState::Cancelled;
        CancelBehavior::Close
    }
}

impl LoginModal {
    fn render_pending(&self, chunks: std::rc::Rc<[Rect]>, buf: &mut Buffer) {
        // URL label
        let url_label = Line::from(vec![Span::styled(
            "Visit this URL to authenticate:",
            Style::default().fg(TEXT_DIM),
        )]);
        buf.set_line(chunks[1].x, chunks[1].y, &url_label, chunks[1].width);

        // URL (truncate if needed)
        let url_display = if self.verification_url.len() > chunks[2].width as usize - 2 {
            format!(
                "{}...",
                &self.verification_url[..chunks[2].width as usize - 5]
            )
        } else {
            self.verification_url.clone()
        };
        let url_line = Line::from(vec![Span::styled(
            url_display,
            Style::default().fg(CYAN_PRIMARY),
        )]);
        buf.set_line(chunks[2].x, chunks[2].y, &url_line, chunks[2].width);

        // Code box
        self.render_code_box(chunks[4], buf);

        // Status
        let status_text = if matches!(self.state, LoginState::Polling) {
            format!("{} Waiting for authentication...", self.loading_indicator())
        } else {
            "Enter the code above on the website".to_string()
        };
        let status_line = Line::from(vec![Span::styled(
            status_text,
            Style::default().fg(TEXT_MUTED),
        )]);
        // Center the status
        let status_x =
            chunks[6].x + (chunks[6].width.saturating_sub(status_line.width() as u16)) / 2;
        buf.set_line(status_x, chunks[6].y, &status_line, chunks[6].width);

        // Action bar
        let bar = ActionBar::new()
            .hint("Enter/c", "copy code")
            .hint("o", "open browser")
            .hint("Esc", "cancel");
        bar.render(chunks[8], buf);
    }

    fn render_code_box(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 20 {
            return;
        }

        let code_len = self.user_code.len() as u16;
        let box_width = code_len + 6;
        let box_x = area.x + (area.width.saturating_sub(box_width)) / 2;

        // Top border
        let top = format!("┌{}┐", "─".repeat(box_width as usize - 2));
        buf.set_string(box_x, area.y, &top, Style::default().fg(BORDER));

        // Code line with padding
        let _code_line = format!("│  {}  │", self.user_code);
        buf.set_string(box_x, area.y + 1, "│", Style::default().fg(BORDER));
        buf.set_string(
            box_x + 1,
            area.y + 1,
            format!("  {}  ", self.user_code),
            Style::default().fg(CYAN_PRIMARY).bg(SURFACE_1),
        );
        buf.set_string(
            box_x + box_width - 1,
            area.y + 1,
            "│",
            Style::default().fg(BORDER),
        );

        // Bottom border
        let bottom = format!("└{}┘", "─".repeat(box_width as usize - 2));
        buf.set_string(box_x, area.y + 2, &bottom, Style::default().fg(BORDER));
    }

    fn render_success(&self, chunks: std::rc::Rc<[Rect]>, buf: &mut Buffer, username: &str) {
        // Success message centered
        let msg1 = Line::from(vec![Span::styled(
            "[+] Login successful!",
            Style::default().fg(GREEN),
        )]);
        let msg1_x = chunks[2].x + (chunks[2].width.saturating_sub(msg1.width() as u16)) / 2;
        buf.set_line(msg1_x, chunks[2].y, &msg1, chunks[2].width);

        let msg2 = Line::from(vec![Span::styled(
            format!("Welcome, {}", username),
            Style::default().fg(TEXT),
        )]);
        let msg2_x = chunks[4].x + (chunks[4].width.saturating_sub(msg2.width() as u16)) / 2;
        buf.set_line(msg2_x, chunks[4].y, &msg2, chunks[4].width);

        // Action bar
        let bar = ActionBar::new().hint("Enter", "close");
        bar.render(chunks[8], buf);
    }

    fn render_failed(&self, chunks: std::rc::Rc<[Rect]>, buf: &mut Buffer, error: &str) {
        // Error message centered
        let msg1 = Line::from(vec![Span::styled(
            "[!] Login failed",
            Style::default().fg(RED),
        )]);
        let msg1_x = chunks[2].x + (chunks[2].width.saturating_sub(msg1.width() as u16)) / 2;
        buf.set_line(msg1_x, chunks[2].y, &msg1, chunks[2].width);

        // Truncate error if needed
        let error_display = if error.len() > chunks[4].width as usize - 4 {
            format!("{}...", &error[..chunks[4].width as usize - 7])
        } else {
            error.to_string()
        };
        let msg2 = Line::from(vec![Span::styled(
            error_display,
            Style::default().fg(TEXT_DIM),
        )]);
        let msg2_x = chunks[4].x + (chunks[4].width.saturating_sub(msg2.width() as u16)) / 2;
        buf.set_line(msg2_x, chunks[4].y, &msg2, chunks[4].width);

        // Action bar
        let bar = ActionBar::new().hint("Enter", "close");
        bar.render(chunks[8], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_modal_creation() {
        let modal = LoginModal::new(
            "https://auth.cortex.foundation/device".to_string(),
            "ABCD1234".to_string(),
            900,
        );
        assert_eq!(modal.user_code, "ABCD1234");
        assert!(matches!(modal.state, LoginState::Pending));
    }

    #[test]
    fn test_state_transitions() {
        let mut modal = LoginModal::new(
            "https://example.com".to_string(),
            "CODE123".to_string(),
            600,
        );

        modal.set_polling();
        assert!(matches!(modal.state, LoginState::Polling));

        modal.set_success("testuser".to_string());
        assert!(matches!(modal.state, LoginState::Success(_)));

        let mut modal2 = LoginModal::new("url".to_string(), "code".to_string(), 300);
        modal2.set_failed("Connection error".to_string());
        assert!(matches!(modal2.state, LoginState::Failed(_)));
    }

    #[test]
    fn test_key_handling_pending() {
        let mut modal = LoginModal::new("url".to_string(), "CODE".to_string(), 300);

        // ESC should cancel
        let result = modal.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(
            matches!(result, ModalResult::Action(ModalAction::Custom(ref s)) if s == "login:cancel")
        );
    }

    #[test]
    fn test_key_handling_success() {
        let mut modal = LoginModal::new("url".to_string(), "CODE".to_string(), 300);
        modal.set_success("user".to_string());

        // Any key should close
        let result = modal.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_loading_indicator() {
        let mut modal = LoginModal::new("url".to_string(), "CODE".to_string(), 300);
        assert_eq!(modal.loading_indicator(), "|");
        modal.tick();
        assert_eq!(modal.loading_indicator(), "/");
        modal.tick();
        assert_eq!(modal.loading_indicator(), "-");
        modal.tick();
        assert_eq!(modal.loading_indicator(), "\\");
        modal.tick();
        assert_eq!(modal.loading_indicator(), "|"); // Wraps around
    }
}
