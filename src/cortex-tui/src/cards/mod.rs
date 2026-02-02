//! Card view system for modal dialogs and overlays.
//!
//! Cards are stackable modal views that can be pushed onto a CardStack
//! and handle their own input and rendering.

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::path::PathBuf;

// Card modules
pub mod commands;
pub mod help;
pub mod mcp;
pub mod models;
// pub mod providers; // REMOVED (single Cortex provider)
pub mod sessions;

// Future card modules
// pub mod settings;
// pub mod context;
// pub mod export;

pub use commands::{CommandCategory, CommandEntry, CommandsCard};
pub use help::{HelpCard, HelpItem, HelpSection};
pub use mcp::{McpCard, McpServerInfo, McpStatus};
pub use models::{ModelInfo, ModelsCard};
// pub use providers::{ProviderInfo, ProvidersCard, known_providers}; // REMOVED (single Cortex provider)
pub use sessions::{SessionInfo, SessionsCard};

/// Result of handling input in a card.
pub enum CardResult {
    /// Stay on current card
    Continue,
    /// Close the card
    Close,
    /// Perform an action
    Action(CardAction),
    /// Replace with another card
    Replace(Box<dyn CardView>),
}

/// Actions that can be triggered by cards.
pub enum CardAction {
    /// Select a model by name
    SelectModel(String),
    /// Select a provider by name
    SelectProvider(String),
    /// Select a session by path
    SelectSession(PathBuf),
    /// Restart an MCP server by name
    RestartMcpServer(String),
    /// Add an MCP server by name/config
    AddMcpServer(String),
    /// Remove an MCP server by name
    RemoveMcpServer(String),
    /// Update a setting
    UpdateSetting { key: String, value: String },
    /// Export session to a file
    ExportSession { format: String, path: PathBuf },
    /// Add a file to context
    AddContextFile(PathBuf),
    /// Remove a file from context
    RemoveContextFile(PathBuf),
    /// Execute a command
    ExecuteCommand(String),
    /// Custom action with arbitrary data
    Custom(String),
}

/// Result of handling a cancellation event (Escape key).
pub enum CancellationEvent {
    /// The card handled the cancellation (e.g., closed a submenu)
    Handled,
    /// The card did not handle it (should close the card)
    NotHandled,
}

/// Trait for modal card views.
///
/// Cards are stackable modal dialogs that can handle their own input
/// and rendering. They can return actions to be processed by the main
/// application or replace themselves with other cards.
pub trait CardView: Send {
    /// Returns the title of the card (displayed in the header).
    fn title(&self) -> &str;

    /// Returns the desired height for the card given constraints.
    ///
    /// # Arguments
    /// * `max_height` - Maximum available height
    /// * `width` - Available width (may affect height for wrapped content)
    fn desired_height(&self, max_height: u16, width: u16) -> u16;

    /// Render the card content to the buffer.
    ///
    /// # Arguments
    /// * `area` - The area to render into (inside the card border)
    /// * `buf` - The buffer to render to
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Handle a key event.
    ///
    /// # Arguments
    /// * `key` - The key event to handle
    ///
    /// # Returns
    /// A `CardResult` indicating what should happen next.
    fn handle_key(&mut self, key: KeyEvent) -> CardResult;

    /// Returns key hints to display at the bottom of the card.
    ///
    /// Each tuple is (key, description), e.g., ("Enter", "Select").
    fn key_hints(&self) -> Vec<(&'static str, &'static str)>;

    /// Handle cancellation (Escape key).
    ///
    /// Override this to handle Escape for internal state (e.g., closing
    /// a dropdown) before the card itself is closed.
    ///
    /// # Returns
    /// `CancellationEvent::Handled` if the card handled the cancellation,
    /// `CancellationEvent::NotHandled` if the card should be closed.
    fn on_cancel(&mut self) -> CancellationEvent {
        CancellationEvent::Handled
    }

    /// Returns whether the card has completed its purpose.
    ///
    /// Used to determine if the card should auto-close after an action.
    fn is_complete(&self) -> bool;

    /// Returns whether the card supports search/filtering.
    fn is_searchable(&self) -> bool {
        false
    }

    /// Returns the placeholder text for the search input.
    fn search_placeholder(&self) -> Option<&str> {
        None
    }
}

/// A stack of card views for managing modal overlays.
pub struct CardStack {
    stack: Vec<Box<dyn CardView>>,
}

impl CardStack {
    /// Create a new empty card stack.
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a card onto the stack.
    pub fn push(&mut self, card: Box<dyn CardView>) {
        self.stack.push(card);
    }

    /// Pop the top card from the stack.
    ///
    /// # Returns
    /// The popped card, or `None` if the stack was empty.
    pub fn pop(&mut self) -> Option<Box<dyn CardView>> {
        self.stack.pop()
    }

    /// Check if the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Check if a card is currently active (stack is not empty).
    pub fn is_active(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Get a reference to the current (top) card.
    ///
    /// # Returns
    /// A reference to the top card, or `None` if the stack is empty.
    pub fn current(&self) -> Option<&dyn CardView> {
        self.stack.last().map(|c| c.as_ref())
    }

    /// Get a mutable reference to the current (top) card.
    ///
    /// # Returns
    /// A mutable reference to the top card, or `None` if the stack is empty.
    pub fn current_mut(&mut self) -> Option<&mut Box<dyn CardView>> {
        self.stack.last_mut()
    }

    /// Handle a key event for the current card.
    ///
    /// # Arguments
    /// * `key` - The key event to handle
    ///
    /// # Returns
    /// The `CardResult` from the current card, or `CardResult::Continue`
    /// if the stack is empty.
    pub fn handle_key(&mut self, key: KeyEvent) -> CardResult {
        if let Some(card) = self.stack.last_mut() {
            let result = card.handle_key(key);

            match result {
                CardResult::Close => {
                    self.stack.pop();
                    CardResult::Continue
                }
                CardResult::Replace(new_card) => {
                    self.stack.pop();
                    self.stack.push(new_card);
                    CardResult::Continue
                }
                other => other,
            }
        } else {
            CardResult::Continue
        }
    }

    /// Render the current card.
    ///
    /// # Arguments
    /// * `area` - The area to render into
    /// * `buf` - The buffer to render to
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if let Some(card) = self.stack.last() {
            card.render(area, buf);
        }
    }
}

impl Default for CardStack {
    fn default() -> Self {
        Self::new()
    }
}
