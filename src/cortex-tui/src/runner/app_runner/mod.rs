//! High-level application runner for Cortex TUI.
//!
//! This module provides `AppRunner`, the main entry point for running the
//! cortex-tui application. It handles terminal initialization, session
//! management, event loop execution, and cleanup.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::runner::AppRunner;
//! use cortex_engine::Config;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = Config::load_sync(Default::default())?;
//!     let exit_info = AppRunner::new(config)
//!         .with_initial_prompt("Hello!")
//!         .run()
//!         .await?;
//!     
//!     println!("Exited with: {:?}", exit_info.exit_reason);
//!     Ok(())
//! }
//! ```
//!
//! # Architecture
//!
//! The `AppRunner` orchestrates the following components:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         AppRunner                                │
//! │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
//! │  │  CortexTerminal │  │  SessionBridge  │  │   EventLoop     │  │
//! │  │  (Terminal I/O) │  │  (Backend Comm) │  │  (Main Loop)    │  │
//! │  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘  │
//! │           │                    │                    │           │
//! │           └────────────────────┴────────────────────┘           │
//! │                              │                                   │
//! └──────────────────────────────┼───────────────────────────────────┘
//!                                │
//!                                ▼
//!                         AppExitInfo
//! ```

mod auth_status;
mod exit_info;
mod quick_start;
mod runner;
mod trusted_workspaces;

// Re-export public types for backwards compatibility
pub use exit_info::{AppExitInfo, ExitReason};
pub use quick_start::{resume, resume_cortex, run, run_demo, run_direct};
pub use runner::AppRunner;
