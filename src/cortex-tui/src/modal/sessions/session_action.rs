//! Internal session actions for the sessions modal.

/// Internal actions that can be performed on a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAction {
    /// No action pending.
    None,
    /// Confirm a dangerous action (e.g., delete).
    Confirm(Box<SessionAction>),
    /// Delete the selected session.
    Delete,
}
