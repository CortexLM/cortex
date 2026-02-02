//! Key action mapping system for cortex-tui.
//!
//! This module defines all possible user actions, their contexts, and the mapping
//! from key events to actions. It provides a flexible keybinding system that supports
//! context-aware key mappings.
//!
//! # Module Structure
//!
//! - [`key_action`] - The `KeyAction` enum defining all possible user actions
//! - [`context`] - The `ActionContext` enum for context-aware key mappings
//! - [`binding`] - The `KeyBinding` struct for individual key bindings
//! - [`mapper`] - The `ActionMapper` for looking up actions from key events
//! - [`key_utils`] - Utility functions for parsing and formatting key events

mod binding;
mod context;
mod key_action;
mod key_utils;
mod mapper;

// Re-export all public types and functions for backwards compatibility
pub use binding::KeyBinding;
pub use context::ActionContext;
pub use key_action::KeyAction;
pub use key_utils::{format_key, parse_key_string};
pub use mapper::ActionMapper;
