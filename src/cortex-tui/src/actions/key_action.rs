//! KeyAction enum - All possible user actions.

use std::fmt;
use uuid::Uuid;

/// All possible actions that can be triggered by keybindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    // === Core ===
    /// Quit the application.
    Quit,
    /// Show help overlay.
    Help,
    /// Cancel current operation or close modal.
    Cancel,

    // === Navigation ===
    /// Move focus to next element.
    FocusNext,
    /// Move focus to previous element.
    FocusPrev,
    /// Focus the input area.
    FocusInput,
    /// Focus the chat area.
    FocusChat,
    /// Focus the sidebar.
    FocusSidebar,

    // === Scrolling ===
    /// Scroll up one line.
    ScrollUp,
    /// Scroll down one line.
    ScrollDown,
    /// Scroll up one page.
    ScrollPageUp,
    /// Scroll down one page.
    ScrollPageDown,
    /// Scroll to the top.
    ScrollToTop,
    /// Scroll to the bottom.
    ScrollToBottom,

    // === Input ===
    /// Submit the current input.
    Submit,
    /// Insert a new line in input.
    NewLine,
    /// Clear the input.
    Clear,
    /// Navigate to previous input in history.
    HistoryPrev,
    /// Navigate to next input in history.
    HistoryNext,

    // === View ===
    /// Toggle sidebar visibility.
    ToggleSidebar,
    /// Toggle help view.
    ToggleHelp,
    /// Toggle settings view.
    ToggleSettings,

    // === Session ===
    /// Create a new session.
    NewSession,
    /// Load a specific session by ID.
    LoadSession(Uuid),
    /// Delete the current/selected session.
    DeleteSession,
    /// Rename the current/selected session.
    RenameSession,
    /// Export the current session.
    ExportSession,

    // === Model ===
    /// Open model switcher.
    SwitchModel,
    // SwitchProvider - removed: provider is now always "cortex"

    // === Cards (new minimalist UI) ===
    /// Open command palette.
    OpenCommandPalette,
    /// Open sessions card.
    OpenSessions,
    /// Open MCP servers card.
    OpenMcp,

    // === Transcript ===
    /// View the full conversation transcript.
    ViewTranscript,

    // === Edit ===
    /// Copy selection to clipboard.
    Copy,
    /// Paste from clipboard.
    Paste,
    /// Select all text.
    SelectAll,

    // === Approval ===
    /// Approve a pending request.
    Approve,
    /// Reject a pending request.
    Reject,
    /// Approve for the current session (auto-approve this tool for the session).
    ApproveSession,
    /// Approve always (add to always-allowed list).
    ApproveAlways,
    /// Approve all pending requests.
    ApproveAll,
    /// Reject all pending requests.
    RejectAll,
    /// View the diff for a pending change.
    ViewDiff,

    // === Tools ===
    /// Cancel the current tool execution.
    CancelTool,
    /// Retry the last tool execution.
    RetryTool,
    /// Cycle through permission modes (Yolo -> Low -> Medium -> High -> Yolo)
    CyclePermissionMode,
    /// Toggle tool call details (expand/collapse)
    ToggleToolDetails,

    // === Operation Mode ===
    /// Toggle operation mode (Build -> Plan -> Spec -> Build)
    ToggleOperationMode,
    /// Approve a spec plan and transition to build
    ApproveSpec,
    /// Reject a spec plan
    RejectSpec,

    // === Context ===
    /// Add a file to the context.
    AddFile,
    /// Add a folder to the context.
    AddFolder,
    /// Clear all context.
    ClearContext,

    // === Commands ===
    /// Execute a slash command.
    ExecuteSlashCommand(String),

    // === Backtracking ===
    /// Open the backtrack overlay (rewind session).
    OpenBacktrack,
    /// Confirm backtrack selection.
    ConfirmBacktrack,
    /// Cancel backtrack and close overlay.
    CancelBacktrack,
    /// Navigate to previous message in backtrack.
    BacktrackPrev,
    /// Navigate to next message in backtrack.
    BacktrackNext,
    /// Fork session from selected backtrack point.
    ForkSession,

    // === External Editor ===
    /// Open external editor for composing prompt (Ctrl+G).
    OpenExternalEditor,

    // === None ===
    /// No action (ignore key).
    None,
}

impl fmt::Display for KeyAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Core
            KeyAction::Quit => write!(f, "quit"),
            KeyAction::Help => write!(f, "help"),
            KeyAction::Cancel => write!(f, "cancel"),

            // Navigation
            KeyAction::FocusNext => write!(f, "focus_next"),
            KeyAction::FocusPrev => write!(f, "focus_prev"),
            KeyAction::FocusInput => write!(f, "focus_input"),
            KeyAction::FocusChat => write!(f, "focus_chat"),
            KeyAction::FocusSidebar => write!(f, "focus_sidebar"),

            // Scrolling
            KeyAction::ScrollUp => write!(f, "scroll_up"),
            KeyAction::ScrollDown => write!(f, "scroll_down"),
            KeyAction::ScrollPageUp => write!(f, "scroll_page_up"),
            KeyAction::ScrollPageDown => write!(f, "scroll_page_down"),
            KeyAction::ScrollToTop => write!(f, "scroll_to_top"),
            KeyAction::ScrollToBottom => write!(f, "scroll_to_bottom"),

            // Input
            KeyAction::Submit => write!(f, "submit"),
            KeyAction::NewLine => write!(f, "new_line"),
            KeyAction::Clear => write!(f, "clear"),
            KeyAction::HistoryPrev => write!(f, "history_prev"),
            KeyAction::HistoryNext => write!(f, "history_next"),

            // View
            KeyAction::ToggleSidebar => write!(f, "toggle_sidebar"),
            KeyAction::ToggleHelp => write!(f, "toggle_help"),
            KeyAction::ToggleSettings => write!(f, "toggle_settings"),

            // Session
            KeyAction::NewSession => write!(f, "new_session"),
            KeyAction::LoadSession(id) => write!(f, "load_session:{id}"),
            KeyAction::DeleteSession => write!(f, "delete_session"),
            KeyAction::RenameSession => write!(f, "rename_session"),
            KeyAction::ExportSession => write!(f, "export_session"),

            // Model
            KeyAction::SwitchModel => write!(f, "switch_model"),
            // SwitchProvider removed: provider is now always "cortex"

            // Cards
            KeyAction::OpenCommandPalette => write!(f, "open_command_palette"),
            KeyAction::OpenSessions => write!(f, "open_sessions"),
            KeyAction::OpenMcp => write!(f, "open_mcp"),

            // Transcript
            KeyAction::ViewTranscript => write!(f, "view_transcript"),

            // Edit
            KeyAction::Copy => write!(f, "copy"),
            KeyAction::Paste => write!(f, "paste"),
            KeyAction::SelectAll => write!(f, "select_all"),

            // Approval
            KeyAction::Approve => write!(f, "approve"),
            KeyAction::Reject => write!(f, "reject"),
            KeyAction::ApproveSession => write!(f, "approve_session"),
            KeyAction::ApproveAlways => write!(f, "approve_always"),
            KeyAction::ApproveAll => write!(f, "approve_all"),
            KeyAction::RejectAll => write!(f, "reject_all"),
            KeyAction::ViewDiff => write!(f, "view_diff"),

            // Tools
            KeyAction::CancelTool => write!(f, "cancel_tool"),
            KeyAction::RetryTool => write!(f, "retry_tool"),
            KeyAction::CyclePermissionMode => write!(f, "cycle_permission_mode"),
            KeyAction::ToggleToolDetails => write!(f, "toggle_tool_details"),

            // Operation Mode
            KeyAction::ToggleOperationMode => write!(f, "toggle_operation_mode"),
            KeyAction::ApproveSpec => write!(f, "approve_spec"),
            KeyAction::RejectSpec => write!(f, "reject_spec"),

            // Context
            KeyAction::AddFile => write!(f, "add_file"),
            KeyAction::AddFolder => write!(f, "add_folder"),
            KeyAction::ClearContext => write!(f, "clear_context"),

            // Commands
            KeyAction::ExecuteSlashCommand(cmd) => write!(f, "command:{cmd}"),

            // Backtracking
            KeyAction::OpenBacktrack => write!(f, "open_backtrack"),
            KeyAction::ConfirmBacktrack => write!(f, "confirm_backtrack"),
            KeyAction::CancelBacktrack => write!(f, "cancel_backtrack"),
            KeyAction::BacktrackPrev => write!(f, "backtrack_prev"),
            KeyAction::BacktrackNext => write!(f, "backtrack_next"),
            KeyAction::ForkSession => write!(f, "fork_session"),

            // External Editor
            KeyAction::OpenExternalEditor => write!(f, "open_external_editor"),

            // None
            KeyAction::None => write!(f, "none"),
        }
    }
}

impl std::str::FromStr for KeyAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Handle parameterized actions
        if let Some(rest) = s.strip_prefix("load_session:") {
            let id = Uuid::parse_str(rest).map_err(|e| format!("Invalid UUID: {e}"))?;
            return Ok(KeyAction::LoadSession(id));
        }
        if let Some(rest) = s.strip_prefix("command:") {
            return Ok(KeyAction::ExecuteSlashCommand(rest.to_string()));
        }

        match s {
            // Core
            "quit" => Ok(KeyAction::Quit),
            "help" => Ok(KeyAction::Help),
            "cancel" => Ok(KeyAction::Cancel),

            // Navigation
            "focus_next" => Ok(KeyAction::FocusNext),
            "focus_prev" => Ok(KeyAction::FocusPrev),
            "focus_input" => Ok(KeyAction::FocusInput),
            "focus_chat" => Ok(KeyAction::FocusChat),
            "focus_sidebar" => Ok(KeyAction::FocusSidebar),

            // Scrolling
            "scroll_up" => Ok(KeyAction::ScrollUp),
            "scroll_down" => Ok(KeyAction::ScrollDown),
            "scroll_page_up" => Ok(KeyAction::ScrollPageUp),
            "scroll_page_down" => Ok(KeyAction::ScrollPageDown),
            "scroll_to_top" => Ok(KeyAction::ScrollToTop),
            "scroll_to_bottom" => Ok(KeyAction::ScrollToBottom),

            // Input
            "submit" => Ok(KeyAction::Submit),
            "new_line" => Ok(KeyAction::NewLine),
            "clear" => Ok(KeyAction::Clear),
            "history_prev" => Ok(KeyAction::HistoryPrev),
            "history_next" => Ok(KeyAction::HistoryNext),

            // View
            "toggle_sidebar" => Ok(KeyAction::ToggleSidebar),
            "toggle_help" => Ok(KeyAction::ToggleHelp),
            "toggle_settings" => Ok(KeyAction::ToggleSettings),

            // Session
            "new_session" => Ok(KeyAction::NewSession),
            "delete_session" => Ok(KeyAction::DeleteSession),
            "rename_session" => Ok(KeyAction::RenameSession),
            "export_session" => Ok(KeyAction::ExportSession),

            // Model
            "switch_model" => Ok(KeyAction::SwitchModel),
            // "switch_provider" removed: provider is now always "cortex"

            // Cards
            "open_command_palette" => Ok(KeyAction::OpenCommandPalette),
            "open_sessions" => Ok(KeyAction::OpenSessions),
            "open_mcp" => Ok(KeyAction::OpenMcp),

            // Transcript
            "view_transcript" => Ok(KeyAction::ViewTranscript),

            // Edit
            "copy" => Ok(KeyAction::Copy),
            "paste" => Ok(KeyAction::Paste),
            "select_all" => Ok(KeyAction::SelectAll),

            // Approval
            "approve" => Ok(KeyAction::Approve),
            "reject" => Ok(KeyAction::Reject),
            "approve_session" => Ok(KeyAction::ApproveSession),
            "approve_always" => Ok(KeyAction::ApproveAlways),
            "approve_all" => Ok(KeyAction::ApproveAll),
            "reject_all" => Ok(KeyAction::RejectAll),
            "view_diff" => Ok(KeyAction::ViewDiff),

            // Tools
            "cancel_tool" => Ok(KeyAction::CancelTool),
            "retry_tool" => Ok(KeyAction::RetryTool),
            "cycle_permission_mode" => Ok(KeyAction::CyclePermissionMode),
            "toggle_tool_details" => Ok(KeyAction::ToggleToolDetails),

            // Operation Mode
            "toggle_operation_mode" => Ok(KeyAction::ToggleOperationMode),
            "approve_spec" => Ok(KeyAction::ApproveSpec),
            "reject_spec" => Ok(KeyAction::RejectSpec),

            // Context
            "add_file" => Ok(KeyAction::AddFile),
            "add_folder" => Ok(KeyAction::AddFolder),
            "clear_context" => Ok(KeyAction::ClearContext),

            // Backtracking
            "open_backtrack" => Ok(KeyAction::OpenBacktrack),
            "confirm_backtrack" => Ok(KeyAction::ConfirmBacktrack),
            "cancel_backtrack" => Ok(KeyAction::CancelBacktrack),
            "backtrack_prev" => Ok(KeyAction::BacktrackPrev),
            "backtrack_next" => Ok(KeyAction::BacktrackNext),
            "fork_session" => Ok(KeyAction::ForkSession),

            // External Editor
            "open_external_editor" => Ok(KeyAction::OpenExternalEditor),

            // None
            "none" => Ok(KeyAction::None),

            _ => Err(format!("Unknown action: {s}")),
        }
    }
}

impl KeyAction {
    /// Returns a human-readable description of this action.
    pub fn description(&self) -> &'static str {
        match self {
            // Core
            KeyAction::Quit => "Quit application",
            KeyAction::Help => "Show help",
            KeyAction::Cancel => "Cancel/close",

            // Navigation
            KeyAction::FocusNext => "Focus next element",
            KeyAction::FocusPrev => "Focus previous element",
            KeyAction::FocusInput => "Focus input",
            KeyAction::FocusChat => "Focus chat",
            KeyAction::FocusSidebar => "Focus sidebar",

            // Scrolling
            KeyAction::ScrollUp => "Scroll up",
            KeyAction::ScrollDown => "Scroll down",
            KeyAction::ScrollPageUp => "Page up",
            KeyAction::ScrollPageDown => "Page down",
            KeyAction::ScrollToTop => "Scroll to top",
            KeyAction::ScrollToBottom => "Scroll to bottom",

            // Input
            KeyAction::Submit => "Submit message",
            KeyAction::NewLine => "Insert new line",
            KeyAction::Clear => "Clear input",
            KeyAction::HistoryPrev => "Previous in history",
            KeyAction::HistoryNext => "Next in history",

            // View
            KeyAction::ToggleSidebar => "Toggle sidebar",
            KeyAction::ToggleHelp => "Toggle help",
            KeyAction::ToggleSettings => "Toggle settings",

            // Session
            KeyAction::NewSession => "New session",
            KeyAction::LoadSession(_) => "Load session",
            KeyAction::DeleteSession => "Delete session",
            KeyAction::RenameSession => "Rename session",
            KeyAction::ExportSession => "Export session",

            // Model
            KeyAction::SwitchModel => "Switch model",
            // SwitchProvider removed: provider is now always "cortex"

            // Cards
            KeyAction::OpenCommandPalette => "Open command palette",
            KeyAction::OpenSessions => "Open sessions",
            KeyAction::OpenMcp => "Open MCP servers",

            // Transcript
            KeyAction::ViewTranscript => "View transcript",

            // Edit
            KeyAction::Copy => "Copy",
            KeyAction::Paste => "Paste",
            KeyAction::SelectAll => "Select all",

            // Approval
            KeyAction::Approve => "Approve",
            KeyAction::Reject => "Reject",
            KeyAction::ApproveSession => "Approve for session",
            KeyAction::ApproveAlways => "Always allow",
            KeyAction::ApproveAll => "Approve all",
            KeyAction::RejectAll => "Reject all",
            KeyAction::ViewDiff => "View diff",

            // Tools
            KeyAction::CancelTool => "Cancel tool",
            KeyAction::RetryTool => "Retry tool",
            KeyAction::CyclePermissionMode => "Cycle permission mode",
            KeyAction::ToggleToolDetails => "Toggle tool details",

            // Operation Mode
            KeyAction::ToggleOperationMode => "Toggle operation mode",
            KeyAction::ApproveSpec => "Approve spec plan",
            KeyAction::RejectSpec => "Reject spec plan",

            // Context
            KeyAction::AddFile => "Add file",
            KeyAction::AddFolder => "Add folder",
            KeyAction::ClearContext => "Clear context",

            // Commands
            KeyAction::ExecuteSlashCommand(_) => "Execute command",

            // Backtracking
            KeyAction::OpenBacktrack => "Rewind session",
            KeyAction::ConfirmBacktrack => "Confirm rewind",
            KeyAction::CancelBacktrack => "Cancel rewind",
            KeyAction::BacktrackPrev => "Previous message",
            KeyAction::BacktrackNext => "Next message",
            KeyAction::ForkSession => "Fork session",

            // External Editor
            KeyAction::OpenExternalEditor => "Open external editor",

            // None
            KeyAction::None => "No action",
        }
    }

    /// Returns true if this action requires confirmation.
    pub fn requires_confirmation(&self) -> bool {
        matches!(
            self,
            KeyAction::Quit
                | KeyAction::DeleteSession
                | KeyAction::ClearContext
                | KeyAction::ApproveAll
                | KeyAction::RejectAll
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_display_roundtrip() {
        let actions = vec![
            KeyAction::Quit,
            KeyAction::Help,
            KeyAction::FocusNext,
            KeyAction::ScrollUp,
            KeyAction::Submit,
            KeyAction::Copy,
            KeyAction::Approve,
            KeyAction::None,
        ];

        for action in actions {
            let s = action.to_string();
            let parsed: KeyAction = s.parse().unwrap();
            assert_eq!(action, parsed);
        }
    }

    #[test]
    fn test_action_with_uuid() {
        let id = Uuid::new_v4();
        let action = KeyAction::LoadSession(id);
        let s = action.to_string();
        let parsed: KeyAction = s.parse().unwrap();
        assert_eq!(action, parsed);
    }

    #[test]
    fn test_action_with_command() {
        let action = KeyAction::ExecuteSlashCommand("help".to_string());
        let s = action.to_string();
        let parsed: KeyAction = s.parse().unwrap();
        assert_eq!(action, parsed);
    }

    #[test]
    fn test_action_requires_confirmation() {
        assert!(KeyAction::Quit.requires_confirmation());
        assert!(KeyAction::DeleteSession.requires_confirmation());
        assert!(!KeyAction::Help.requires_confirmation());
        assert!(!KeyAction::ScrollUp.requires_confirmation());
    }
}
