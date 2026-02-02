//! Plugin hook system for intercepting and modifying Cortex behavior.
//!
//! Hooks allow plugins to:
//! - Intercept tool execution (before/after)
//! - Process chat messages (before/after AI response)
//! - Override permission requests
//! - React to session events
//! - Intercept file operations (create/edit/delete)
//! - Intercept command execution
//! - Intercept user input
//! - Modify AI prompts (prompt injection)
//! - Handle errors and diagnostics
//! - React to configuration changes
//! - Intercept clipboard operations
//! - React to focus/blur events
//! - Custom context providers
//!
//! # Hook Priority
//! Hooks are executed in priority order (lowest value first).
//! A hook can:
//! - Continue: Allow the operation and next hooks to run
//! - Skip: Skip remaining hooks but allow the operation
//! - Abort: Cancel the operation entirely
//! - Replace: Replace the result with custom data
//! - Modify: Modify the input/output in place

// Core types
mod types;
pub use types::{HookPriority, HookResult};

// Tool execution hooks
mod tool_hooks;
pub use tool_hooks::{
    ToolExecuteAfterHook, ToolExecuteAfterInput, ToolExecuteAfterOutput, ToolExecuteBeforeHook,
    ToolExecuteBeforeInput, ToolExecuteBeforeOutput,
};

// Chat message hooks
mod chat_hooks;
pub use chat_hooks::{ChatMessageHook, ChatMessageInput, ChatMessageOutput, MessagePart};

// Permission hooks
mod permission_hooks;
pub use permission_hooks::{
    PermissionAskHook, PermissionAskInput, PermissionAskOutput, PermissionDecision,
};

// Prompt injection hooks
mod prompt_hooks;
pub use prompt_hooks::{
    ContextDocument, ContextDocumentType, PromptInjectHook, PromptInjectInput, PromptInjectOutput,
};

// AI response hooks
mod ai_response_hooks;
pub use ai_response_hooks::{
    AiResponseAfterHook, AiResponseAfterInput, AiResponseAfterOutput, AiResponseBeforeHook,
    AiResponseBeforeInput, AiResponseBeforeOutput, AiResponseStreamHook, AiResponseStreamInput,
    AiResponseStreamOutput, TokenUsage,
};

// File operation hooks
mod file_hooks;
pub use file_hooks::{
    FileOperation, FileOperationAfterHook, FileOperationAfterInput, FileOperationAfterOutput,
    FileOperationBeforeHook, FileOperationBeforeInput, FileOperationBeforeOutput, FilePostAction,
};

// Command execution hooks
mod command_hooks;
pub use command_hooks::{
    CommandExecuteAfterHook, CommandExecuteAfterInput, CommandExecuteAfterOutput,
    CommandExecuteBeforeHook, CommandExecuteBeforeInput, CommandExecuteBeforeOutput,
};

// User input hooks
mod input_hooks;
pub use input_hooks::{
    InputAction, InputInterceptHook, InputInterceptInput, InputInterceptOutput, InputSuggestion,
    QuickPickItem, SuggestionKind,
};

// Error handling hooks
mod error_hooks;
pub use error_hooks::{
    ErrorHandleHook, ErrorHandleInput, ErrorHandleOutput, ErrorRecovery, ErrorSource,
};

// Configuration change hooks
mod config_hooks;
pub use config_hooks::{
    ConfigChangeAction, ConfigChangeSource, ConfigChangedHook, ConfigChangedInput,
    ConfigChangedOutput,
};

// Session lifecycle hooks
mod session_hooks;
pub use session_hooks::{
    SessionEndAction, SessionEndHook, SessionEndInput, SessionEndOutput, SessionStartHook,
    SessionStartInput, SessionStartOutput,
};

// Workspace change hooks
mod workspace_hooks;
pub use workspace_hooks::{
    ProjectType, WorkspaceChangedHook, WorkspaceChangedInput, WorkspaceChangedOutput,
};

// Clipboard hooks
mod clipboard_hooks;
pub use clipboard_hooks::{
    ClipboardCopyHook, ClipboardCopyInput, ClipboardCopyOutput, ClipboardPasteHook,
    ClipboardPasteInput, ClipboardPasteOutput, ClipboardSource,
};

// UI rendering hooks
mod ui_hooks;
pub use ui_hooks::{
    // Style types
    BorderStyle,
    Color,
    // Keyboard bindings
    KeyBinding,
    KeyBindingHook,
    KeyBindingInput,
    KeyBindingOutput,
    KeyBindingResult,
    KeyModifier,
    // Layout customization
    LayoutConfig,
    LayoutCustomizeHook,
    LayoutCustomizeInput,
    LayoutCustomizeOutput,
    LayoutDirection,
    LayoutPanel,
    // Modal injection
    ModalDefinition,
    ModalInjectHook,
    ModalInjectInput,
    ModalInjectOutput,
    ModalLayer,
    TextStyle,
    // Theme types
    ThemeColors,
    ThemeOverride,
    ThemeOverrideHook,
    ThemeOverrideInput,
    ThemeOverrideOutput,
    // Toast notifications
    ToastDefinition,
    ToastLevel,
    ToastShowHook,
    ToastShowInput,
    ToastShowOutput,
    // Core UI types
    UiComponent,
    UiRegion,
    UiRenderHook,
    UiRenderInput,
    UiRenderOutput,
    UiWidget,
    WidgetConstraints,
    // Widget registration
    WidgetRegisterHook,
    WidgetRegisterInput,
    WidgetRegisterOutput,
    WidgetSize,
    WidgetStyle,
};

// Focus change hooks
mod focus_hooks;
pub use focus_hooks::{FocusAction, FocusChangeHook, FocusChangeInput, FocusChangeOutput};

// TUI event hooks
mod tui_events;
pub use tui_events::{
    AnimationFrameHook, AnimationFrameInput, AnimationFrameOutput, CustomEventEmitHook,
    CustomEventEmitInput, CustomEventEmitOutput, EventInterceptHook, EventInterceptInput,
    EventInterceptOutput, InterceptMode, MouseButton, MouseEventType, ScrollDirection, TuiEvent,
    TuiEventDispatchHook, TuiEventDispatchInput, TuiEventDispatchOutput, TuiEventFilter,
    TuiEventSubscribeHook, TuiEventSubscribeInput, TuiEventSubscribeOutput,
};

// Completion hooks
mod completion_hooks;
pub use completion_hooks::{
    ArgumentCompletionHook, ArgumentCompletionInput, ArgumentCompletionOutput, ArgumentDefinition,
    CompletionContext, CompletionItem, CompletionKind, CompletionProvider,
    CompletionProviderRegisterHook, CompletionProviderRegisterInput,
    CompletionProviderRegisterOutput, CompletionRequestHook, CompletionRequestInput,
    CompletionRequestOutput, CompletionResolveHook, CompletionResolveInput,
    CompletionResolveOutput,
};

// Hook registry
mod registry;
pub use registry::HookRegistry;

// Hook dispatcher
mod dispatcher;
pub use dispatcher::HookDispatcher;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Result;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct TestBeforeHook {
        priority: HookPriority,
    }

    #[async_trait]
    impl ToolExecuteBeforeHook for TestBeforeHook {
        fn priority(&self) -> HookPriority {
            self.priority
        }

        async fn execute(
            &self,
            _input: &ToolExecuteBeforeInput,
            output: &mut ToolExecuteBeforeOutput,
        ) -> Result<()> {
            if let Some(obj) = output.args.as_object_mut() {
                obj.insert("modified".to_string(), serde_json::json!(true));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_hook_registry() {
        use crate::manifest::HookType;

        let registry = HookRegistry::new();

        let hook = Arc::new(TestBeforeHook {
            priority: HookPriority::NORMAL,
        });

        registry
            .register_tool_execute_before("test-plugin", hook)
            .await;

        assert_eq!(registry.hook_count(HookType::ToolExecuteBefore).await, 1);
    }

    #[tokio::test]
    async fn test_hook_dispatcher() {
        let registry = Arc::new(HookRegistry::new());

        let hook = Arc::new(TestBeforeHook {
            priority: HookPriority::NORMAL,
        });

        registry
            .register_tool_execute_before("test-plugin", hook)
            .await;

        let dispatcher = HookDispatcher::new(registry);

        let input = ToolExecuteBeforeInput {
            tool: "read".to_string(),
            session_id: "session-1".to_string(),
            call_id: "call-1".to_string(),
            args: serde_json::json!({"original": "value"}),
        };

        let output = dispatcher.trigger_tool_execute_before(input).await.unwrap();

        assert_eq!(output.args["modified"], true);
        assert_eq!(output.args["original"], "value");
    }

    #[tokio::test]
    async fn test_hook_priority_ordering() {
        let registry = HookRegistry::new();

        let hook_low = Arc::new(TestBeforeHook {
            priority: HookPriority::LOW,
        });
        let hook_high = Arc::new(TestBeforeHook {
            priority: HookPriority::PLUGIN_HIGH,
        });
        let hook_normal = Arc::new(TestBeforeHook {
            priority: HookPriority::NORMAL,
        });

        // Register in wrong order
        registry
            .register_tool_execute_before("plugin-1", hook_low)
            .await;
        registry
            .register_tool_execute_before("plugin-2", hook_high)
            .await;
        registry
            .register_tool_execute_before("plugin-3", hook_normal)
            .await;

        // Check ordering
        let hooks = registry.tool_execute_before.read().await;
        assert_eq!(hooks[0].priority, HookPriority::PLUGIN_HIGH);
        assert_eq!(hooks[1].priority, HookPriority::NORMAL);
        assert_eq!(hooks[2].priority, HookPriority::LOW);
    }
}
