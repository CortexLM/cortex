//! KeyBinding - A single key binding.

use crossterm::event::KeyEvent;

use super::{ActionContext, KeyAction};

/// A key binding that maps a key event to an action in a specific context.
#[derive(Debug, Clone)]
pub struct KeyBinding {
    /// The key event that triggers this binding.
    pub key: KeyEvent,
    /// The action to perform.
    pub action: KeyAction,
    /// The context in which this binding is active.
    pub context: ActionContext,
    /// Human-readable description of the binding.
    pub description: &'static str,
}

impl KeyBinding {
    /// Create a new key binding.
    pub fn new(
        key: KeyEvent,
        action: KeyAction,
        context: ActionContext,
        description: &'static str,
    ) -> Self {
        Self {
            key,
            action,
            context,
            description,
        }
    }

    /// Create a global key binding.
    pub fn global(key: KeyEvent, action: KeyAction, description: &'static str) -> Self {
        Self::new(key, action, ActionContext::Global, description)
    }

    /// Create an input context key binding.
    pub fn input(key: KeyEvent, action: KeyAction, description: &'static str) -> Self {
        Self::new(key, action, ActionContext::Input, description)
    }

    /// Create a chat context key binding.
    pub fn chat(key: KeyEvent, action: KeyAction, description: &'static str) -> Self {
        Self::new(key, action, ActionContext::Chat, description)
    }

    /// Create a sidebar context key binding.
    pub fn sidebar(key: KeyEvent, action: KeyAction, description: &'static str) -> Self {
        Self::new(key, action, ActionContext::Sidebar, description)
    }

    /// Create an approval context key binding.
    pub fn approval(key: KeyEvent, action: KeyAction, description: &'static str) -> Self {
        Self::new(key, action, ActionContext::Approval, description)
    }

    /// Create a help context key binding.
    pub fn help(key: KeyEvent, action: KeyAction, description: &'static str) -> Self {
        Self::new(key, action, ActionContext::Help, description)
    }
}
