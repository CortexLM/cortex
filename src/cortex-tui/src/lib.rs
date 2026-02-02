//! # Cortex TUI
//!
//! Terminal user interface for Cortex AI assistant.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use cortex_tui::{run, AppState};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     run(AppState::new(), None).await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                         User                                │
//! │                           │                                 │
//! │  ┌────────────────────────▼─────────────────────────────┐  │
//! │  │                    AppRunner                          │  │
//! │  │   run() / resume() / run_demo()                       │  │
//! │  └────────────────────────┬─────────────────────────────┘  │
//! │                           │                                 │
//! │  ┌────────────────────────▼─────────────────────────────┐  │
//! │  │                    EventLoop                          │  │
//! │  │   tokio::select! { tick, keys, provider_stream }      │  │
//! │  └──────┬─────────────────┬──────────────────┬──────────┘  │
//! │         │                 │                  │              │
//! │  ┌──────▼──────┐   ┌──────▼──────┐   ┌──────▼──────────┐  │
//! │  │ FrameEngine │   │ ActionMapper│   │ProviderManager  │  │
//! │  │  (120 FPS)  │   │ (keybinds)  │   │ (API clients)   │  │
//! │  └─────────────┘   └─────────────┘   └─────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Main Components
//!
//! - [`AppRunner`] - High-level runner for starting the TUI
//! - [`EventLoop`] - Main event loop with `tokio::select!`
//! - [`AppState`] - Application state management
//! - [`ProviderManager`] - Direct API access to AI providers
//! - [`CortexSession`] - Local session management
//! - [`ActionMapper`] - Key binding configuration
//!
//! ## Features
//!
//! - 120 FPS rendering with Ocean/Cyan theme
//! - Full keyboard navigation
//! - Streaming response display with typewriter effect
//! - Multi-provider support (Cortex, Anthropic, OpenAI, etc.)
//! - Local session management (save, resume, fork, export)

// Core modules
pub mod actions;
pub mod agent;
pub mod app;
pub mod events;

// Command system (slash commands)
pub mod commands;

// Input handling (mouse, keyboard)
pub mod input;

// UI modules (new minimalist system)
pub mod cards;
pub mod modal;
pub mod ui;

// Legacy UI modules (being migrated)
pub mod views;
pub mod widgets;

// Provider management (direct API access)
pub mod providers;

// Session management (local storage)
pub mod session;

// Text selection for copy/paste
pub mod selection;

// Permission system for tool execution
pub mod permissions;

// Question prompt system
pub mod question;

// Interactive input system
pub mod interactive;

// Bridge to cortex-core (legacy, being phased out)
pub mod bridge;

// Application runner
pub mod runner;

// Backtracking system for conversation history navigation
pub mod backtrack;

// File mentions system for @ references
pub mod mentions;

// External editor support (Ctrl+G)
pub mod external_editor;

// Collaboration modes (Plan/Code/Pair/Execute)
pub mod collaboration_modes;

// TUI capture and debugging (enabled via CORTEX_TUI_CAPTURE=1)
pub mod capture;

// MCP server storage (persistent storage for MCP configurations)
pub mod mcp_storage;

// Sound notification system
pub mod sound;

// Re-export main types
pub use actions::{ActionContext, ActionMapper, KeyAction, KeyBinding};
pub use app::{
    AppState, AppView, ApprovalMode, ApprovalState, FocusTarget, SessionSummary, StreamingState,
};
pub use bridge::{SessionBridge, StreamController, StreamState, SubmissionBuilder, adapt_event};
pub use commands::{
    CommandCategory, CommandDef, CommandParser, CommandRegistry, CommandResult, Completion,
    CompletionEngine, ModalType, ParsedCommand, ViewType,
};
pub use events::{
    AppEvent, AppEventBus, AppEventSender, EventDispatcher, EventHandler, ScrollTarget,
};
pub use input::{
    ClickZone, ClickZoneId, ClickZoneRegistry, MouseAction, MouseButton, MouseHandler,
};
pub use modal::{Modal, ModalAction, ModalResult, ModalStack};
pub use providers::{CortexConfig, ModelInfo, ProviderConfig, ProviderManager};
pub use runner::{
    app_runner::{AppExitInfo, AppRunner, ExitReason, resume, run, run_demo},
    event_loop::EventLoop,
    terminal::{CortexTerminal, TerminalGuard, TerminalOptions},
};
pub use selection::TextSelection;
pub use session::{CortexSession, ExportFormat, SessionMeta, SessionStorage, StoredMessage};

// Backtracking re-exports
pub use backtrack::{
    BacktrackAction, BacktrackMode, BacktrackSelection, BacktrackState, Direction, ForkRequest,
    MessageRole, MessageSnapshot, PendingBacktrackRollback, TextElement, TextElementType,
    backtrack_hint, calculate_rollback_turns, count_user_messages,
};

// Collaboration modes re-exports
pub use collaboration_modes::{
    CollaborationModeMask, ModeKind, next_mode, presets_for_tui, prev_mode,
};

// File mentions re-exports
pub use mentions::{FileMentionState, MentionInsert};

// External editor re-exports
pub use external_editor::{
    EditorError, get_editor, open_external_editor, open_external_editor_sync,
};

// TUI capture re-exports
pub use capture::{TuiCapture, capture_enabled};

// MCP storage re-exports
pub use mcp_storage::{McpStorage, McpTransport, StoredMcpServer};

// Sound notification re-exports
pub use sound::{SoundType, play_approval_required, play_response_complete};

// Re-export cortex-core for downstream users
pub use cortex_engine;

/// Cortex TUI version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Check if the terminal supports the TUI
pub fn is_tui_supported() -> bool {
    runner::terminal::is_terminal() && runner::terminal::supports_color()
}
