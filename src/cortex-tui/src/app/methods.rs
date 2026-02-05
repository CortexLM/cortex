use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::permissions::PermissionMode;
use crate::question::QuestionState;
use crate::views::tool_call::{ContentSegment, ToolResultDisplay, ToolStatus};

use super::approval::ApprovalState;
use super::session::ActiveModal;
use super::state::AppState;
use super::subagent::SubagentTaskDisplay;
use super::types::{AppView, ApprovalMode, FocusTarget, OperationMode};

// ============================================================================
// APPSTATE METHODS - Approval
// ============================================================================

impl AppState {
    /// Request approval for a tool
    pub fn request_approval(
        &mut self,
        tool_name: String,
        tool_args: String,
        diff_preview: Option<String>,
    ) {
        // Try to parse the args as JSON
        let tool_args_json = serde_json::from_str(&tool_args).ok();
        self.pending_approval = Some(ApprovalState {
            tool_call_id: String::new(),
            tool_name,
            tool_args,
            tool_args_json,
            diff_preview,
            approval_mode: ApprovalMode::Ask,
        });
        self.set_view(AppView::Approval);
    }

    /// Request approval for a tool with full details
    pub fn request_tool_approval(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        tool_args: serde_json::Value,
        diff_preview: Option<String>,
    ) {
        self.pending_approval = Some(ApprovalState {
            tool_call_id,
            tool_name,
            tool_args: serde_json::to_string_pretty(&tool_args).unwrap_or_default(),
            tool_args_json: Some(tool_args),
            diff_preview,
            approval_mode: ApprovalMode::Ask,
        });
        self.set_view(AppView::Approval);

        // Play approval required sound
        crate::sound::play_approval_required(self.sound_enabled);
    }

    /// Approve the pending tool
    pub fn approve(&mut self) -> Option<ApprovalState> {
        let approval = self.pending_approval.take();
        self.go_back();
        approval
    }

    /// Reject the pending tool
    pub fn reject(&mut self) -> Option<ApprovalState> {
        let approval = self.pending_approval.take();
        self.go_back();
        approval
    }

    /// Check if there's a pending approval
    pub fn has_pending_approval(&self) -> bool {
        self.pending_approval.is_some()
    }
}

// ============================================================================
// APPSTATE METHODS - Sessions
// ============================================================================

impl AppState {
    /// Start a new session
    pub fn new_session(&mut self) {
        self.session_id = Some(Uuid::new_v4());
        self.clear_messages();
        self.set_view(AppView::Session);
    }

    /// Load an existing session
    pub fn load_session(&mut self, session_id: Uuid) {
        self.session_id = Some(session_id);
        self.set_view(AppView::Session);
    }

    /// Get the display name for the current model
    pub fn model_display(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

// ============================================================================
// APPSTATE METHODS - Animations and Timers
// ============================================================================

impl AppState {
    /// Tick animations and timers
    pub fn tick(&mut self) {
        self.brain_pulse.tick();
        self.spinner.tick();
        self.brain_frame = self.brain_frame.wrapping_add(1);
        self.tick_scrollbar();

        if let Some(ref mut typewriter) = self.typewriter {
            typewriter.tick();
        }
    }

    /// Set the terminal size
    pub fn set_terminal_size(&mut self, width: u16, height: u16) {
        self.terminal_size = (width, height);
    }

    /// Issue #2327: Invalidate cached content layout calculations.
    ///
    /// Called on terminal resize to ensure code blocks and wrapped text
    /// are properly re-rendered for the new terminal dimensions.
    /// This prevents rendering corruption when the terminal is resized
    /// while streaming content (especially code blocks with syntax highlighting).
    pub fn invalidate_content_layout(&mut self) {
        // Reset scroll positions to prevent displaying content outside new bounds
        self.chat_scroll = 0;
        self.sidebar_scroll = 0;
        self.diff_scroll = 0;

        // Reset content line counts - will be recalculated on next render
        self.chat_content_lines = 0;
        self.chat_visible_lines = 0;

        // Clear any partial text segment that might have incomplete line wrapping
        // The typewriter will regenerate content on the next render
        if let Some(ref mut tw) = self.typewriter {
            tw.reset();
        }

        // Re-pin to bottom if we were following the stream
        if self.streaming.is_streaming {
            self.chat_scroll_pinned_bottom = true;
        }
    }
}

// ============================================================================
// APPSTATE METHODS - Quit and Input Handling
// ============================================================================

impl AppState {
    /// Request to quit the application
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Handle Ctrl+C press (double-tap to quit)
    pub fn handle_ctrl_c(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_ctrl_c
            && now.duration_since(last) < Duration::from_millis(500)
        {
            self.quit();
            return true;
        }
        self.last_ctrl_c = Some(now);
        false
    }

    /// Reset the Ctrl+C timer
    pub fn reset_ctrl_c(&mut self) {
        self.last_ctrl_c = None;
    }

    /// Handle ESC press (double-tap to quit when idle)
    pub fn handle_esc(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_esc
            && now.duration_since(last) < Duration::from_millis(500)
        {
            self.quit();
            return true;
        }
        self.last_esc = Some(now);
        false
    }

    /// Reset the ESC timer
    pub fn reset_esc(&mut self) {
        self.last_esc = None;
    }
}

// ============================================================================
// APPSTATE METHODS - Modals
// ============================================================================

impl AppState {
    /// Open the provider picker modal - DEPRECATED
    /// Provider is now always "cortex", so this is a no-op
    pub fn open_provider_picker(&mut self) {
        // No-op: provider switching has been removed
        // Provider is now always "cortex"
    }

    /// Open the model picker modal
    pub fn open_model_picker(&mut self) {
        self.model_picker.load_models(&self.provider, &self.model);
        self.active_modal = Some(ActiveModal::ModelPicker);
        self.focus = FocusTarget::Modal;
    }

    /// Open the theme picker modal
    pub fn open_theme_picker(&mut self) {
        self.active_modal = Some(ActiveModal::ThemePicker);
        self.focus = FocusTarget::Modal;
    }

    /// Open a modal
    pub fn open_modal(&mut self, modal: ActiveModal) {
        self.active_modal = Some(modal);
        self.focus = FocusTarget::Modal;
    }

    /// Close the current modal
    pub fn close_modal(&mut self) {
        self.active_modal = None;
        self.focus = FocusTarget::Input;
    }

    /// Check if a modal is open
    pub fn has_modal(&self) -> bool {
        self.active_modal.is_some()
    }

    /// Get the current modal
    pub fn modal(&self) -> Option<&ActiveModal> {
        self.active_modal.as_ref()
    }
}

// ============================================================================
// APPSTATE METHODS - Tool Calls
// ============================================================================

impl AppState {
    /// Cycle to the next permission mode
    pub fn cycle_permission_mode(&mut self) {
        self.permission_mode = self.permission_mode.cycle_next();
    }

    /// Set the thinking budget level
    pub fn set_thinking_budget(&mut self, budget: Option<String>) {
        self.thinking_budget = budget;
    }

    /// Add a tool call for display
    /// This also flushes any pending text as a segment before the tool call
    pub fn add_tool_call(&mut self, id: String, name: String, arguments: serde_json::Value) {
        use crate::views::tool_call::ToolCallDisplay;

        // First, flush any accumulated text as a segment
        if !self.pending_text_segment.is_empty() {
            let text_seq = self.event_sequence;
            self.event_sequence += 1;
            self.content_segments.push(ContentSegment::Text {
                content: std::mem::take(&mut self.pending_text_segment),
                sequence: text_seq,
            });
        }

        // Now add the tool call with its sequence
        let seq = self.event_sequence;
        self.event_sequence += 1;

        // Add to content segments timeline
        self.content_segments.push(ContentSegment::ToolCall {
            tool_call_id: id.clone(),
            sequence: seq,
        });

        // Add to tool_calls for status tracking
        self.tool_calls
            .push(ToolCallDisplay::new(id, name, arguments, seq));
    }

    /// Append text to the pending segment (called during streaming)
    pub fn append_streaming_text(&mut self, text: &str) {
        self.pending_text_segment.push_str(text);
    }

    /// Flush any remaining pending text as a final segment
    pub fn flush_pending_text(&mut self) {
        if !self.pending_text_segment.is_empty() {
            let seq = self.event_sequence;
            self.event_sequence += 1;
            self.content_segments.push(ContentSegment::Text {
                content: std::mem::take(&mut self.pending_text_segment),
                sequence: seq,
            });
        }
    }

    /// Clear content segments (for new conversation turn)
    pub fn clear_content_segments(&mut self) {
        self.content_segments.clear();
        self.pending_text_segment.clear();
    }

    /// Update tool call status
    pub fn update_tool_status(&mut self, id: &str, status: ToolStatus) {
        if let Some(call) = self.tool_calls.iter_mut().find(|c| c.id == id) {
            call.set_status(status);
        }
    }

    /// Update tool call result
    pub fn update_tool_result(&mut self, id: &str, output: String, success: bool, summary: String) {
        if let Some(call) = self.tool_calls.iter_mut().find(|c| c.id == id) {
            // Clear live output when tool completes (replaced by result summary)
            call.clear_live_output();
            call.set_result(ToolResultDisplay {
                output,
                success,
                summary,
            });
            call.set_status(if success {
                ToolStatus::Completed
            } else {
                ToolStatus::Failed
            });
        }
    }

    /// Toggle tool call collapsed state
    pub fn toggle_tool_collapsed(&mut self, id: &str) {
        if let Some(call) = self.tool_calls.iter_mut().find(|c| c.id == id) {
            call.toggle_collapsed();
        }
    }

    /// Clear all tool calls and content segments (for new conversation turn)
    pub fn clear_tool_calls(&mut self) {
        self.tool_calls.clear();
        self.clear_content_segments();
        // Also clear completed/failed subagents from the previous turn
        self.active_subagents.retain(|t| !t.status.is_terminal());
    }

    /// Advance spinner frames for all running tool calls
    pub fn tick_tool_spinners(&mut self) {
        for call in &mut self.tool_calls {
            if call.status == ToolStatus::Running {
                call.tick_spinner();
            }
        }
    }

    /// Check if any tool calls are currently running.
    /// Used for render optimization - we need to keep rendering for spinner animations.
    pub fn has_active_tool_calls(&self) -> bool {
        self.tool_calls
            .iter()
            .any(|c| c.status == ToolStatus::Running)
    }

    /// Append output to a tool call's live output buffer
    pub fn append_tool_output(&mut self, id: &str, line: String) {
        if let Some(call) = self.tool_calls.iter_mut().find(|c| c.id == id) {
            call.append_output(line);
        }
    }

    /// Add a pending tool result for agentic loop continuation
    pub fn add_pending_tool_result(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        output: String,
        success: bool,
    ) {
        use super::approval::PendingToolResult;
        self.pending_tool_results.push(PendingToolResult {
            tool_call_id,
            tool_name,
            output,
            success,
        });
    }

    /// Take all pending tool results (clears the list)
    pub fn take_pending_tool_results(&mut self) -> Vec<super::approval::PendingToolResult> {
        std::mem::take(&mut self.pending_tool_results)
    }

    /// Check if there are pending tool results
    pub fn has_pending_tool_results(&self) -> bool {
        !self.pending_tool_results.is_empty()
    }
}

// ============================================================================
// APPSTATE METHODS - Subagent Management
// ============================================================================

impl AppState {
    /// Add a new subagent task for display.
    pub fn add_subagent_task(&mut self, task: SubagentTaskDisplay) {
        self.active_subagents.push(task);
    }

    /// Update a subagent task's state.
    pub fn update_subagent<F>(&mut self, session_id: &str, update: F)
    where
        F: FnOnce(&mut SubagentTaskDisplay),
    {
        if let Some(task) = self
            .active_subagents
            .iter_mut()
            .find(|t| t.session_id == session_id)
        {
            update(task);
        }
    }

    /// Remove a completed/failed subagent task and return it.
    pub fn remove_subagent(&mut self, session_id: &str) -> Option<SubagentTaskDisplay> {
        if let Some(idx) = self
            .active_subagents
            .iter()
            .position(|t| t.session_id == session_id)
        {
            Some(self.active_subagents.remove(idx))
        } else {
            None
        }
    }

    /// Tick spinner frames for all active subagents.
    pub fn tick_subagent_spinners(&mut self) {
        for task in &mut self.active_subagents {
            if !task.status.is_terminal() {
                task.spinner_frame = (task.spinner_frame + 1) % 4;
            }
        }
    }

    /// Check if any subagents are currently running.
    pub fn has_active_subagents(&self) -> bool {
        self.active_subagents
            .iter()
            .any(|t| !t.status.is_terminal())
    }

    /// Get the number of active (non-terminal) subagents.
    pub fn active_subagent_count(&self) -> usize {
        self.active_subagents
            .iter()
            .filter(|t| !t.status.is_terminal())
            .count()
    }

    /// Enter subagent conversation view
    pub fn view_subagent_conversation(&mut self, session_id: String) {
        self.viewing_subagent = Some(session_id.clone());
        self.set_view(AppView::SubagentConversation(session_id));
    }

    /// Return to main conversation from subagent view
    pub fn return_to_main_conversation(&mut self) {
        self.viewing_subagent = None;
        self.set_view(AppView::Session);
    }

    /// Check if viewing a subagent conversation
    pub fn is_viewing_subagent(&self) -> bool {
        self.viewing_subagent.is_some()
    }

    /// Get the currently viewed subagent session ID
    pub fn get_viewing_subagent(&self) -> Option<&String> {
        self.viewing_subagent.as_ref()
    }
}

// ============================================================================
// APPSTATE METHODS - Question Prompt
// ============================================================================

impl AppState {
    /// Start a question prompt session
    pub fn start_question_prompt(&mut self, state: QuestionState) {
        self.question_state = Some(state);
        self.question_hovered_option = None;
        self.question_hovered_tab = None;
        self.view = AppView::Questions;
    }

    /// Get the current question state
    pub fn get_question_state(&self) -> Option<&QuestionState> {
        self.question_state.as_ref()
    }

    /// Get mutable question state
    pub fn get_question_state_mut(&mut self) -> Option<&mut QuestionState> {
        self.question_state.as_mut()
    }

    /// Complete the question prompt and return answers
    pub fn complete_question_prompt(&mut self) -> Option<serde_json::Value> {
        let state = self.question_state.take()?;
        self.question_hovered_option = None;
        self.question_hovered_tab = None;
        self.view = AppView::Session;
        Some(state.get_formatted_answers())
    }

    /// Cancel the question prompt
    pub fn cancel_question_prompt(&mut self) -> Option<String> {
        let state = self.question_state.take()?;
        self.question_hovered_option = None;
        self.question_hovered_tab = None;
        self.view = AppView::Session;
        Some(state.request.id)
    }

    /// Check if there's an active question prompt
    pub fn has_question_prompt(&self) -> bool {
        self.question_state.is_some()
    }

    /// Set hovered option for mouse support
    pub fn set_question_hovered_option(&mut self, index: Option<usize>) {
        self.question_hovered_option = index;
    }

    /// Set hovered tab for mouse support
    pub fn set_question_hovered_tab(&mut self, index: Option<usize>) {
        self.question_hovered_tab = index;
    }
}

// ============================================================================
// APPSTATE METHODS - Interactive Input Mode
// ============================================================================

impl AppState {
    /// Check if currently in interactive mode.
    pub fn is_interactive_mode(&self) -> bool {
        self.input_mode.is_interactive()
    }

    /// Enter interactive mode with the given state.
    pub fn enter_interactive_mode(&mut self, state: crate::interactive::InteractiveState) {
        self.input_mode = crate::interactive::InputMode::Interactive(Box::new(state));
    }

    /// Exit interactive mode and return to normal input.
    pub fn exit_interactive_mode(&mut self) {
        self.input_mode = crate::interactive::InputMode::Normal;
    }

    /// Get the interactive state if in interactive mode.
    pub fn get_interactive_state(&self) -> Option<&crate::interactive::InteractiveState> {
        self.input_mode.interactive()
    }

    /// Get mutable interactive state if in interactive mode.
    pub fn get_interactive_state_mut(
        &mut self,
    ) -> Option<&mut crate::interactive::InteractiveState> {
        self.input_mode.interactive_mut()
    }
}

// ============================================================================
// APPSTATE METHODS - Approval Mode and Settings
// ============================================================================

impl AppState {
    /// Returns the current approval mode as a string
    pub fn approval_mode_string(&self) -> String {
        match self.permission_mode {
            PermissionMode::High => "ask".to_string(),
            PermissionMode::Medium => "medium".to_string(),
            PermissionMode::Low => "auto".to_string(),
            PermissionMode::Yolo => "yolo".to_string(),
        }
    }

    /// Sets the approval mode
    pub fn set_approval_mode(&mut self, mode: ApprovalMode) {
        self.permission_mode = match mode {
            ApprovalMode::Ask => PermissionMode::High,
            ApprovalMode::AllowSession => PermissionMode::Medium,
            ApprovalMode::AllowAlways => PermissionMode::Yolo,
        };
    }

    /// Toggle compact mode
    pub fn toggle_compact(&mut self) {
        self.compact_mode = !self.compact_mode;
    }

    /// Toggle debug mode
    pub fn toggle_debug(&mut self) {
        self.debug_mode = !self.debug_mode;
    }

    /// Toggle sandbox mode
    pub fn toggle_sandbox(&mut self) {
        self.sandbox_mode = !self.sandbox_mode;
    }

    /// Add command to history
    pub fn add_to_history(&mut self, cmd: &str) {
        if !cmd.is_empty() {
            self.command_history.push(cmd.to_string());
            // Keep only last 100 commands
            if self.command_history.len() > 100 {
                self.command_history.remove(0);
            }
        }
    }
}

// ============================================================================
// APPSTATE METHODS - Operation Mode
// ============================================================================

impl AppState {
    /// Toggle the operation mode (Build -> Plan -> Spec -> Build)
    pub fn toggle_operation_mode(&mut self) {
        self.operation_mode = self.operation_mode.next();
    }

    /// Set the operation mode directly
    pub fn set_operation_mode(&mut self, mode: OperationMode) {
        self.operation_mode = mode;
    }

    /// Get the current operation mode
    pub fn get_operation_mode(&self) -> OperationMode {
        self.operation_mode
    }

    /// Check if the current mode allows writing
    pub fn can_write(&self) -> bool {
        matches!(self.operation_mode, OperationMode::Build)
    }

    /// Check if the current mode is Plan mode
    pub fn is_plan_mode(&self) -> bool {
        matches!(self.operation_mode, OperationMode::Plan)
    }

    /// Check if the current mode is Spec mode
    pub fn is_spec_mode(&self) -> bool {
        matches!(self.operation_mode, OperationMode::Spec)
    }

    /// Get the mode indicator text for display
    pub fn mode_indicator(&self) -> &'static str {
        self.operation_mode.indicator()
    }

    /// Get the mode name for display
    pub fn mode_name(&self) -> &'static str {
        self.operation_mode.name()
    }
}
