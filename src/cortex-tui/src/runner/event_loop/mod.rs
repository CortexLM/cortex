//! Main event loop for Cortex TUI.
//!
//! This module provides the heart of the application - the main event loop that
//! coordinates the 120 FPS render loop with cortex-core backend events using
//! `tokio::select!`.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                         EventLoop                                    │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌────────────┐  │
//! │  │ FrameEngine │  │SessionBridge│  │StreamControl│  │ActionMapper│  │
//! │  │  (120 FPS)  │  │  (Backend)  │  │  (State)    │  │ (Bindings) │  │
//! │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └──────┬─────┘  │
//! │         │                │                │                │        │
//! │         │    tokio::select!              │                │        │
//! │         ▼                ▼                ▼                ▼        │
//! │  ┌──────────────────────────────────────────────────────────────┐   │
//! │  │                    Main Event Loop                           │   │
//! │  │  • Engine events (ticks, keys, mouse, resize)                │   │
//! │  │  • Backend events (streaming, tools, errors)                 │   │
//! │  │  • Action dispatch and state updates                         │   │
//! │  │  • View rendering                                            │   │
//! │  └──────────────────────────────────────────────────────────────┘   │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```

mod actions;
mod auth;
mod commands;
mod core;
mod input;
mod modal;
mod mouse;
mod rendering;
mod streaming;
mod subagent;
mod tools;

#[cfg(test)]
mod tests;

// Re-export main types
pub use self::core::{EventLoop, PendingToolCall};

// Re-export helpers that might be needed externally
pub use self::core::open_browser_url;
pub use self::core::simplify_error_message;
