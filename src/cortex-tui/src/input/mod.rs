//! Input handling module for keyboard and mouse
//!
//! This module provides abstractions for handling user input events,
//! including mouse events with click zone management for interactive
//! UI elements.
//!
//! # Overview
//!
//! The input system consists of:
//! - `MouseHandler`: Processes raw crossterm mouse events into high-level actions
//! - `ClickZoneRegistry`: Manages clickable regions updated each frame
//! - `ClickZoneId`: Identifies specific UI elements for click handling
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::input::{MouseHandler, ClickZoneRegistry, ClickZoneId};
//! use ratatui::layout::Rect;
//!
//! let mut mouse_handler = MouseHandler::new();
//! let mut zones = ClickZoneRegistry::new();
//!
//! // During render, register click zones
//! zones.clear();
//! zones.register(ClickZoneId::Sidebar, sidebar_rect);
//! zones.register(ClickZoneId::ChatArea, chat_rect);
//!
//! // When handling mouse events
//! if let Some(action) = mouse_handler.handle(mouse_event) {
//!     if let MouseAction::Click { x, y, button } = action {
//!         if let Some(zone_id) = zones.find(x, y) {
//!             // Handle click on specific zone
//!         }
//!     }
//! }
//! ```

pub mod mouse;
pub mod zones;

pub use mouse::{MouseAction, MouseButton, MouseHandler};
pub use zones::{ClickZone, ClickZoneId, ClickZoneRegistry};
