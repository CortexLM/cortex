//! Scenario registration for screenshot generation.
//!
//! This module contains all the predefined scenarios for TUI screenshots.

use crate::screenshot_generator::types::ScreenshotScenario;

/// Trait for registering scenarios.
pub trait ScenarioRegistry {
    /// Get mutable access to the scenarios list.
    fn scenarios_mut(&mut self) -> &mut Vec<ScreenshotScenario>;

    /// Register all built-in scenarios.
    fn register_all_scenarios(&mut self) {
        self.register_view_scenarios();
        self.register_autocomplete_scenarios();
        self.register_modal_scenarios();
        self.register_streaming_scenarios();
        self.register_tool_scenarios();
        self.register_approval_scenarios();
        self.register_permission_scenarios();
        self.register_message_scenarios();
        self.register_error_scenarios();
        self.register_sidebar_scenarios();
        self.register_question_scenarios();
        self.register_input_scenarios();
        self.register_scroll_scenarios();
        self.register_animation_scenarios();
    }

    /// Register view-related scenarios.
    fn register_view_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "empty_session",
                "Empty Session View",
                "views",
                "Initial empty session with no messages",
            )
            .with_tags(vec!["session", "empty", "initial"]),
            ScreenshotScenario::new(
                "session_with_messages",
                "Session with Messages",
                "views",
                "Session view with user and assistant messages",
            )
            .with_tags(vec!["session", "messages", "conversation"]),
            ScreenshotScenario::new(
                "help_view",
                "Help View",
                "views",
                "The help overlay showing keybindings",
            )
            .with_tags(vec!["help", "keybindings"]),
            ScreenshotScenario::new(
                "settings_view",
                "Settings View",
                "views",
                "Settings panel with configuration options",
            )
            .with_tags(vec!["settings", "config"]),
        ]);
    }

    /// Register autocomplete-related scenarios.
    fn register_autocomplete_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "autocomplete_commands",
                "Command Autocomplete",
                "autocomplete",
                "Autocomplete popup showing slash commands",
            )
            .with_tags(vec!["autocomplete", "commands", "slash"]),
            ScreenshotScenario::new(
                "autocomplete_commands_filtered",
                "Filtered Command Autocomplete",
                "autocomplete",
                "Autocomplete with filter query showing matching commands",
            )
            .with_tags(vec!["autocomplete", "commands", "filter"]),
            ScreenshotScenario::new(
                "autocomplete_mentions",
                "Mention Autocomplete",
                "autocomplete",
                "Autocomplete popup showing @ mentions",
            )
            .with_tags(vec!["autocomplete", "mentions", "at"]),
            ScreenshotScenario::new(
                "autocomplete_scroll",
                "Scrollable Autocomplete",
                "autocomplete",
                "Autocomplete with many items showing scrollbar",
            )
            .with_tags(vec!["autocomplete", "scroll", "long"]),
            ScreenshotScenario::new(
                "autocomplete_selected",
                "Autocomplete with Selection",
                "autocomplete",
                "Autocomplete with an item selected (highlighted)",
            )
            .with_tags(vec!["autocomplete", "selection", "highlight"]),
        ]);
    }

    /// Register modal-related scenarios.
    fn register_modal_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "modal_model_picker",
                "Model Picker Modal",
                "modals",
                "Modal for selecting AI model",
            )
            .with_tags(vec!["modal", "model", "picker"]),
            ScreenshotScenario::new(
                "modal_command_palette",
                "Command Palette",
                "modals",
                "Command palette modal with search",
            )
            .with_tags(vec!["modal", "command", "palette"]),
            ScreenshotScenario::new(
                "modal_export",
                "Export Modal",
                "modals",
                "Session export dialog",
            )
            .with_tags(vec!["modal", "export"]),
            ScreenshotScenario::new(
                "modal_form",
                "Form Modal",
                "modals",
                "Generic form modal with inputs",
            )
            .with_tags(vec!["modal", "form", "input"]),
        ]);
    }

    /// Register streaming-related scenarios.
    fn register_streaming_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "streaming_started",
                "Streaming Started",
                "streaming",
                "Initial streaming state with thinking indicator",
            )
            .with_tags(vec!["streaming", "thinking", "start"]),
            ScreenshotScenario::new(
                "streaming_in_progress",
                "Streaming In Progress",
                "streaming",
                "Active streaming with partial response",
            )
            .with_tags(vec!["streaming", "progress", "partial"]),
            ScreenshotScenario::new(
                "streaming_with_spinner",
                "Streaming with Spinner",
                "streaming",
                "Streaming state showing animated spinner",
            )
            .with_tags(vec!["streaming", "spinner", "animation"]),
            ScreenshotScenario::new(
                "streaming_completed",
                "Streaming Completed",
                "streaming",
                "State after streaming completes",
            )
            .with_tags(vec!["streaming", "complete", "done"]),
        ]);
    }

    /// Register tool execution scenarios.
    fn register_tool_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "tool_pending",
                "Tool Pending Execution",
                "tools",
                "Tool call waiting to execute",
            )
            .with_tags(vec!["tool", "pending", "waiting"]),
            ScreenshotScenario::new(
                "tool_running",
                "Tool Running",
                "tools",
                "Tool actively executing with spinner",
            )
            .with_tags(vec!["tool", "running", "executing"]),
            ScreenshotScenario::new(
                "tool_completed",
                "Tool Completed",
                "tools",
                "Tool finished successfully with output",
            )
            .with_tags(vec!["tool", "completed", "success"]),
            ScreenshotScenario::new(
                "tool_failed",
                "Tool Failed",
                "tools",
                "Tool execution that failed with error",
            )
            .with_tags(vec!["tool", "failed", "error"]),
            ScreenshotScenario::new(
                "tool_collapsed",
                "Tool Collapsed",
                "tools",
                "Tool display in collapsed state",
            )
            .with_tags(vec!["tool", "collapsed", "minimized"]),
            ScreenshotScenario::new(
                "tool_expanded",
                "Tool Expanded",
                "tools",
                "Tool display fully expanded with details",
            )
            .with_tags(vec!["tool", "expanded", "details"]),
            ScreenshotScenario::new(
                "tool_multiple",
                "Multiple Tools",
                "tools",
                "Multiple tool calls displayed together",
            )
            .with_tags(vec!["tool", "multiple", "list"]),
        ]);
    }

    /// Register approval-related scenarios.
    fn register_approval_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "approval_simple",
                "Simple Approval",
                "approval",
                "Basic tool approval dialog",
            )
            .with_tags(vec!["approval", "simple", "dialog"]),
            ScreenshotScenario::new(
                "approval_with_diff",
                "Approval with Diff",
                "approval",
                "Tool approval showing file diff",
            )
            .with_tags(vec!["approval", "diff", "file"]),
            ScreenshotScenario::new(
                "approval_dangerous",
                "Dangerous Operation Approval",
                "approval",
                "Approval for potentially dangerous operation",
            )
            .with_tags(vec!["approval", "dangerous", "warning"]),
            ScreenshotScenario::new(
                "approval_modes",
                "Approval Mode Selection",
                "approval",
                "Approval showing mode options (ask/session/always)",
            )
            .with_tags(vec!["approval", "modes", "options"]),
        ]);
    }

    /// Register permission-related scenarios.
    fn register_permission_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "permission_high",
                "High Security Mode",
                "permissions",
                "UI in high security (ask) permission mode",
            )
            .with_tags(vec!["permission", "high", "ask"]),
            ScreenshotScenario::new(
                "permission_medium",
                "Medium Security Mode",
                "permissions",
                "UI in medium permission mode",
            )
            .with_tags(vec!["permission", "medium"]),
            ScreenshotScenario::new(
                "permission_low",
                "Low Security Mode",
                "permissions",
                "UI in low (auto) permission mode",
            )
            .with_tags(vec!["permission", "low", "auto"]),
            ScreenshotScenario::new(
                "permission_yolo",
                "YOLO Mode",
                "permissions",
                "UI in YOLO (all auto-approved) mode",
            )
            .with_tags(vec!["permission", "yolo"]),
        ]);
    }

    /// Register message-related scenarios.
    fn register_message_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "message_user",
                "User Message",
                "messages",
                "A single user message",
            )
            .with_tags(vec!["message", "user"]),
            ScreenshotScenario::new(
                "message_assistant",
                "Assistant Message",
                "messages",
                "A single assistant message",
            )
            .with_tags(vec!["message", "assistant"]),
            ScreenshotScenario::new(
                "message_long",
                "Long Message",
                "messages",
                "A very long message with wrapping",
            )
            .with_tags(vec!["message", "long", "wrap"]),
            ScreenshotScenario::new(
                "message_code_block",
                "Message with Code Block",
                "messages",
                "Message containing syntax-highlighted code",
            )
            .with_tags(vec!["message", "code", "syntax"]),
            ScreenshotScenario::new(
                "message_markdown",
                "Markdown Message",
                "messages",
                "Message with various markdown formatting",
            )
            .with_tags(vec!["message", "markdown", "format"]),
        ]);
    }

    /// Register error-related scenarios.
    fn register_error_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "error_toast",
                "Error Toast",
                "errors",
                "Error notification toast",
            )
            .with_tags(vec!["error", "toast", "notification"]),
            ScreenshotScenario::new(
                "error_streaming",
                "Streaming Error",
                "errors",
                "Error during streaming response",
            )
            .with_tags(vec!["error", "streaming"]),
            ScreenshotScenario::new(
                "error_connection",
                "Connection Error",
                "errors",
                "Network/connection error display",
            )
            .with_tags(vec!["error", "connection", "network"]),
            ScreenshotScenario::new(
                "warning_toast",
                "Warning Toast",
                "errors",
                "Warning notification toast",
            )
            .with_tags(vec!["warning", "toast", "notification"]),
            ScreenshotScenario::new(
                "info_toast",
                "Info Toast",
                "errors",
                "Informational notification toast",
            )
            .with_tags(vec!["info", "toast", "notification"]),
        ]);
    }

    /// Register sidebar-related scenarios.
    fn register_sidebar_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "sidebar_visible",
                "Sidebar Visible",
                "sidebar",
                "Session with sidebar open",
            )
            .with_tags(vec!["sidebar", "visible", "open"]),
            ScreenshotScenario::new(
                "sidebar_hidden",
                "Sidebar Hidden",
                "sidebar",
                "Session with sidebar collapsed",
            )
            .with_tags(vec!["sidebar", "hidden", "closed"]),
            ScreenshotScenario::new(
                "sidebar_sessions",
                "Sidebar with Sessions",
                "sidebar",
                "Sidebar showing session list",
            )
            .with_tags(vec!["sidebar", "sessions", "list"]),
            ScreenshotScenario::new(
                "sidebar_selected",
                "Sidebar Selection",
                "sidebar",
                "Sidebar with selected session",
            )
            .with_tags(vec!["sidebar", "selected", "focus"]),
        ]);
    }

    /// Register question-related scenarios.
    fn register_question_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "question_single",
                "Single Choice Question",
                "questions",
                "Question prompt with single selection",
            )
            .with_tags(vec!["question", "single", "choice"]),
            ScreenshotScenario::new(
                "question_multiple",
                "Multiple Choice Question",
                "questions",
                "Question prompt with multiple selections",
            )
            .with_tags(vec!["question", "multiple", "choice"]),
            ScreenshotScenario::new(
                "question_text",
                "Text Input Question",
                "questions",
                "Question prompt expecting text input",
            )
            .with_tags(vec!["question", "text", "input"]),
            ScreenshotScenario::new(
                "question_tabs",
                "Question with Tabs",
                "questions",
                "Multi-step question with tab navigation",
            )
            .with_tags(vec!["question", "tabs", "multi"]),
        ]);
    }

    /// Register input-related scenarios.
    fn register_input_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "input_empty",
                "Empty Input",
                "input",
                "Input field when empty",
            )
            .with_tags(vec!["input", "empty"]),
            ScreenshotScenario::new(
                "input_with_text",
                "Input with Text",
                "input",
                "Input field with user typing",
            )
            .with_tags(vec!["input", "text", "typing"]),
            ScreenshotScenario::new(
                "input_multiline",
                "Multiline Input",
                "input",
                "Input field with multiple lines",
            )
            .with_tags(vec!["input", "multiline"]),
            ScreenshotScenario::new(
                "input_with_cursor",
                "Input with Cursor",
                "input",
                "Input showing cursor position",
            )
            .with_tags(vec!["input", "cursor"]),
            ScreenshotScenario::new(
                "input_command",
                "Command Input",
                "input",
                "Input with slash command being typed",
            )
            .with_tags(vec!["input", "command", "slash"]),
        ]);
    }

    /// Register scroll-related scenarios.
    fn register_scroll_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "scroll_top",
                "Scrolled to Top",
                "scroll",
                "Chat scrolled to oldest messages",
            )
            .with_tags(vec!["scroll", "top"]),
            ScreenshotScenario::new(
                "scroll_bottom",
                "Scrolled to Bottom",
                "scroll",
                "Chat at bottom (newest messages)",
            )
            .with_tags(vec!["scroll", "bottom"]),
            ScreenshotScenario::new(
                "scroll_middle",
                "Scrolled Middle",
                "scroll",
                "Chat scrolled to middle",
            )
            .with_tags(vec!["scroll", "middle"]),
            ScreenshotScenario::new(
                "scrollbar_visible",
                "Visible Scrollbar",
                "scroll",
                "Scrollbar indicator visible",
            )
            .with_tags(vec!["scroll", "scrollbar", "visible"]),
        ]);
    }

    /// Register animation-related scenarios.
    fn register_animation_scenarios(&mut self) {
        self.scenarios_mut().extend(vec![
            ScreenshotScenario::new(
                "spinner_frame_1",
                "Spinner Frame 1",
                "animations",
                "Spinner animation frame 1",
            )
            .with_tags(vec!["animation", "spinner", "frame"]),
            ScreenshotScenario::new(
                "spinner_frame_2",
                "Spinner Frame 2",
                "animations",
                "Spinner animation frame 2",
            )
            .with_tags(vec!["animation", "spinner", "frame"]),
            ScreenshotScenario::new(
                "spinner_frame_3",
                "Spinner Frame 3",
                "animations",
                "Spinner animation frame 3",
            )
            .with_tags(vec!["animation", "spinner", "frame"]),
            ScreenshotScenario::new(
                "brain_pulse",
                "Brain Pulse Animation",
                "animations",
                "Thinking indicator brain pulse",
            )
            .with_tags(vec!["animation", "brain", "pulse"]),
            ScreenshotScenario::new(
                "typewriter_effect",
                "Typewriter Effect",
                "animations",
                "Text appearing with typewriter effect",
            )
            .with_tags(vec!["animation", "typewriter", "text"]),
        ]);
    }
}
