//! Main dispatch logic for action handling.

use crate::actions::KeyAction;
use crate::app::FocusTarget;

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle a key action, returning whether it was consumed.
    ///
    /// This is the main entry point for action handling. It dispatches the
    /// action to the appropriate handler method based on the action type.
    ///
    /// # Arguments
    ///
    /// * `action` - The key action to handle
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - The action was handled and consumed
    /// * `Ok(false)` - The action was not handled (should propagate)
    /// * `Err(_)` - An error occurred during handling
    pub async fn handle(&mut self, action: KeyAction) -> Result<bool> {
        match action {
            // Core actions
            KeyAction::Quit => self.handle_quit().await,
            KeyAction::Help => self.handle_help(),
            KeyAction::Cancel => self.handle_cancel().await,

            // Navigation
            KeyAction::FocusNext => self.handle_focus_next(),
            KeyAction::FocusPrev => self.handle_focus_prev(),
            KeyAction::FocusInput => self.handle_focus(FocusTarget::Input),
            KeyAction::FocusChat => self.handle_focus(FocusTarget::Chat),
            KeyAction::FocusSidebar => self.handle_focus(FocusTarget::Sidebar),

            // Scrolling
            KeyAction::ScrollUp => self.handle_scroll(-1),
            KeyAction::ScrollDown => self.handle_scroll(1),
            KeyAction::ScrollPageUp => {
                // Use terminal height - 1 for standard page scroll (provides context overlap)
                let page_size = (self.state.terminal_size.1 as i32).saturating_sub(1).max(1);
                self.handle_scroll(-page_size)
            }
            KeyAction::ScrollPageDown => {
                // Use terminal height - 1 for standard page scroll (provides context overlap)
                let page_size = (self.state.terminal_size.1 as i32).saturating_sub(1).max(1);
                self.handle_scroll(page_size)
            }
            KeyAction::ScrollToTop => self.handle_scroll_to_top(),
            KeyAction::ScrollToBottom => self.handle_scroll_to_bottom(),

            // Input
            KeyAction::Submit => self.handle_submit().await,
            KeyAction::NewLine => self.handle_newline(),
            KeyAction::Clear => self.handle_clear(),
            KeyAction::HistoryPrev => self.handle_history_prev(),
            KeyAction::HistoryNext => self.handle_history_next(),

            // View
            KeyAction::ToggleSidebar => self.handle_toggle_sidebar(),
            KeyAction::ToggleHelp => self.handle_help(),
            KeyAction::ToggleSettings => self.handle_settings(),
            KeyAction::CyclePermissionMode => self.handle_cycle_permission_mode(),
            KeyAction::ToggleToolDetails => self.handle_toggle_tool_details(),

            // Session
            KeyAction::NewSession => self.handle_new_session().await,
            KeyAction::DeleteSession => self.handle_delete_session().await,
            KeyAction::RenameSession => self.handle_rename_session(),
            KeyAction::ExportSession => self.handle_export_session().await,

            // Model
            KeyAction::SwitchModel => self.handle_switch_model(),
            // SwitchProvider removed: provider is now always "cortex"

            // Edit
            KeyAction::Copy => self.handle_copy(),
            KeyAction::Paste => self.handle_paste(),
            KeyAction::SelectAll => self.handle_select_all(),

            // Approval
            KeyAction::Approve => self.handle_approve().await,
            KeyAction::Reject => self.handle_reject().await,
            KeyAction::ApproveSession => self.handle_approve_session().await,
            KeyAction::ApproveAlways => self.handle_approve_always().await,
            KeyAction::ApproveAll => self.handle_approve_all().await,
            KeyAction::RejectAll => self.handle_reject_all().await,
            KeyAction::ViewDiff => self.handle_view_diff(),

            // Tools
            KeyAction::CancelTool => self.handle_cancel_tool().await,
            KeyAction::RetryTool => self.handle_retry_tool().await,

            // Operation Mode
            KeyAction::ToggleOperationMode => self.handle_toggle_operation_mode(),
            KeyAction::ApproveSpec => self.handle_approve_spec().await,
            KeyAction::RejectSpec => self.handle_reject_spec(),

            // Context
            KeyAction::AddFile => self.handle_add_file(),
            KeyAction::AddFolder => self.handle_add_folder(),
            KeyAction::ClearContext => self.handle_clear_context(),

            // Commands
            KeyAction::ExecuteSlashCommand(cmd) => self.handle_slash_command(&cmd).await,

            // Session loading (has UUID parameter)
            KeyAction::LoadSession(id) => self.handle_load_session(id).await,

            // Card shortcuts (handled in event_loop.rs, not here)
            // These are handled by the EventLoop directly before ActionHandler
            KeyAction::OpenCommandPalette => Ok(false),
            KeyAction::OpenSessions => Ok(false),
            KeyAction::OpenMcp => Ok(false),

            // Transcript (handled in event_loop.rs)
            KeyAction::ViewTranscript => Ok(false),

            // Backtracking actions (handled in event_loop.rs for state coordination)
            KeyAction::OpenBacktrack => Ok(false),
            KeyAction::ConfirmBacktrack => Ok(false),
            KeyAction::CancelBacktrack => Ok(false),
            KeyAction::BacktrackPrev => Ok(false),
            KeyAction::BacktrackNext => Ok(false),
            KeyAction::ForkSession => Ok(false),

            // External editor (handled in event_loop.rs)
            KeyAction::OpenExternalEditor => Ok(false),

            // No action
            KeyAction::None => Ok(false),
        }
    }
}
