use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use cortex_core::{
    animation::{Pulse, Spinner, Typewriter},
    markdown::MarkdownTheme,
    style::ThemeColors,
    widgets::{CortexInput, Message},
};
use uuid::Uuid;

use crate::permissions::PermissionMode;
use crate::question::QuestionState;
use crate::selection::TextSelection;
use crate::views::tool_call::{ContentSegment, ToolCallDisplay};
use crate::widgets::{ToastManager, ToastPosition};

use super::approval::{ApprovalState, PendingToolResult};
use super::autocomplete::AutocompleteState;
use super::session::{ActiveModal, SessionSummary};
use super::streaming::StreamingState;
use super::subagent::SubagentTaskDisplay;
use super::types::{AppView, FocusTarget, OperationMode};

/// Main application state
pub struct AppState {
    pub view: AppView,
    pub previous_view: Option<AppView>,
    pub focus: FocusTarget,
    pub session_id: Option<Uuid>,
    pub messages: Vec<Message>,
    pub model: String,
    pub provider: String,
    pub system_prompt: Option<String>,
    pub sidebar_visible: bool,
    pub sidebar_width: u16,
    pub chat_scroll: usize,
    pub sidebar_scroll: usize,
    pub scrollbar_visible_until: Option<Instant>,
    pub scrollbar_hovered: bool,
    pub scrollbar_dragging: bool,
    /// Cached chat content metrics for scroll calculations
    pub chat_content_lines: usize,
    pub chat_visible_lines: usize,
    pub chat_scroll_pinned_bottom: bool,
    pub input: CortexInput<'static>,
    pub autocomplete: AutocompleteState,
    pub streaming: StreamingState,
    pub typewriter: Option<Typewriter>,
    pub brain_pulse: Pulse,
    pub spinner: Spinner,
    pub brain_frame: u64,
    pub login_flow: Option<crate::interactive::builders::LoginFlowState>,
    pub account_flow: Option<crate::interactive::builders::AccountFlowState>,
    pub billing_flow: Option<crate::interactive::builders::BillingFlowState>,
    pub pending_approval: Option<ApprovalState>,
    pub session_history: Vec<SessionSummary>,
    pub terminal_size: (u16, u16),
    pub running: bool,
    /// Flag to request clearing terminal scrollback buffer (#2817)
    pub pending_scrollback_clear: bool,
    pub last_ctrl_c: Option<Instant>,
    pub last_esc: Option<Instant>,
    pub active_modal: Option<ActiveModal>,
    // provider_picker removed: provider is now always "cortex"
    pub model_picker: crate::widgets::ModelPickerState,
    pub text_selection: TextSelection,
    pub toasts: ToastManager,
    /// Current permission mode for tool execution
    pub permission_mode: PermissionMode,
    /// Tool calls being displayed
    pub tool_calls: Vec<ToolCallDisplay>,
    /// Pending tool results that need to be sent back to the LLM
    pub pending_tool_results: Vec<PendingToolResult>,
    /// Current thinking budget level (for models that support it)
    pub thinking_budget: Option<String>,
    /// Event sequence counter for ordering tool calls by arrival
    pub event_sequence: u64,
    /// Content segments for interleaved text/tool display during streaming
    pub content_segments: Vec<ContentSegment>,
    /// Accumulated text before the next tool call (for segment creation)
    pub pending_text_segment: String,
    /// Active question prompt state
    pub question_state: Option<QuestionState>,
    /// Hovered option in question prompt (for mouse support)
    pub question_hovered_option: Option<usize>,
    /// Hovered tab in question prompt (for mouse support)
    pub question_hovered_tab: Option<usize>,
    /// Queue of pending user messages (sent together when system becomes available)
    pub message_queue: VecDeque<String>,
    /// Active subagent tasks being displayed.
    pub active_subagents: Vec<SubagentTaskDisplay>,
    /// Currently viewed subagent session ID (for SubagentConversation view)
    pub viewing_subagent: Option<String>,
    /// MCP servers list for management
    pub mcp_servers: Vec<crate::modal::mcp_manager::McpServerInfo>,
    /// Context files added to the session
    pub context_files: Vec<std::path::PathBuf>,
    /// Current log level setting
    pub log_level: String,
    /// Generic settings storage
    pub settings: HashMap<String, String>,
    /// Diff scroll position for approval view
    pub diff_scroll: i32,
    /// Input mode (normal text input or interactive selection)
    pub input_mode: crate::interactive::InputMode,
    /// Active theme name
    pub active_theme: String,
    /// Cached theme colors for quick access
    pub theme_colors: ThemeColors,
    /// Cached markdown theme for quick access
    pub markdown_theme: MarkdownTheme,
    /// Compact display mode
    pub compact_mode: bool,
    /// Debug mode enabled
    pub debug_mode: bool,
    /// Sandbox mode (restricted execution)
    pub sandbox_mode: bool,
    /// Temperature for model sampling
    pub temperature: f32,
    /// Max tokens for model output
    pub max_tokens: Option<u32>,
    /// Command history for /history
    pub command_history: Vec<String>,
    // Extended settings
    /// Show timestamps on messages
    pub timestamps_enabled: bool,
    /// Show line numbers in code blocks
    pub line_numbers_enabled: bool,
    /// Word wrap enabled
    pub word_wrap_enabled: bool,
    /// Syntax highlighting enabled
    pub syntax_highlight_enabled: bool,
    /// Auto scroll to new messages
    pub auto_scroll_enabled: bool,
    /// Sound notifications enabled
    pub sound_enabled: bool,
    /// Text streaming animation enabled
    pub streaming_enabled: bool,
    /// Context-aware mode (include open files)
    pub context_aware_enabled: bool,
    /// Add as co-author on commits
    pub co_author_enabled: bool,
    /// Auto-suggest commits after changes
    pub auto_commit_enabled: bool,
    /// Sign commits with GPG
    pub sign_commits_enabled: bool,
    /// Cloud sync for sessions
    pub cloud_sync_enabled: bool,
    /// Auto-save sessions
    pub auto_save_enabled: bool,
    /// Keep session history
    pub session_history_enabled: bool,
    /// Telemetry enabled
    pub telemetry_enabled: bool,
    /// Analytics enabled
    pub analytics_enabled: bool,
    /// Current operation mode (Build/Plan/Spec)
    pub operation_mode: OperationMode,
    // Agent creation state
    /// Agent creation location (project or global)
    pub agent_creation_location: Option<crate::interactive::builders::AgentLocation>,
    /// Agent creation method mode ("ai" or "manual_name")
    pub agent_creation_mode: Option<String>,
    /// Agent creation configuration in progress
    pub agent_creation_config: Option<crate::interactive::builders::NewAgentConfig>,
    /// Message to display after logout (printed after TUI closes)
    pub logout_message: Option<String>,
    /// User name for welcome screen
    pub user_name: Option<String>,
    /// User email for welcome screen
    pub user_email: Option<String>,
    /// Organization name for welcome screen
    pub org_name: Option<String>,
}

impl AppState {
    /// Create a new AppState with default values
    pub fn new() -> Self {
        Self {
            view: AppView::default(),
            previous_view: None,
            focus: FocusTarget::default(),
            session_id: None,
            messages: Vec::new(),
            model: String::from("gpt-4"),
            provider: String::from("cortex"),
            system_prompt: None,
            sidebar_visible: true,
            sidebar_width: 30,
            chat_scroll: 0,
            sidebar_scroll: 0,
            scrollbar_visible_until: None,
            scrollbar_hovered: false,
            scrollbar_dragging: false,
            chat_content_lines: 0,
            chat_visible_lines: 0,
            chat_scroll_pinned_bottom: true,
            input: CortexInput::new(),
            autocomplete: AutocompleteState::new(),
            streaming: StreamingState::default(),
            typewriter: None,
            brain_pulse: Pulse::new(2000),
            spinner: Spinner::dots(),
            brain_frame: 0,
            login_flow: None,
            account_flow: None,
            billing_flow: None,
            pending_approval: None,
            session_history: Vec::new(),
            terminal_size: (80, 24),
            running: true,
            pending_scrollback_clear: false,
            last_ctrl_c: None,
            last_esc: None,
            active_modal: None,
            // provider_picker removed: provider is now always "cortex"
            model_picker: crate::widgets::ModelPickerState::new(),
            text_selection: TextSelection::new(),
            toasts: ToastManager::new().with_position(ToastPosition::BottomLeft),
            permission_mode: PermissionMode::default(),
            tool_calls: Vec::new(),
            pending_tool_results: Vec::new(),
            thinking_budget: None,
            event_sequence: 0,
            content_segments: Vec::new(),
            pending_text_segment: String::new(),
            question_state: None,
            question_hovered_option: None,
            question_hovered_tab: None,
            message_queue: VecDeque::new(),
            active_subagents: Vec::new(),
            viewing_subagent: None,
            mcp_servers: Vec::new(),
            context_files: Vec::new(),
            log_level: String::from("info"),
            settings: HashMap::new(),
            diff_scroll: 0,
            input_mode: crate::interactive::InputMode::Normal,
            active_theme: "dark".to_string(),
            theme_colors: ThemeColors::dark(),
            markdown_theme: MarkdownTheme::default(),
            compact_mode: false,
            debug_mode: false,
            sandbox_mode: false,
            temperature: 0.7,
            max_tokens: None,
            command_history: Vec::new(),
            // Extended settings with sensible defaults
            timestamps_enabled: false,
            line_numbers_enabled: true,
            word_wrap_enabled: true,
            syntax_highlight_enabled: true,
            auto_scroll_enabled: true,
            sound_enabled: false,
            streaming_enabled: false,
            context_aware_enabled: true,
            co_author_enabled: true,
            auto_commit_enabled: false,
            sign_commits_enabled: false,
            cloud_sync_enabled: false,
            auto_save_enabled: true,
            session_history_enabled: true,
            telemetry_enabled: false,
            analytics_enabled: false,
            operation_mode: OperationMode::default(),
            // Agent creation state
            agent_creation_location: None,
            agent_creation_mode: None,
            agent_creation_config: None,
            logout_message: None,
            user_name: None,
            user_email: None,
            org_name: None,
        }
    }

    /// Create AppState with a specific model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Create AppState with a specific provider
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = provider.into();
        self
    }

    /// Create AppState with a system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Create AppState with terminal size
    pub fn with_terminal_size(mut self, width: u16, height: u16) -> Self {
        self.terminal_size = (width, height);
        self
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// APPSTATE METHODS - View and Focus
// ============================================================================

impl AppState {
    /// Set the current view
    pub fn set_view(&mut self, view: AppView) {
        self.previous_view = Some(self.view.clone());
        self.view = view;
    }

    /// Go back to the previous view
    pub fn go_back(&mut self) {
        if let Some(prev) = self.previous_view.take() {
            self.view = prev;
        }
    }

    /// Set the focus target
    pub fn set_focus(&mut self, focus: FocusTarget) {
        self.focus = focus;
    }

    /// Move focus to the next element
    pub fn focus_next(&mut self) {
        self.focus = match self.focus {
            FocusTarget::Input => FocusTarget::Chat,
            FocusTarget::Chat => {
                if self.sidebar_visible {
                    FocusTarget::Sidebar
                } else {
                    FocusTarget::Input
                }
            }
            FocusTarget::Sidebar => FocusTarget::Input,
            FocusTarget::Modal => FocusTarget::Modal,
        };
    }

    /// Move focus to the previous element
    pub fn focus_prev(&mut self) {
        self.focus = match self.focus {
            FocusTarget::Input => {
                if self.sidebar_visible {
                    FocusTarget::Sidebar
                } else {
                    FocusTarget::Chat
                }
            }
            FocusTarget::Chat => FocusTarget::Input,
            FocusTarget::Sidebar => FocusTarget::Chat,
            FocusTarget::Modal => FocusTarget::Modal,
        };
    }

    /// Toggle sidebar visibility
    pub fn toggle_sidebar(&mut self) {
        self.sidebar_visible = !self.sidebar_visible;
        if !self.sidebar_visible && self.focus == FocusTarget::Sidebar {
            self.focus = FocusTarget::Input;
        }
    }

    /// Change the active theme
    pub fn set_theme(&mut self, name: &str) {
        self.active_theme = name.to_string();
        self.theme_colors = ThemeColors::from_name(name);
        self.markdown_theme = MarkdownTheme::from_name(name);
    }

    /// Get AdaptiveColors from the current theme
    pub fn adaptive_colors(&self) -> crate::ui::AdaptiveColors {
        crate::ui::AdaptiveColors::from_theme_colors(&self.theme_colors)
    }
}

// ============================================================================
// APPSTATE METHODS - Messages
// ============================================================================

impl AppState {
    /// Add a message to the chat
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        if self.chat_scroll_pinned_bottom {
            self.scroll_chat_to_bottom();
        }
    }

    /// Get the last message
    pub fn last_message(&self) -> Option<&Message> {
        self.messages.last()
    }

    /// Get mutable reference to the last message
    pub fn last_message_mut(&mut self) -> Option<&mut Message> {
        self.messages.last_mut()
    }

    /// Clear all messages and request terminal scrollback clear (#2817)
    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.chat_scroll = 0;
        // Reset scroll state to bottom (no "↓ End" hint after clearing)
        self.chat_scroll_pinned_bottom = true;
        // Request clearing terminal scrollback buffer for privacy
        self.pending_scrollback_clear = true;
    }

    /// Check if scrollback clear is pending and reset the flag
    pub fn take_pending_scrollback_clear(&mut self) -> bool {
        let pending = self.pending_scrollback_clear;
        self.pending_scrollback_clear = false;
        pending
    }

    /// Add a message to the queue
    pub fn queue_message(&mut self, message: String) {
        if !message.trim().is_empty() {
            self.message_queue.push_back(message);
        }
    }

    /// Get number of queued messages
    pub fn queued_count(&self) -> usize {
        self.message_queue.len()
    }

    /// Check if there are queued messages
    pub fn has_queued_messages(&self) -> bool {
        !self.message_queue.is_empty()
    }

    /// Take all queued messages and combine into one
    /// Messages are joined with double newlines
    pub fn take_queued_messages(&mut self) -> Option<String> {
        if self.message_queue.is_empty() {
            return None;
        }
        let messages: Vec<String> = self.message_queue.drain(..).collect();
        Some(messages.join("\n\n"))
    }

    /// Clear the message queue (for cancellation)
    pub fn clear_message_queue(&mut self) {
        self.message_queue.clear();
    }
}

// ============================================================================
// APPSTATE METHODS - Streaming
// ============================================================================

impl AppState {
    /// Start streaming a response
    pub fn start_streaming(&mut self, tool: Option<String>) {
        self.streaming.start(tool);
        // Use typewriter only if streaming animation is enabled
        if self.streaming_enabled {
            self.typewriter = Some(Typewriter::dynamic(String::new(), 500.0));
        } else {
            self.typewriter = None;
        }
    }

    /// Stop streaming
    pub fn stop_streaming(&mut self) {
        self.streaming.stop();
    }

    /// Append content to the last streaming message
    pub fn append_streaming_content(&mut self, content: &str) {
        if let Some(msg) = self.messages.last_mut() {
            msg.content.push_str(content);
        }
        self.streaming.thinking = false;
    }

    /// Check if currently streaming
    pub fn is_streaming(&self) -> bool {
        self.streaming.is_streaming
    }

    /// Check if the system is busy (streaming, executing tool, has pending tool results, or running subagents)
    /// Used to determine if new messages should be queued
    pub fn is_busy(&self) -> bool {
        self.streaming.is_streaming
            || self.streaming.is_tool_executing()
            || !self.pending_tool_results.is_empty()
            || self.has_active_subagents()
    }
}

// ============================================================================
// APPSTATE METHODS - Scrolling
// ============================================================================

impl AppState {
    /// Scroll the chat by a delta amount
    pub fn scroll_chat(&mut self, delta: i32) {
        if delta < 0 {
            self.chat_scroll = self
                .chat_scroll
                .saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.chat_scroll = self.chat_scroll.saturating_add(delta as usize);
        }
        self.chat_scroll_pinned_bottom = false;
        self.show_scrollbar();
    }

    /// Scroll to the bottom of the chat
    pub fn scroll_chat_to_bottom(&mut self) {
        self.chat_scroll = 0; // 0 = at bottom (showing newest messages)
        self.chat_scroll_pinned_bottom = true;
        self.show_scrollbar();
    }

    /// Scroll to the top of the chat
    pub fn scroll_chat_to_top(&mut self) {
        self.chat_scroll = usize::MAX; // Large value = scrolled up (showing oldest messages)
        self.chat_scroll_pinned_bottom = false;
        self.show_scrollbar();
    }

    /// Check if chat is scrolled to the bottom
    pub fn is_chat_at_bottom(&self) -> bool {
        self.chat_scroll_pinned_bottom
    }

    /// Show the scrollbar temporarily
    pub fn show_scrollbar(&mut self) {
        self.scrollbar_visible_until = Some(Instant::now() + Duration::from_secs(2));
    }

    /// Check if the scrollbar should be visible
    pub fn is_scrollbar_visible(&self) -> bool {
        // Scrollbar is visible when: hovered, dragging, or within visibility timeout
        self.scrollbar_hovered
            || self.scrollbar_dragging
            || self
                .scrollbar_visible_until
                .map(|until| Instant::now() < until)
                .unwrap_or(false)
    }

    /// Get the scrollbar opacity (for fade effect)
    pub fn scrollbar_opacity(&self) -> f32 {
        // Full opacity when hovered or dragging
        if self.scrollbar_hovered || self.scrollbar_dragging {
            return 1.0;
        }

        self.scrollbar_visible_until
            .map(|until| {
                let remaining = until.saturating_duration_since(Instant::now());
                let fade_start = Duration::from_millis(500);
                if remaining > fade_start {
                    1.0
                } else {
                    remaining.as_secs_f32() / fade_start.as_secs_f32()
                }
            })
            .unwrap_or(0.0)
    }

    /// Tick the scrollbar visibility timer
    pub fn tick_scrollbar(&mut self) {
        // Don't expire visibility while hovered or dragging
        if self.scrollbar_hovered || self.scrollbar_dragging {
            return;
        }
        if let Some(until) = self.scrollbar_visible_until
            && Instant::now() >= until
        {
            self.scrollbar_visible_until = None;
        }
    }

    /// Set scrollbar hover state
    pub fn set_scrollbar_hovered(&mut self, hovered: bool) {
        self.scrollbar_hovered = hovered;
        if hovered {
            self.show_scrollbar();
        }
    }

    /// Start scrollbar drag operation
    pub fn start_scrollbar_drag(&mut self) {
        self.scrollbar_dragging = true;
        self.show_scrollbar();
    }

    /// End scrollbar drag operation
    pub fn end_scrollbar_drag(&mut self) {
        self.scrollbar_dragging = false;
    }

    /// Update cached chat content metrics (called after rendering)
    pub fn update_chat_metrics(&mut self, total_lines: usize, visible_lines: usize) {
        self.chat_content_lines = total_lines;
        self.chat_visible_lines = visible_lines;
    }

    /// Estimate total chat content lines based on messages
    /// This is used for scroll position calculations when actual metrics aren't available
    pub fn estimate_chat_lines(&self, area_width: u16) -> usize {
        let mut total = 0;
        for msg in &self.messages {
            // Estimate lines: content length / width + 2 for spacing
            let content_len = msg.content.len();
            let width = area_width.saturating_sub(4) as usize; // Account for margins
            if let Some(lines) = content_len.checked_div(width) {
                total += lines + 1; // At least 1 line per message
            }
            total += 2; // Blank line + some overhead
        }
        // Add tool calls
        total += self.tool_calls.len() * 3; // Estimate 3 lines per tool call
        // Add subagents
        total += self.active_subagents.len() * 4; // Estimate 4 lines per subagent
        total
    }

    /// Calculate scroll position from Y coordinate relative to chat area
    /// Returns scroll offset (0 = at bottom, max_scroll = at top)
    pub fn scroll_from_y_position(&self, y_in_area: u16, area_height: u16) -> usize {
        // Use cached metrics if available, otherwise estimate
        let total_lines = if self.chat_content_lines > 0 {
            self.chat_content_lines
        } else {
            // Estimate based on terminal width (80 is a reasonable default)
            self.estimate_chat_lines(80)
        };
        let visible_lines = if self.chat_visible_lines > 0 {
            self.chat_visible_lines
        } else {
            area_height as usize
        };

        if total_lines <= visible_lines {
            return 0;
        }
        let max_scroll = total_lines.saturating_sub(visible_lines);
        // y_in_area = 0 means top of scrollbar (max_scroll)
        // y_in_area = area_height-1 means bottom of scrollbar (0)
        let ratio = 1.0 - (y_in_area as f32 / area_height.saturating_sub(1).max(1) as f32);
        (ratio * max_scroll as f32).round() as usize
    }

    /// Scroll the sidebar
    pub fn scroll_sidebar(&mut self, delta: i32) {
        if delta < 0 {
            self.sidebar_scroll = self
                .sidebar_scroll
                .saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.sidebar_scroll = self.sidebar_scroll.saturating_add(delta as usize);
        }
    }

    /// Scroll the diff view
    pub fn scroll_diff(&mut self, delta: i32) {
        self.diff_scroll = (self.diff_scroll + delta).max(0);
    }
}
