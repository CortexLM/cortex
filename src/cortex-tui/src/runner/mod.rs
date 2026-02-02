//! Runner module for TUI execution.
//!
//! This module contains the components needed to run the TUI application,
//! including terminal management, the main event loop, and the application runner.
//!
//! # Overview
//!
//! The runner module provides:
//! - `AppRunner`: High-level API for starting the TUI
//! - `CortexTerminal`: Terminal setup and management
//! - `EventLoop`: Main event processing loop using tokio::select!
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use cortex_tui::runner::{AppRunner, run};
//! use cortex_engine::Config;
//!
//! // Using AppRunner directly
//! let exit_info = AppRunner::new(config)
//!     .with_initial_prompt("Hello!")
//!     .run()
//!     .await?;
//!
//! // Or using the quick-start function
//! let exit_info = run(config, Some("Hello!".to_string())).await?;
//! ```
//!
//! # Using EventLoop Directly
//!
//! For more control over the application lifecycle, you can use the EventLoop directly:
//!
//! ```rust,ignore
//! use cortex_tui::app::AppState;
//! use cortex_tui::runner::{CortexTerminal, EventLoop};
//!
//! let mut terminal = CortexTerminal::new()?;
//! let app_state = AppState::new();
//!
//! let mut event_loop = EventLoop::new(app_state);
//! event_loop.run(&mut terminal).await?;
//! ```

pub mod app_runner;
pub mod auth_handlers;
pub mod billing_handlers;
pub mod card_handler;
pub mod event_loop;
pub mod handlers;
pub mod login_screen;
pub mod terminal;
pub mod trust_screen;

// Note: handlers is now a directory module (handlers/mod.rs) instead of a single file

// App runner exports
pub use app_runner::{AppExitInfo, AppRunner, ExitReason, resume, run, run_demo};

// Trust screen exports
pub use trust_screen::{TrustResult, TrustScreen};

// Card handler exports
pub use card_handler::CardHandler;

// Event loop exports
pub use event_loop::EventLoop;

// Handler exports
pub use handlers::ActionHandler;

// Terminal exports
pub use terminal::{
    CortexTerminal, TerminalGuard, TerminalOptions, is_terminal, restore_terminal,
    supports_256_colors, supports_color, supports_true_color, supports_unicode, terminal_size,
};
