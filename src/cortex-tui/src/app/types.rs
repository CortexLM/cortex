use cortex_core::widgets::DisplayMode;

/// Re-export OperationMode from cortex_core's DisplayMode
/// This is used to track the agent operation mode (Build/Plan/Spec)
pub type OperationMode = DisplayMode;

/// The current view/screen being displayed
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AppView {
    #[default]
    Session,
    Approval,
    Questions,
    Settings,
    Help,
    /// Viewing a subagent's conversation (stores the subagent session_id)
    SubagentConversation(String),
}

/// Which UI element currently has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusTarget {
    #[default]
    Input,
    Chat,
    Sidebar,
    Modal,
}

/// Mode for tool approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    #[default]
    Ask,
    AllowSession,
    AllowAlways,
}

/// Trigger type for autocomplete
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutocompleteTrigger {
    Command,
    Mention,
}
