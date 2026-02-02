//! Unified Modal System for cortex-tui
//!
//! This module provides a unified interface for all modal dialogs in the TUI,
//! replacing the previous Card system with a more flexible Modal trait.

use cortex_core::style::{BORDER, CYAN_PRIMARY, TEXT_DIM, TEXT_MUTED};
use crossterm::event::KeyEvent;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
};
use std::path::PathBuf;

// Re-export modal implementations
pub mod commands;
pub mod help;
pub mod login;
pub mod mcp_manager;
pub mod models;
pub mod pickers;
pub mod providers;
pub mod sessions;
pub mod theme;
pub mod upgrade;

pub use commands::CommandsModal;
pub use help::HelpModal;
pub use login::{LoginModal, LoginState};
pub use mcp_manager::{
    McpManagerModal, McpServerInfo, McpServerSource, McpStatus, McpTransportType,
};
pub use models::{ModelInfo, ModelsModal};
pub use pickers::{ApprovalPickerModal, LogLevelPickerModal};
pub use providers::{ProviderInfo, ProvidersModal, known_providers};
pub use sessions::{SessionInfo, SessionsModal};
pub use theme::ThemeSelectorModal;
pub use upgrade::{UpgradeModal, UpgradeState};

// ============================================================================
// MODAL TRAIT
// ============================================================================

/// Unified trait for all modal dialogs.
///
/// Modals are overlay windows that capture input focus until closed.
/// They can return actions to be processed by the event loop.
pub trait Modal: Send {
    /// Title displayed in the modal header
    fn title(&self) -> &str;

    /// Calculate desired height given constraints
    fn desired_height(&self, max_height: u16, width: u16) -> u16;

    /// Render the modal content
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Handle a key event, returning the result
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult;

    /// Handle pasted text. Returns true if the paste was handled.
    /// Default implementation does nothing.
    fn handle_paste(&mut self, _text: &str) -> bool {
        false
    }

    /// Key hints to display at the bottom
    fn key_hints(&self) -> Vec<(&'static str, &'static str)>;

    /// Called when Escape is pressed. Return Handled to consume the event
    /// (e.g., to clear search), or NotHandled to close the modal.
    fn on_cancel(&mut self) -> CancelBehavior {
        CancelBehavior::Close
    }

    /// Whether this modal supports search/filtering
    fn is_searchable(&self) -> bool {
        false
    }

    /// Search placeholder text (if searchable)
    fn search_placeholder(&self) -> Option<&str> {
        None
    }
}

// ============================================================================
// MODAL RESULT
// ============================================================================

/// Result of handling a key event in a modal
pub enum ModalResult {
    /// Continue showing the current modal
    Continue,
    /// Close the modal
    Close,
    /// Perform an action and close
    Action(ModalAction),
    /// Push a new modal on top of this one
    Push(Box<dyn Modal>),
    /// Replace this modal with another
    Replace(Box<dyn Modal>),
}

/// Behavior when cancel (Escape) is pressed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelBehavior {
    /// Close the modal
    Close,
    /// Event was handled internally (e.g., cleared search)
    Handled,
}

// ============================================================================
// MODAL ACTIONS
// ============================================================================

/// Actions that can be returned by modals for processing by the event loop
#[derive(Debug, Clone)]
pub enum ModalAction {
    // MCP Server Actions
    RestartMcpServer(String),
    AddMcpServer {
        name: String,
        command: String,
        args: Vec<String>,
    },
    AddMcpServerHttp {
        name: String,
        url: String,
    },
    RemoveMcpServer(String),
    AuthMcpServer {
        name: String,
        api_key: String,
    },
    StartMcpServer(String),
    StopMcpServer(String),

    // Selection Actions
    SelectModel(String),
    SelectProvider(String),
    ConfigureProvider(String),
    SelectSession(PathBuf),

    // Configuration Actions
    SetApprovalMode(String),
    SetLogLevel(String),

    // File Context Actions
    AddContextFile(PathBuf),
    RemoveContextFile(PathBuf),

    // Command Execution
    ExecuteCommand(String),

    // Session Actions
    ExportSession {
        format: String,
        path: PathBuf,
    },
    ForkSession {
        name: Option<String>,
    },
    DeleteSession(PathBuf),
    NewSession,

    // API Key Actions
    SaveApiKey {
        provider: String,
        api_key: String,
    },

    // Generic/Custom
    Custom(String),
}

// ============================================================================
// MODAL STACK
// ============================================================================

/// Stack of modals for nested modal support
pub struct ModalStack {
    stack: Vec<Box<dyn Modal>>,
}

impl ModalStack {
    /// Create a new empty modal stack
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a modal onto the stack
    pub fn push(&mut self, modal: Box<dyn Modal>) {
        self.stack.push(modal);
    }

    /// Pop the top modal from the stack
    pub fn pop(&mut self) -> Option<Box<dyn Modal>> {
        self.stack.pop()
    }

    /// Check if any modal is active
    pub fn is_active(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Check if the stack is empty
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Get a reference to the current (top) modal
    pub fn current(&self) -> Option<&dyn Modal> {
        self.stack.last().map(|m| m.as_ref())
    }

    /// Get a mutable reference to the current (top) modal
    pub fn current_mut(&mut self) -> Option<&mut Box<dyn Modal>> {
        self.stack.last_mut()
    }

    /// Handle a key event, delegating to the top modal.
    ///
    /// Key events are always sent to the topmost (most recently pushed) modal only.
    /// This ensures that nested modals (e.g., Config -> Privacy Settings) receive
    /// events in the correct order:
    /// - Pressing Escape closes the inner modal first
    /// - Only after the inner modal is closed does Escape reach the outer modal
    ///
    /// This follows the standard z-order convention where the topmost UI element
    /// has input focus.
    pub fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        if let Some(modal) = self.current_mut() {
            let result = modal.handle_key(key);
            match result {
                ModalResult::Close => {
                    self.pop();
                    ModalResult::Continue
                }
                ModalResult::Push(new_modal) => {
                    self.push(new_modal);
                    ModalResult::Continue
                }
                ModalResult::Replace(new_modal) => {
                    self.pop();
                    self.push(new_modal);
                    ModalResult::Continue
                }
                ModalResult::Action(action) => {
                    // Close the modal after an action is returned
                    self.pop();
                    ModalResult::Action(action)
                }
                other => other,
            }
        } else {
            ModalResult::Continue
        }
    }

    /// Handle pasted text, delegating to the top modal
    /// Returns true if the paste was handled
    pub fn handle_paste(&mut self, text: &str) -> bool {
        if let Some(modal) = self.current_mut() {
            modal.handle_paste(text)
        } else {
            false
        }
    }

    /// Clear all modals from the stack
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// Get the number of modals in the stack
    pub fn len(&self) -> usize {
        self.stack.len()
    }
}

impl Default for ModalStack {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// RENDER HELPERS
// ============================================================================

/// Render a section header/separator
/// Example: "─── Anthropic ───"
pub fn render_section_header(area: Rect, buf: &mut Buffer, title: &str) {
    if area.width < 10 || area.height == 0 {
        return;
    }

    let title_len = title.len() as u16;
    let dash_len = (area.width.saturating_sub(title_len + 6)) / 2;

    let dashes_left = "─".repeat(dash_len as usize);
    let dashes_right = "─".repeat(dash_len as usize);

    let line = Line::from(vec![
        Span::styled(&dashes_left, Style::default().fg(BORDER)),
        Span::styled(format!(" {} ", title), Style::default().fg(TEXT_DIM)),
        Span::styled(&dashes_right, Style::default().fg(BORDER)),
    ]);

    buf.set_line(area.x, area.y, &line, area.width);
}

/// Render a search bar
/// Example: "> [search query here...]"
pub fn render_search_bar(area: Rect, buf: &mut Buffer, query: &str, placeholder: &str) {
    if area.height == 0 {
        return;
    }

    let icon = "> ";
    let bracket_open = "[";
    let bracket_close = "]";

    let display_text = if query.is_empty() { placeholder } else { query };

    let text_style = if query.is_empty() {
        Style::default().fg(TEXT_MUTED)
    } else {
        Style::default().fg(CYAN_PRIMARY)
    };

    let line = Line::from(vec![
        Span::raw(icon),
        Span::styled(bracket_open, Style::default().fg(BORDER)),
        Span::styled(display_text, text_style),
        Span::styled(bracket_close, Style::default().fg(BORDER)),
    ]);

    buf.set_line(area.x, area.y, &line, area.width);
}

/// Status icons for various states (ASCII-only for compatibility)
pub mod icons {
    pub const RUNNING: &str = "[*]";
    pub const STOPPED: &str = "[ ]";
    pub const STARTING: &str = "[~]";
    pub const ERROR: &str = "[!]";
    pub const SUCCESS: &str = "[+]";
    pub const FAILURE: &str = "[x]";
    pub const LOADING: &str = "...";
    pub const CURRENT: &str = "[>]";
    pub const NOT_CURRENT: &str = "[ ]";
    pub const NEW: &str = "+";
    pub const SEARCH: &str = ">";
}
