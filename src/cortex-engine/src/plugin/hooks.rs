//! Plugin hooks module.
//!
//! Defines hook types, contexts, and dispatch mechanisms for the plugin system.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::types::{HookContext, HookResponse, PluginHook};

/// Hook registration for a plugin.
#[derive(Debug, Clone)]
pub struct HookRegistration {
    /// Plugin that registered this hook.
    pub plugin_name: String,
    /// The hook event.
    pub hook: PluginHook,
    /// Priority (lower = runs first).
    pub priority: i32,
    /// Whether this registration is enabled.
    pub enabled: bool,
}

/// Hook dispatcher that manages hook registrations and dispatch.
#[derive(Debug, Default)]
pub struct HookDispatcher {
    /// Registrations organized by hook type.
    registrations: HashMap<PluginHook, Vec<HookRegistration>>,
}

impl HookDispatcher {
    /// Create a new hook dispatcher.
    pub fn new() -> Self {
        Self {
            registrations: HashMap::new(),
        }
    }

    /// Register a hook handler.
    pub fn register(&mut self, registration: HookRegistration) {
        let handlers = self
            .registrations
            .entry(registration.hook)
            .or_insert_with(Vec::new);

        handlers.push(registration);

        // Sort by priority (lower first)
        handlers.sort_by_key(|r| r.priority);
    }

    /// Unregister all hooks for a plugin.
    pub fn unregister_all(&mut self, plugin_name: &str) {
        for handlers in self.registrations.values_mut() {
            handlers.retain(|r| r.plugin_name != plugin_name);
        }
    }

    /// Unregister a specific hook for a plugin.
    pub fn unregister(&mut self, plugin_name: &str, hook: PluginHook) {
        if let Some(handlers) = self.registrations.get_mut(&hook) {
            handlers.retain(|r| r.plugin_name != plugin_name);
        }
    }

    /// Get all handlers for a hook.
    pub fn get_handlers(&self, hook: PluginHook) -> Vec<HookRegistration> {
        self.registrations.get(&hook).cloned().unwrap_or_default()
    }

    /// Set enabled state for all hooks of a plugin.
    pub fn set_enabled(&mut self, plugin_name: &str, enabled: bool) {
        for handlers in self.registrations.values_mut() {
            for handler in handlers.iter_mut() {
                if handler.plugin_name == plugin_name {
                    handler.enabled = enabled;
                }
            }
        }
    }

    /// Get all registered hooks.
    pub fn all_hooks(&self) -> Vec<(PluginHook, Vec<String>)> {
        self.registrations
            .iter()
            .map(|(hook, handlers)| {
                let names: Vec<String> = handlers.iter().map(|h| h.plugin_name.clone()).collect();
                (*hook, names)
            })
            .collect()
    }

    /// Clear all registrations.
    pub fn clear(&mut self) {
        self.registrations.clear();
    }

    /// Get count of registered handlers.
    pub fn handler_count(&self) -> usize {
        self.registrations.values().map(|v| v.len()).sum()
    }
}

/// Context for session-related hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHookContext {
    /// Session ID.
    pub session_id: String,
    /// Session name/title.
    pub session_name: Option<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Model being used.
    pub model: Option<String>,
    /// Provider being used.
    pub provider: Option<String>,
    /// Additional metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl SessionHookContext {
    /// Create new session hook context.
    pub fn new(session_id: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            session_id: session_id.into(),
            session_name: None,
            cwd: cwd.into(),
            model: None,
            provider: None,
            metadata: HashMap::new(),
        }
    }

    /// Set session name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.session_name = Some(name.into());
        self
    }

    /// Set model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set provider.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Convert to generic HookContext.
    pub fn to_hook_context(&self) -> HookContext {
        HookContext::new(&self.session_id, &self.cwd)
            .with_data(serde_json::to_value(self).unwrap_or_default())
    }
}

/// Context for tool-related hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolHookContext {
    /// Session ID.
    pub session_id: String,
    /// Turn ID.
    pub turn_id: Option<String>,
    /// Tool name.
    pub tool_name: String,
    /// Tool arguments.
    pub args: serde_json::Value,
    /// Tool result (for after_call hooks).
    pub result: Option<serde_json::Value>,
    /// Error message (for error hooks).
    pub error: Option<String>,
    /// Execution duration in milliseconds.
    pub duration_ms: Option<u64>,
    /// Working directory.
    pub cwd: PathBuf,
}

impl ToolHookContext {
    /// Create new tool hook context.
    pub fn new(
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        args: serde_json::Value,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            turn_id: None,
            tool_name: tool_name.into(),
            args,
            result: None,
            error: None,
            duration_ms: None,
            cwd: cwd.into(),
        }
    }

    /// Set turn ID.
    pub fn with_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    /// Set result.
    pub fn with_result(mut self, result: serde_json::Value) -> Self {
        self.result = Some(result);
        self
    }

    /// Set error.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Set duration.
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    /// Convert to generic HookContext.
    pub fn to_hook_context(&self) -> HookContext {
        let mut ctx = HookContext::new(&self.session_id, &self.cwd);
        if let Some(ref turn_id) = self.turn_id {
            ctx = ctx.with_turn_id(turn_id);
        }
        ctx.with_data(serde_json::to_value(self).unwrap_or_default())
    }
}

/// Context for message-related hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageHookContext {
    /// Session ID.
    pub session_id: String,
    /// Turn ID.
    pub turn_id: Option<String>,
    /// Message role (user/assistant).
    pub role: String,
    /// Message content.
    pub content: String,
    /// Token count.
    pub token_count: Option<i64>,
    /// Working directory.
    pub cwd: PathBuf,
}

impl MessageHookContext {
    /// Create new message hook context.
    pub fn new(
        session_id: impl Into<String>,
        role: impl Into<String>,
        content: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            turn_id: None,
            role: role.into(),
            content: content.into(),
            token_count: None,
            cwd: cwd.into(),
        }
    }

    /// Set turn ID.
    pub fn with_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    /// Set token count.
    pub fn with_token_count(mut self, count: i64) -> Self {
        self.token_count = Some(count);
        self
    }

    /// Convert to generic HookContext.
    pub fn to_hook_context(&self) -> HookContext {
        let mut ctx = HookContext::new(&self.session_id, &self.cwd);
        if let Some(ref turn_id) = self.turn_id {
            ctx = ctx.with_turn_id(turn_id);
        }
        ctx.with_data(serde_json::to_value(self).unwrap_or_default())
    }
}

/// Context for compaction-related hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionHookContext {
    /// Session ID.
    pub session_id: String,
    /// Current message count.
    pub message_count: usize,
    /// Current token count.
    pub token_count: i64,
    /// Target token count after compaction.
    pub target_tokens: Option<i64>,
    /// Messages to compact (for before hooks).
    pub messages: Option<Vec<serde_json::Value>>,
    /// Compacted summary (for after hooks).
    pub summary: Option<String>,
    /// Working directory.
    pub cwd: PathBuf,
}

impl CompactionHookContext {
    /// Create new compaction hook context.
    pub fn new(
        session_id: impl Into<String>,
        message_count: usize,
        token_count: i64,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            message_count,
            token_count,
            target_tokens: None,
            messages: None,
            summary: None,
            cwd: cwd.into(),
        }
    }

    /// Set target tokens.
    pub fn with_target_tokens(mut self, target: i64) -> Self {
        self.target_tokens = Some(target);
        self
    }

    /// Set messages.
    pub fn with_messages(mut self, messages: Vec<serde_json::Value>) -> Self {
        self.messages = Some(messages);
        self
    }

    /// Set summary.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Convert to generic HookContext.
    pub fn to_hook_context(&self) -> HookContext {
        HookContext::new(&self.session_id, &self.cwd)
            .with_data(serde_json::to_value(self).unwrap_or_default())
    }
}

/// Context for permission-related hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionHookContext {
    /// Session ID.
    pub session_id: String,
    /// Permission being requested.
    pub permission: String,
    /// Resource the permission is for.
    pub resource: String,
    /// Reason for the request.
    pub reason: Option<String>,
    /// Whether permission was granted (for granted/denied hooks).
    pub granted: Option<bool>,
    /// Working directory.
    pub cwd: PathBuf,
}

impl PermissionHookContext {
    /// Create new permission hook context.
    pub fn new(
        session_id: impl Into<String>,
        permission: impl Into<String>,
        resource: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            permission: permission.into(),
            resource: resource.into(),
            reason: None,
            granted: None,
            cwd: cwd.into(),
        }
    }

    /// Set reason.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Set granted status.
    pub fn with_granted(mut self, granted: bool) -> Self {
        self.granted = Some(granted);
        self
    }

    /// Convert to generic HookContext.
    pub fn to_hook_context(&self) -> HookContext {
        HookContext::new(&self.session_id, &self.cwd)
            .with_data(serde_json::to_value(self).unwrap_or_default())
    }
}

/// Context for error-related hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorHookContext {
    /// Session ID.
    pub session_id: String,
    /// Error message.
    pub error: String,
    /// Error code if available.
    pub error_code: Option<String>,
    /// Error source/component.
    pub source: Option<String>,
    /// Whether error is retriable.
    pub retriable: bool,
    /// Retry count.
    pub retry_count: u32,
    /// Working directory.
    pub cwd: PathBuf,
}

impl ErrorHookContext {
    /// Create new error hook context.
    pub fn new(
        session_id: impl Into<String>,
        error: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            error: error.into(),
            error_code: None,
            source: None,
            retriable: false,
            retry_count: 0,
            cwd: cwd.into(),
        }
    }

    /// Set error code.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.error_code = Some(code.into());
        self
    }

    /// Set source.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set retriable flag.
    pub fn with_retriable(mut self, retriable: bool) -> Self {
        self.retriable = retriable;
        self
    }

    /// Set retry count.
    pub fn with_retry_count(mut self, count: u32) -> Self {
        self.retry_count = count;
        self
    }

    /// Convert to generic HookContext.
    pub fn to_hook_context(&self) -> HookContext {
        HookContext::new(&self.session_id, &self.cwd)
            .with_data(serde_json::to_value(self).unwrap_or_default())
    }
}

/// Combined hook result from multiple handlers.
#[derive(Debug, Clone, Default)]
pub struct CombinedHookResult {
    /// Whether to continue execution.
    pub should_continue: bool,
    /// Modified data from handlers.
    pub modified_data: Option<serde_json::Value>,
    /// Injected messages.
    pub injected_messages: Vec<String>,
    /// Warnings from handlers.
    pub warnings: Vec<String>,
    /// Stop reason if execution should stop.
    pub stop_reason: Option<String>,
}

impl CombinedHookResult {
    /// Create a new result that allows continuation.
    pub fn continue_default() -> Self {
        Self {
            should_continue: true,
            ..Default::default()
        }
    }

    /// Create a new result that stops execution.
    pub fn stop(reason: impl Into<String>) -> Self {
        Self {
            should_continue: false,
            stop_reason: Some(reason.into()),
            ..Default::default()
        }
    }

    /// Add a warning.
    pub fn add_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    /// Add an injected message.
    pub fn add_message(&mut self, message: impl Into<String>) {
        self.injected_messages.push(message.into());
    }

    /// Update with modified data.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.modified_data = Some(data);
        self
    }

    /// Merge another result into this one.
    pub fn merge(&mut self, other: CombinedHookResult) {
        // Stop if either says stop
        if !other.should_continue {
            self.should_continue = false;
            if other.stop_reason.is_some() {
                self.stop_reason = other.stop_reason;
            }
        }

        // Merge data (later takes precedence)
        if other.modified_data.is_some() {
            self.modified_data = other.modified_data;
        }

        // Collect messages and warnings
        self.injected_messages.extend(other.injected_messages);
        self.warnings.extend(other.warnings);
    }
}

/// Convert HookResponse to CombinedHookResult.
impl From<HookResponse> for CombinedHookResult {
    fn from(response: HookResponse) -> Self {
        match response {
            HookResponse::Continue => Self::continue_default(),
            HookResponse::ContinueWith { data } => Self::continue_default().with_data(data),
            HookResponse::Stop { reason } => Self {
                should_continue: false,
                stop_reason: reason,
                ..Default::default()
            },
            HookResponse::Skip => Self::continue_default(),
            HookResponse::InjectMessage { message } => {
                let mut result = Self::continue_default();
                result.add_message(message);
                result
            }
            HookResponse::Error { message } => Self {
                should_continue: false,
                stop_reason: Some(message),
                ..Default::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_dispatcher() {
        let mut dispatcher = HookDispatcher::new();

        dispatcher.register(HookRegistration {
            plugin_name: "plugin-a".to_string(),
            hook: PluginHook::SessionStarting,
            priority: 10,
            enabled: true,
        });

        dispatcher.register(HookRegistration {
            plugin_name: "plugin-b".to_string(),
            hook: PluginHook::SessionStarting,
            priority: 5,
            enabled: true,
        });

        let handlers = dispatcher.get_handlers(PluginHook::SessionStarting);
        assert_eq!(handlers.len(), 2);
        // Lower priority should be first
        assert_eq!(handlers[0].plugin_name, "plugin-b");
        assert_eq!(handlers[1].plugin_name, "plugin-a");
    }

    #[test]
    fn test_hook_dispatcher_unregister() {
        let mut dispatcher = HookDispatcher::new();

        dispatcher.register(HookRegistration {
            plugin_name: "plugin-a".to_string(),
            hook: PluginHook::SessionStarting,
            priority: 0,
            enabled: true,
        });

        dispatcher.unregister_all("plugin-a");

        let handlers = dispatcher.get_handlers(PluginHook::SessionStarting);
        assert!(handlers.is_empty());
    }

    #[test]
    fn test_session_hook_context() {
        let ctx = SessionHookContext::new("session-123", "/tmp")
            .with_name("Test Session")
            .with_model("claude-3");

        assert_eq!(ctx.session_id, "session-123");
        assert_eq!(ctx.session_name, Some("Test Session".to_string()));
        assert_eq!(ctx.model, Some("claude-3".to_string()));

        let hook_ctx = ctx.to_hook_context();
        assert_eq!(hook_ctx.session_id, "session-123");
    }

    #[test]
    fn test_tool_hook_context() {
        let ctx = ToolHookContext::new(
            "session-123",
            "read_file",
            serde_json::json!({"path": "/test.txt"}),
            "/tmp",
        )
        .with_duration(100);

        assert_eq!(ctx.tool_name, "read_file");
        assert_eq!(ctx.duration_ms, Some(100));
    }

    #[test]
    fn test_combined_hook_result() {
        let mut result = CombinedHookResult::continue_default();
        result.add_warning("Test warning");
        result.add_message("Test message");

        assert!(result.should_continue);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.injected_messages.len(), 1);

        let other = CombinedHookResult::stop("Stopped by hook");
        result.merge(other);

        assert!(!result.should_continue);
        assert_eq!(result.stop_reason, Some("Stopped by hook".to_string()));
    }
}
