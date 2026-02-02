//! Hook registry for storing and managing registered hooks.
//!
//! The registry maintains collections of registered hooks organized by type,
//! with support for priority-based ordering and plugin-level management.

use std::sync::Arc;
use tokio::sync::RwLock;

use super::chat_hooks::ChatMessageHook;
use super::command_hooks::{CommandExecuteAfterHook, CommandExecuteBeforeHook};
use super::focus_hooks::FocusChangeHook;
use super::input_hooks::InputInterceptHook;
use super::permission_hooks::PermissionAskHook;
use super::session_hooks::{SessionEndHook, SessionStartHook};
use super::tool_hooks::{ToolExecuteAfterHook, ToolExecuteBeforeHook};
use super::tui_events::{
    AnimationFrameHook, CustomEventEmitHook, EventInterceptHook, TuiEventDispatchHook,
    TuiEventSubscribeHook,
};
use super::types::HookPriority;
use super::ui_hooks::{
    KeyBindingHook, LayoutCustomizeHook, ModalInjectHook, ThemeOverrideHook, ToastShowHook,
    UiRenderHook, WidgetRegisterHook,
};
use crate::manifest::HookType;

// ============================================================================
// REGISTERED HOOK WRAPPERS
// ============================================================================

/// Registered hook with metadata for tool.execute.before hook type.
pub(crate) struct RegisteredToolBeforeHook {
    pub plugin_id: String,
    pub hook: Arc<dyn ToolExecuteBeforeHook>,
    pub priority: HookPriority,
}

/// Registered hook with metadata for tool.execute.after hook type.
pub(crate) struct RegisteredToolAfterHook {
    pub plugin_id: String,
    pub hook: Arc<dyn ToolExecuteAfterHook>,
    pub priority: HookPriority,
}

/// Registered hook with metadata for chat.message hook type.
pub(crate) struct RegisteredChatHook {
    pub plugin_id: String,
    pub hook: Arc<dyn ChatMessageHook>,
    pub priority: HookPriority,
}

/// Registered hook with metadata for permission.ask hook type.
pub(crate) struct RegisteredPermissionHook {
    pub plugin_id: String,
    pub hook: Arc<dyn PermissionAskHook>,
    pub priority: HookPriority,
}

/// Registered hook with metadata for ui.render hook type.
#[allow(dead_code)]
pub(crate) struct RegisteredUiRenderHook {
    pub plugin_id: String,
    pub hook: Arc<dyn UiRenderHook>,
    pub priority: HookPriority,
}

/// Registered hook for widget registration.
#[allow(dead_code)]
pub(crate) struct RegisteredWidgetRegisterHook {
    pub plugin_id: String,
    pub hook: Arc<dyn WidgetRegisterHook>,
    pub priority: HookPriority,
}

/// Registered hook for key binding registration.
#[allow(dead_code)]
pub(crate) struct RegisteredKeyBindingHook {
    pub plugin_id: String,
    pub hook: Arc<dyn KeyBindingHook>,
    pub priority: HookPriority,
}

/// Registered hook for theme override.
#[allow(dead_code)]
pub(crate) struct RegisteredThemeOverrideHook {
    pub plugin_id: String,
    pub hook: Arc<dyn ThemeOverrideHook>,
    pub priority: HookPriority,
}

/// Registered hook for layout customization.
#[allow(dead_code)]
pub(crate) struct RegisteredLayoutCustomizeHook {
    pub plugin_id: String,
    pub hook: Arc<dyn LayoutCustomizeHook>,
    pub priority: HookPriority,
}

/// Registered hook for modal injection.
#[allow(dead_code)]
pub(crate) struct RegisteredModalInjectHook {
    pub plugin_id: String,
    pub hook: Arc<dyn ModalInjectHook>,
    pub priority: HookPriority,
}

/// Registered hook for toast notifications.
#[allow(dead_code)]
pub(crate) struct RegisteredToastShowHook {
    pub plugin_id: String,
    pub hook: Arc<dyn ToastShowHook>,
    pub priority: HookPriority,
}

/// Registered hook for TUI event subscription.
#[allow(dead_code)]
pub(crate) struct RegisteredTuiEventSubscribeHook {
    pub plugin_id: String,
    pub hook: Arc<dyn TuiEventSubscribeHook>,
    pub priority: HookPriority,
}

/// Registered hook for TUI event dispatch.
#[allow(dead_code)]
pub(crate) struct RegisteredTuiEventDispatchHook {
    pub plugin_id: String,
    pub hook: Arc<dyn TuiEventDispatchHook>,
    pub priority: HookPriority,
}

/// Registered hook for custom event emission.
#[allow(dead_code)]
pub(crate) struct RegisteredCustomEventEmitHook {
    pub plugin_id: String,
    pub hook: Arc<dyn CustomEventEmitHook>,
    pub priority: HookPriority,
}

/// Registered hook for event interception.
#[allow(dead_code)]
pub(crate) struct RegisteredEventInterceptHook {
    pub plugin_id: String,
    pub hook: Arc<dyn EventInterceptHook>,
    pub priority: HookPriority,
}

/// Registered hook for animation frames.
#[allow(dead_code)]
pub(crate) struct RegisteredAnimationFrameHook {
    pub plugin_id: String,
    pub hook: Arc<dyn AnimationFrameHook>,
    pub priority: HookPriority,
}

/// Registered hook for command.execute.before.
#[allow(dead_code)]
pub(crate) struct RegisteredCommandBeforeHook {
    pub plugin_id: String,
    pub hook: Arc<dyn CommandExecuteBeforeHook>,
    pub priority: HookPriority,
}

/// Registered hook for command.execute.after.
#[allow(dead_code)]
pub(crate) struct RegisteredCommandAfterHook {
    pub plugin_id: String,
    pub hook: Arc<dyn CommandExecuteAfterHook>,
    pub priority: HookPriority,
}

/// Registered hook for input interception.
#[allow(dead_code)]
pub(crate) struct RegisteredInputInterceptHook {
    pub plugin_id: String,
    pub hook: Arc<dyn InputInterceptHook>,
    pub priority: HookPriority,
}

/// Registered hook for session start.
#[allow(dead_code)]
pub(crate) struct RegisteredSessionStartHook {
    pub plugin_id: String,
    pub hook: Arc<dyn SessionStartHook>,
    pub priority: HookPriority,
}

/// Registered hook for session end.
#[allow(dead_code)]
pub(crate) struct RegisteredSessionEndHook {
    pub plugin_id: String,
    pub hook: Arc<dyn SessionEndHook>,
    pub priority: HookPriority,
}

/// Registered hook for focus change.
#[allow(dead_code)]
pub(crate) struct RegisteredFocusChangeHook {
    pub plugin_id: String,
    pub hook: Arc<dyn FocusChangeHook>,
    pub priority: HookPriority,
}

// ============================================================================
// HOOK REGISTRY
// ============================================================================

/// Registry for all hook handlers.
///
/// The registry maintains collections of registered hooks organized by type.
/// All hooks are stored with their plugin ID and priority for proper ordering
/// and cleanup when plugins are unloaded.
pub struct HookRegistry {
    // Tool hooks
    pub(crate) tool_execute_before: RwLock<Vec<RegisteredToolBeforeHook>>,
    pub(crate) tool_execute_after: RwLock<Vec<RegisteredToolAfterHook>>,

    // Chat hooks
    pub(crate) chat_message: RwLock<Vec<RegisteredChatHook>>,

    // Permission hooks
    pub(crate) permission_ask: RwLock<Vec<RegisteredPermissionHook>>,

    // UI hooks
    pub(crate) ui_render: RwLock<Vec<RegisteredUiRenderHook>>,
    pub(crate) widget_register: RwLock<Vec<RegisteredWidgetRegisterHook>>,
    pub(crate) key_binding: RwLock<Vec<RegisteredKeyBindingHook>>,
    pub(crate) theme_override: RwLock<Vec<RegisteredThemeOverrideHook>>,
    pub(crate) layout_customize: RwLock<Vec<RegisteredLayoutCustomizeHook>>,
    pub(crate) modal_inject: RwLock<Vec<RegisteredModalInjectHook>>,
    pub(crate) toast_show: RwLock<Vec<RegisteredToastShowHook>>,

    // TUI event hooks
    pub(crate) tui_event_subscribe: RwLock<Vec<RegisteredTuiEventSubscribeHook>>,
    pub(crate) tui_event_dispatch: RwLock<Vec<RegisteredTuiEventDispatchHook>>,
    pub(crate) custom_event_emit: RwLock<Vec<RegisteredCustomEventEmitHook>>,
    pub(crate) event_intercept: RwLock<Vec<RegisteredEventInterceptHook>>,
    pub(crate) animation_frame: RwLock<Vec<RegisteredAnimationFrameHook>>,

    // Command hooks
    pub(crate) command_execute_before: RwLock<Vec<RegisteredCommandBeforeHook>>,
    pub(crate) command_execute_after: RwLock<Vec<RegisteredCommandAfterHook>>,

    // Input hooks
    pub(crate) input_intercept: RwLock<Vec<RegisteredInputInterceptHook>>,

    // Session hooks
    pub(crate) session_start: RwLock<Vec<RegisteredSessionStartHook>>,
    pub(crate) session_end: RwLock<Vec<RegisteredSessionEndHook>>,

    // Focus hooks
    pub(crate) focus_change: RwLock<Vec<RegisteredFocusChangeHook>>,
}

impl HookRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            tool_execute_before: RwLock::new(Vec::new()),
            tool_execute_after: RwLock::new(Vec::new()),
            chat_message: RwLock::new(Vec::new()),
            permission_ask: RwLock::new(Vec::new()),
            ui_render: RwLock::new(Vec::new()),
            widget_register: RwLock::new(Vec::new()),
            key_binding: RwLock::new(Vec::new()),
            theme_override: RwLock::new(Vec::new()),
            layout_customize: RwLock::new(Vec::new()),
            modal_inject: RwLock::new(Vec::new()),
            toast_show: RwLock::new(Vec::new()),
            tui_event_subscribe: RwLock::new(Vec::new()),
            tui_event_dispatch: RwLock::new(Vec::new()),
            custom_event_emit: RwLock::new(Vec::new()),
            event_intercept: RwLock::new(Vec::new()),
            animation_frame: RwLock::new(Vec::new()),
            command_execute_before: RwLock::new(Vec::new()),
            command_execute_after: RwLock::new(Vec::new()),
            input_intercept: RwLock::new(Vec::new()),
            session_start: RwLock::new(Vec::new()),
            session_end: RwLock::new(Vec::new()),
            focus_change: RwLock::new(Vec::new()),
        }
    }

    // ========================================================================
    // TOOL HOOKS
    // ========================================================================

    /// Register a tool.execute.before hook.
    pub async fn register_tool_execute_before(
        &self,
        plugin_id: &str,
        hook: Arc<dyn ToolExecuteBeforeHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.tool_execute_before.write().await;
        hooks.push(RegisteredToolBeforeHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a tool.execute.after hook.
    pub async fn register_tool_execute_after(
        &self,
        plugin_id: &str,
        hook: Arc<dyn ToolExecuteAfterHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.tool_execute_after.write().await;
        hooks.push(RegisteredToolAfterHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    // ========================================================================
    // CHAT HOOKS
    // ========================================================================

    /// Register a chat.message hook.
    pub async fn register_chat_message(&self, plugin_id: &str, hook: Arc<dyn ChatMessageHook>) {
        let priority = hook.priority();
        let mut hooks = self.chat_message.write().await;
        hooks.push(RegisteredChatHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    // ========================================================================
    // PERMISSION HOOKS
    // ========================================================================

    /// Register a permission.ask hook.
    pub async fn register_permission_ask(&self, plugin_id: &str, hook: Arc<dyn PermissionAskHook>) {
        let priority = hook.priority();
        let mut hooks = self.permission_ask.write().await;
        hooks.push(RegisteredPermissionHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    // ========================================================================
    // UI HOOKS
    // ========================================================================

    /// Register a ui.render hook.
    pub async fn register_ui_render(&self, plugin_id: &str, hook: Arc<dyn UiRenderHook>) {
        let priority = hook.priority();
        let mut hooks = self.ui_render.write().await;
        hooks.push(RegisteredUiRenderHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a widget registration hook.
    pub async fn register_widget_register(
        &self,
        plugin_id: &str,
        hook: Arc<dyn WidgetRegisterHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.widget_register.write().await;
        hooks.push(RegisteredWidgetRegisterHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a key binding hook.
    pub async fn register_key_binding(&self, plugin_id: &str, hook: Arc<dyn KeyBindingHook>) {
        let priority = hook.priority();
        let mut hooks = self.key_binding.write().await;
        hooks.push(RegisteredKeyBindingHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a theme override hook.
    pub async fn register_theme_override(&self, plugin_id: &str, hook: Arc<dyn ThemeOverrideHook>) {
        let priority = hook.priority();
        let mut hooks = self.theme_override.write().await;
        hooks.push(RegisteredThemeOverrideHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a layout customization hook.
    pub async fn register_layout_customize(
        &self,
        plugin_id: &str,
        hook: Arc<dyn LayoutCustomizeHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.layout_customize.write().await;
        hooks.push(RegisteredLayoutCustomizeHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a modal injection hook.
    pub async fn register_modal_inject(&self, plugin_id: &str, hook: Arc<dyn ModalInjectHook>) {
        let priority = hook.priority();
        let mut hooks = self.modal_inject.write().await;
        hooks.push(RegisteredModalInjectHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a toast show hook.
    pub async fn register_toast_show(&self, plugin_id: &str, hook: Arc<dyn ToastShowHook>) {
        let priority = hook.priority();
        let mut hooks = self.toast_show.write().await;
        hooks.push(RegisteredToastShowHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    // ========================================================================
    // TUI EVENT HOOKS
    // ========================================================================

    /// Register a TUI event subscription hook.
    pub async fn register_tui_event_subscribe(
        &self,
        plugin_id: &str,
        hook: Arc<dyn TuiEventSubscribeHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.tui_event_subscribe.write().await;
        hooks.push(RegisteredTuiEventSubscribeHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a TUI event dispatch hook.
    pub async fn register_tui_event_dispatch(
        &self,
        plugin_id: &str,
        hook: Arc<dyn TuiEventDispatchHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.tui_event_dispatch.write().await;
        hooks.push(RegisteredTuiEventDispatchHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a custom event emit hook.
    pub async fn register_custom_event_emit(
        &self,
        plugin_id: &str,
        hook: Arc<dyn CustomEventEmitHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.custom_event_emit.write().await;
        hooks.push(RegisteredCustomEventEmitHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register an event intercept hook.
    pub async fn register_event_intercept(
        &self,
        plugin_id: &str,
        hook: Arc<dyn EventInterceptHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.event_intercept.write().await;
        hooks.push(RegisteredEventInterceptHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register an animation frame hook.
    pub async fn register_animation_frame(
        &self,
        plugin_id: &str,
        hook: Arc<dyn AnimationFrameHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.animation_frame.write().await;
        hooks.push(RegisteredAnimationFrameHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    // ========================================================================
    // COMMAND HOOKS
    // ========================================================================

    /// Register a command.execute.before hook.
    pub async fn register_command_execute_before(
        &self,
        plugin_id: &str,
        hook: Arc<dyn CommandExecuteBeforeHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.command_execute_before.write().await;
        hooks.push(RegisteredCommandBeforeHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a command.execute.after hook.
    pub async fn register_command_execute_after(
        &self,
        plugin_id: &str,
        hook: Arc<dyn CommandExecuteAfterHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.command_execute_after.write().await;
        hooks.push(RegisteredCommandAfterHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    // ========================================================================
    // INPUT HOOKS
    // ========================================================================

    /// Register an input intercept hook.
    pub async fn register_input_intercept(
        &self,
        plugin_id: &str,
        hook: Arc<dyn InputInterceptHook>,
    ) {
        let priority = hook.priority();
        let mut hooks = self.input_intercept.write().await;
        hooks.push(RegisteredInputInterceptHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    // ========================================================================
    // SESSION HOOKS
    // ========================================================================

    /// Register a session start hook.
    pub async fn register_session_start(&self, plugin_id: &str, hook: Arc<dyn SessionStartHook>) {
        let priority = hook.priority();
        let mut hooks = self.session_start.write().await;
        hooks.push(RegisteredSessionStartHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    /// Register a session end hook.
    pub async fn register_session_end(&self, plugin_id: &str, hook: Arc<dyn SessionEndHook>) {
        let priority = hook.priority();
        let mut hooks = self.session_end.write().await;
        hooks.push(RegisteredSessionEndHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    // ========================================================================
    // FOCUS HOOKS
    // ========================================================================

    /// Register a focus change hook.
    pub async fn register_focus_change(&self, plugin_id: &str, hook: Arc<dyn FocusChangeHook>) {
        let priority = hook.priority();
        let mut hooks = self.focus_change.write().await;
        hooks.push(RegisteredFocusChangeHook {
            plugin_id: plugin_id.to_string(),
            hook,
            priority,
        });
        hooks.sort_by_key(|h| h.priority);
    }

    // ========================================================================
    // PLUGIN MANAGEMENT
    // ========================================================================

    /// Unregister all hooks for a plugin.
    pub async fn unregister_plugin(&self, plugin_id: &str) {
        // Tool hooks
        {
            let mut hooks = self.tool_execute_before.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.tool_execute_after.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }

        // Chat hooks
        {
            let mut hooks = self.chat_message.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }

        // Permission hooks
        {
            let mut hooks = self.permission_ask.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }

        // UI hooks
        {
            let mut hooks = self.ui_render.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.widget_register.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.key_binding.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.theme_override.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.layout_customize.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.modal_inject.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.toast_show.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }

        // TUI event hooks
        {
            let mut hooks = self.tui_event_subscribe.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.tui_event_dispatch.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.custom_event_emit.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.event_intercept.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.animation_frame.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }

        // Command hooks
        {
            let mut hooks = self.command_execute_before.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.command_execute_after.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }

        // Input hooks
        {
            let mut hooks = self.input_intercept.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }

        // Session hooks
        {
            let mut hooks = self.session_start.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
        {
            let mut hooks = self.session_end.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }

        // Focus hooks
        {
            let mut hooks = self.focus_change.write().await;
            hooks.retain(|h| h.plugin_id != plugin_id);
        }
    }

    /// Get hook count for a specific type.
    pub async fn hook_count(&self, hook_type: HookType) -> usize {
        match hook_type {
            HookType::ToolExecuteBefore => self.tool_execute_before.read().await.len(),
            HookType::ToolExecuteAfter => self.tool_execute_after.read().await.len(),
            HookType::ChatMessage => self.chat_message.read().await.len(),
            HookType::PermissionAsk => self.permission_ask.read().await.len(),
            HookType::UiRender => self.ui_render.read().await.len(),
            HookType::WidgetRegister => self.widget_register.read().await.len(),
            HookType::KeyBinding => self.key_binding.read().await.len(),
            HookType::ThemeOverride => self.theme_override.read().await.len(),
            HookType::LayoutCustomize => self.layout_customize.read().await.len(),
            HookType::ModalInject => self.modal_inject.read().await.len(),
            HookType::ToastShow => self.toast_show.read().await.len(),
            HookType::TuiEventSubscribe => self.tui_event_subscribe.read().await.len(),
            HookType::TuiEventDispatch => self.tui_event_dispatch.read().await.len(),
            HookType::CustomEventEmit => self.custom_event_emit.read().await.len(),
            HookType::EventIntercept => self.event_intercept.read().await.len(),
            HookType::AnimationFrame => self.animation_frame.read().await.len(),
            HookType::CommandExecuteBefore => self.command_execute_before.read().await.len(),
            HookType::CommandExecuteAfter => self.command_execute_after.read().await.len(),
            HookType::InputIntercept => self.input_intercept.read().await.len(),
            HookType::SessionStart => self.session_start.read().await.len(),
            HookType::SessionEnd => self.session_end.read().await.len(),
            HookType::FocusChange => self.focus_change.read().await.len(),
            _ => 0,
        }
    }

    /// Get total number of registered hooks across all types.
    pub async fn total_hook_count(&self) -> usize {
        let mut count = 0;
        count += self.tool_execute_before.read().await.len();
        count += self.tool_execute_after.read().await.len();
        count += self.chat_message.read().await.len();
        count += self.permission_ask.read().await.len();
        count += self.ui_render.read().await.len();
        count += self.widget_register.read().await.len();
        count += self.key_binding.read().await.len();
        count += self.theme_override.read().await.len();
        count += self.layout_customize.read().await.len();
        count += self.modal_inject.read().await.len();
        count += self.toast_show.read().await.len();
        count += self.tui_event_subscribe.read().await.len();
        count += self.tui_event_dispatch.read().await.len();
        count += self.custom_event_emit.read().await.len();
        count += self.event_intercept.read().await.len();
        count += self.animation_frame.read().await.len();
        count += self.command_execute_before.read().await.len();
        count += self.command_execute_after.read().await.len();
        count += self.input_intercept.read().await.len();
        count += self.session_start.read().await.len();
        count += self.session_end.read().await.len();
        count += self.focus_change.read().await.len();
        count
    }

    /// Get list of plugins with registered hooks.
    pub async fn registered_plugins(&self) -> Vec<String> {
        let mut plugins = std::collections::HashSet::new();

        for h in self.tool_execute_before.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.tool_execute_after.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.chat_message.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.permission_ask.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.ui_render.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.widget_register.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.key_binding.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.theme_override.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.layout_customize.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.modal_inject.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.toast_show.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.tui_event_subscribe.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.tui_event_dispatch.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.custom_event_emit.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.event_intercept.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.animation_frame.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.command_execute_before.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.command_execute_after.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.input_intercept.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.session_start.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.session_end.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }
        for h in self.focus_change.read().await.iter() {
            plugins.insert(h.plugin_id.clone());
        }

        plugins.into_iter().collect()
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}
