//! ActionContext - Context for key mappings.

use std::fmt;

/// The context in which key bindings are evaluated.
///
/// Different contexts allow the same key to have different meanings
/// depending on what the user is focused on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionContext {
    /// Global bindings, always active.
    Global,
    /// When input area is focused.
    Input,
    /// When chat area is focused.
    Chat,
    /// When sidebar is focused.
    Sidebar,
    /// When in approval modal.
    Approval,
    /// When in help view.
    Help,
}

impl fmt::Display for ActionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActionContext::Global => write!(f, "Global"),
            ActionContext::Input => write!(f, "Input"),
            ActionContext::Chat => write!(f, "Chat"),
            ActionContext::Sidebar => write!(f, "Sidebar"),
            ActionContext::Approval => write!(f, "Approval"),
            ActionContext::Help => write!(f, "Help"),
        }
    }
}

impl ActionContext {
    /// Returns all context variants.
    pub fn all() -> &'static [ActionContext] {
        &[
            ActionContext::Global,
            ActionContext::Input,
            ActionContext::Chat,
            ActionContext::Sidebar,
            ActionContext::Approval,
            ActionContext::Help,
        ]
    }
}
