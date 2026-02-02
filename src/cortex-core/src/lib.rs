#![allow(warnings, clippy::all)]
//! # Cortex Core
//!
//! High-performance TUI engine with game-loop architecture for Cortex CLI.
//!
//! This crate provides the foundational components for building smooth,
//! 120 FPS terminal user interfaces with a game-engine style architecture.
//!
//! ## Core Components
//!
//! - **Frame Engine**: A 120 FPS game loop using `tokio::select!` for
//!   concurrent tick, input, and event handling.
//! - **Events**: Action-based event system with key mapping support.
//! - **Style**: The Cortex visual identity - a single cohesive theme
//!   with pink/purple/blue brand colors.
//! - **Animation**: Smooth animation primitives (pulse, typewriter, fade, spinner).
//! - **Widgets**: Reusable UI components (brain, chat, input, etc.).
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      Application                            │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
//! │  │   Views     │  │   State     │  │    Event Handlers   │  │
//! │  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘  │
//! └─────────┼────────────────┼───────────────────┼──────────────┘
//!           │                │                   │
//! ┌─────────▼────────────────▼───────────────────▼──────────────┐
//! │                     Cortex Core                             │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
//! │  │ FrameEngine │  │   Events    │  │      Widgets        │  │
//! │  │  (120 FPS)  │  │   (mpsc)    │  │  (brain,chat,etc)   │  │
//! │  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘  │
//! │         │                │                    │             │
//! │  ┌──────▼────────────────▼────────────────────▼──────────┐  │
//! │  │                    Style + Animation                  │  │
//! │  │         (Cortex theme, pulse, typewriter)             │  │
//! │  └───────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//! ┌────────────────────────────▼────────────────────────────────┐
//! │                    Ratatui + Crossterm                      │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use cortex_engine::{
//!     frame_engine::{FrameEngine, EngineEvent, create_event_channel},
//!     events::{Action, EventBus, DefaultKeyMapper, KeyMapper},
//!     style::CortexStyle,
//!     animation::Pulse,
//! };
//! use std::sync::Arc;
//! use std::sync::atomic::AtomicBool;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create event channel and running flag
//!     let (event_tx, mut event_rx) = create_event_channel();
//!     let running = Arc::new(AtomicBool::new(true));
//!
//!     // Create and spawn the frame engine
//!     let mut engine = FrameEngine::new(event_tx, running.clone());
//!     tokio::spawn(async move {
//!         engine.run().await
//!     });
//!
//!     // Create key mapper
//!     let key_mapper = DefaultKeyMapper::new();
//!
//!     // Main event loop
//!     while let Some(event) = event_rx.recv().await {
//!         match event {
//!             EngineEvent::Key(key) => {
//!                 let action = key_mapper.map_key(key);
//!                 if action == Action::Quit {
//!                     break;
//!                 }
//!             }
//!             EngineEvent::Tick(_) => {
//!                 // Update animations, render frame
//!             }
//!             _ => {}
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```

// Module declarations
pub mod animation;
pub mod events;
pub mod frame_engine;
pub mod markdown;
pub mod progress;
pub mod style;
pub mod widgets;

// Re-export commonly used types for convenience
pub use animation::{
    ElapsedTimer, Fade, FadeDirection, ProgressBar, Pulse, Spinner, SpinnerFrames, SpinnerType,
    TokenCounter, Typewriter, interpolate_color,
};
pub use events::{Action, DefaultKeyMapper, EventBus, InputAction, KeyMapper};
pub use frame_engine::{
    DEFAULT_CHANNEL_BUFFER, DEFAULT_TICK_RATE_MS, EngineEvent, FrameEngine, FrameEngineBuilder,
    create_event_channel, create_event_channel_with_capacity,
};
pub use progress::{
    ProgressEmitter, ProgressEvent, ProgressSubscriber, TaskResult, TodoItem, TodoStatus,
    ToolResult,
};
pub use style::{
    BLUE, BORDER, BORDER_HIGHLIGHT, CortexStyle, GREEN, ORANGE, PINK, PURPLE, RED, SURFACE_0,
    SURFACE_1, SURFACE_2, TEXT, TEXT_DIM, TEXT_MUTED, VOID, YELLOW,
};

/// Cortex Core version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default frames per second for the engine
pub const DEFAULT_FPS: u32 = 120;
