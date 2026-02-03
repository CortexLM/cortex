//! Integration layer between cortex-engine and cortex-plugins hook systems.
//!
//! This module provides a bridge that allows the cortex-engine to trigger hooks
//! defined in the cortex-plugins crate and receive results. It handles the mapping
//! between the engine's internal types and the plugin system's types.

use std::sync::Arc;

use cortex_plugins_ext::{
    HookDispatcher as PluginsHookDispatcher, HookRegistry, HookResult as PluginsHookResult,
    PermissionAskInput, PermissionDecision, SessionEndInput, SessionEndOutput, SessionStartInput,
    SessionStartOutput, ToolExecuteAfterInput, ToolExecuteAfterOutput, ToolExecuteBeforeInput,
    ToolExecuteBeforeOutput,
};

use crate::error::{CortexError, Result};

/// Result returned from tool hooks.
#[derive(Debug, Clone)]
pub struct ToolHookResult {
    /// Modified tool arguments (for before hooks).
    pub args: Option<serde_json::Value>,
    /// Modified tool output (for after hooks).
    pub output: Option<String>,
    /// Whether to continue with execution.
    pub should_continue: bool,
    /// Abort reason if hook decided to abort.
    pub abort_reason: Option<String>,
    /// Replacement value if hook decided to replace the result.
    pub replacement: Option<serde_json::Value>,
}

impl Default for ToolHookResult {
    fn default() -> Self {
        Self {
            args: None,
            output: None,
            should_continue: true,
            abort_reason: None,
            replacement: None,
        }
    }
}

impl From<ToolExecuteBeforeOutput> for ToolHookResult {
    fn from(output: ToolExecuteBeforeOutput) -> Self {
        let (should_continue, abort_reason, replacement) = match output.result {
            PluginsHookResult::Continue => (true, None, None),
            PluginsHookResult::Skip => (true, None, None),
            PluginsHookResult::Abort { reason } => (false, Some(reason), None),
            PluginsHookResult::Replace { result } => (true, None, Some(result)),
        };

        Self {
            args: Some(output.args),
            output: None,
            should_continue,
            abort_reason,
            replacement,
        }
    }
}

impl From<ToolExecuteAfterOutput> for ToolHookResult {
    fn from(output: ToolExecuteAfterOutput) -> Self {
        let (should_continue, abort_reason, replacement) = match output.result {
            PluginsHookResult::Continue => (true, None, None),
            PluginsHookResult::Skip => (true, None, None),
            PluginsHookResult::Abort { reason } => (false, Some(reason), None),
            PluginsHookResult::Replace { result } => (true, None, Some(result)),
        };

        Self {
            args: None,
            output: Some(output.output),
            should_continue,
            abort_reason,
            replacement,
        }
    }
}

/// Result returned from session hooks.
#[derive(Debug, Clone)]
pub struct SessionHookResult {
    /// System prompt additions from the hook.
    pub system_prompt_additions: Vec<String>,
    /// Greeting message from the hook.
    pub greeting: Option<String>,
    /// Whether to continue with execution.
    pub should_continue: bool,
    /// Abort reason if hook decided to abort.
    pub abort_reason: Option<String>,
}

impl Default for SessionHookResult {
    fn default() -> Self {
        Self {
            system_prompt_additions: Vec::new(),
            greeting: None,
            should_continue: true,
            abort_reason: None,
        }
    }
}

impl From<SessionStartOutput> for SessionHookResult {
    fn from(output: SessionStartOutput) -> Self {
        let (should_continue, abort_reason) = match output.result {
            PluginsHookResult::Continue | PluginsHookResult::Skip => (true, None),
            PluginsHookResult::Abort { reason } => (false, Some(reason)),
            PluginsHookResult::Replace { .. } => (true, None),
        };

        Self {
            system_prompt_additions: output.system_prompt_additions,
            greeting: output.greeting,
            should_continue,
            abort_reason,
        }
    }
}

impl From<SessionEndOutput> for SessionHookResult {
    fn from(output: SessionEndOutput) -> Self {
        let (should_continue, abort_reason) = match output.result {
            PluginsHookResult::Continue | PluginsHookResult::Skip => (true, None),
            PluginsHookResult::Abort { reason } => (false, Some(reason)),
            PluginsHookResult::Replace { .. } => (true, None),
        };

        Self {
            system_prompt_additions: Vec::new(),
            greeting: None,
            should_continue,
            abort_reason,
        }
    }
}

/// Integration bridge between cortex-engine and cortex-plugins hook systems.
///
/// This struct provides a unified interface to trigger plugin hooks from
/// the engine and map results back to engine-compatible types.
#[derive(Clone)]
pub struct PluginIntegration {
    /// The plugins hook dispatcher.
    dispatcher: Arc<PluginsHookDispatcher>,
}

impl std::fmt::Debug for PluginIntegration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginIntegration")
            .field("dispatcher", &"<HookDispatcher>")
            .finish()
    }
}

impl PluginIntegration {
    /// Create a new plugin integration with the given hook registry.
    pub fn new(registry: Arc<HookRegistry>) -> Self {
        Self {
            dispatcher: Arc::new(PluginsHookDispatcher::new(registry)),
        }
    }

    /// Create a new plugin integration from an existing dispatcher.
    pub fn from_dispatcher(dispatcher: Arc<PluginsHookDispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Trigger the tool.execute.before hook.
    ///
    /// This hook is called before a tool is executed, allowing plugins to:
    /// - Modify the tool arguments
    /// - Abort the tool execution
    /// - Replace the result entirely
    ///
    /// # Arguments
    ///
    /// * `tool` - The name of the tool being executed
    /// * `session_id` - The current session ID
    /// * `args` - The tool arguments as a JSON value
    ///
    /// # Returns
    ///
    /// A `ToolHookResult` containing any modifications or decisions from plugins.
    pub async fn trigger_tool_before(
        &self,
        tool: &str,
        session_id: &str,
        args: serde_json::Value,
    ) -> Result<ToolHookResult> {
        let input = ToolExecuteBeforeInput {
            tool: tool.to_string(),
            session_id: session_id.to_string(),
            call_id: uuid::Uuid::new_v4().to_string(),
            args,
        };

        let output = self
            .dispatcher
            .trigger_tool_execute_before(input)
            .await
            .map_err(|e| CortexError::Internal(format!("Plugin hook error: {}", e)))?;

        Ok(ToolHookResult::from(output))
    }

    /// Trigger the tool.execute.after hook.
    ///
    /// This hook is called after a tool has executed, allowing plugins to:
    /// - Modify the tool output
    /// - Log or analyze the result
    /// - Abort further processing
    ///
    /// # Arguments
    ///
    /// * `tool` - The name of the tool that was executed
    /// * `session_id` - The current session ID
    /// * `success` - Whether the tool execution succeeded
    /// * `duration_ms` - The execution duration in milliseconds
    /// * `result` - The tool's output as a string
    ///
    /// # Returns
    ///
    /// A `ToolHookResult` containing any modifications or decisions from plugins.
    pub async fn trigger_tool_after(
        &self,
        tool: &str,
        session_id: &str,
        success: bool,
        duration_ms: u64,
        result: &str,
    ) -> Result<ToolHookResult> {
        let input = ToolExecuteAfterInput {
            tool: tool.to_string(),
            session_id: session_id.to_string(),
            call_id: uuid::Uuid::new_v4().to_string(),
            success,
            duration_ms,
        };

        let output = self
            .dispatcher
            .trigger_tool_execute_after(input, result.to_string())
            .await
            .map_err(|e| CortexError::Internal(format!("Plugin hook error: {}", e)))?;

        Ok(ToolHookResult::from(output))
    }

    /// Trigger the session.start hook.
    ///
    /// This hook is called when a new session starts, allowing plugins to:
    /// - Add system prompt content
    /// - Provide initial context
    /// - Set a custom greeting message
    ///
    /// # Arguments
    ///
    /// * `session_id` - The new session's ID
    /// * `cwd` - The working directory for the session
    /// * `model` - Optional model name being used
    /// * `agent` - Optional agent name being used
    /// * `resumed` - Whether this is a resumed session
    ///
    /// # Returns
    ///
    /// A `SessionHookResult` containing any additions from plugins.
    pub async fn trigger_session_start(
        &self,
        session_id: &str,
        cwd: &std::path::Path,
        model: Option<&str>,
        agent: Option<&str>,
        resumed: bool,
    ) -> Result<SessionHookResult> {
        let input = SessionStartInput {
            session_id: session_id.to_string(),
            agent: agent.map(|s| s.to_string()),
            model: model.map(|s| s.to_string()),
            cwd: cwd.to_path_buf(),
            resumed,
        };

        // The dispatcher doesn't have a direct trigger_session_start method,
        // so we need to handle this at the registry level if hooks are registered.
        // For now, we return a default result since the dispatcher only handles
        // tool, chat, and permission hooks.
        //
        // In a full implementation, the HookDispatcher would need to be extended
        // to support session hooks, or we'd interact directly with the registry.
        let output = SessionStartOutput::new();

        // Log that session start was triggered (useful for debugging)
        tracing::debug!(
            session_id = %session_id,
            cwd = %cwd.display(),
            model = ?model,
            agent = ?agent,
            resumed = resumed,
            "Session start hook triggered (no plugins registered)"
        );

        let _ = input; // Suppress unused warning

        Ok(SessionHookResult::from(output))
    }

    /// Trigger the session.end hook.
    ///
    /// This hook is called when a session ends, allowing plugins to:
    /// - Generate summaries
    /// - Export chat logs
    /// - Perform cleanup
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session's ID
    /// * `duration_secs` - The session duration in seconds
    /// * `total_messages` - The total number of messages in the session
    /// * `total_tokens` - The total tokens used (if available)
    /// * `saved` - Whether the session was saved
    ///
    /// # Returns
    ///
    /// A `SessionHookResult` indicating any final actions from plugins.
    pub async fn trigger_session_end(
        &self,
        session_id: &str,
        duration_secs: u64,
        total_messages: usize,
        total_tokens: Option<u64>,
        saved: bool,
    ) -> Result<SessionHookResult> {
        let input = SessionEndInput {
            session_id: session_id.to_string(),
            duration_secs,
            total_messages,
            total_tokens,
            saved,
        };

        // Similar to session start, the current dispatcher doesn't have a direct method.
        // Return a default result for now.
        let output = SessionEndOutput::new();

        // Log that session end was triggered
        tracing::debug!(
            session_id = %session_id,
            duration_secs = duration_secs,
            total_messages = total_messages,
            total_tokens = ?total_tokens,
            saved = saved,
            "Session end hook triggered (no plugins registered)"
        );

        let _ = input; // Suppress unused warning

        Ok(SessionHookResult::from(output))
    }

    /// Trigger the permission.ask hook.
    ///
    /// This hook is called when a permission is requested, allowing plugins to:
    /// - Automatically allow certain permissions (trusted plugins only)
    /// - Automatically deny certain permissions
    /// - Defer to the user for a decision
    ///
    /// # Arguments
    ///
    /// * `session_id` - The current session ID
    /// * `permission` - The permission being requested (e.g., "file_read")
    /// * `resource` - The resource the permission is for (e.g., a file path)
    /// * `reason` - Optional reason for the permission request
    ///
    /// # Returns
    ///
    /// The permission decision from the plugins.
    ///
    /// # Security
    ///
    /// Note that only trusted system plugins should be allowed to return `Allow`.
    /// Third-party plugins returning `Allow` could be a security risk.
    pub async fn trigger_permission_ask(
        &self,
        session_id: &str,
        permission: &str,
        resource: &str,
        reason: Option<&str>,
    ) -> Result<PermissionDecision> {
        let input = PermissionAskInput {
            session_id: session_id.to_string(),
            permission: permission.to_string(),
            resource: resource.to_string(),
            reason: reason.map(|s| s.to_string()),
        };

        let output = self
            .dispatcher
            .trigger_permission_ask(input)
            .await
            .map_err(|e| CortexError::Internal(format!("Plugin hook error: {}", e)))?;

        // Validate that third-party plugins aren't auto-granting permissions
        if output.decision.requires_elevated_trust() {
            tracing::warn!(
                permission = %permission,
                resource = %resource,
                "Permission auto-granted by plugin - ensure plugin is trusted"
            );
        }

        Ok(output.decision)
    }

    /// Trigger the chat.message hook.
    ///
    /// This hook is called when a chat message is processed, allowing plugins to:
    /// - Modify message content
    /// - Add metadata
    /// - Abort message processing
    ///
    /// # Arguments
    ///
    /// * `session_id` - The current session ID
    /// * `role` - The message role (user/assistant)
    /// * `content` - The message content
    ///
    /// # Returns
    ///
    /// The potentially modified message content.
    pub async fn trigger_chat_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
    ) -> Result<String> {
        use cortex_plugins_ext::ChatMessageInput;

        let input = ChatMessageInput {
            session_id: session_id.to_string(),
            role: role.to_string(),
            message_id: None,
            agent: None,
            model: None,
        };

        let output = self
            .dispatcher
            .trigger_chat_message(input, content.to_string())
            .await
            .map_err(|e| CortexError::Internal(format!("Plugin hook error: {}", e)))?;

        Ok(output.content)
    }

    /// Check if any hooks are registered for tool execution.
    ///
    /// This can be used to skip hook triggering when no plugins are interested,
    /// improving performance.
    pub fn has_tool_hooks(&self) -> bool {
        // The dispatcher always exists, so we consider hooks available.
        // In a more sophisticated implementation, we'd check the registry.
        true
    }

    /// Check if any hooks are registered for permission decisions.
    pub fn has_permission_hooks(&self) -> bool {
        true
    }
}

/// Builder for creating PluginIntegration instances.
pub struct PluginIntegrationBuilder {
    registry: Option<Arc<HookRegistry>>,
}

impl PluginIntegrationBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self { registry: None }
    }

    /// Set the hook registry to use.
    pub fn with_registry(mut self, registry: Arc<HookRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Build the PluginIntegration instance.
    ///
    /// If no registry was provided, creates a new empty registry.
    pub fn build(self) -> PluginIntegration {
        let registry = self.registry.unwrap_or_else(|| Arc::new(HookRegistry::new()));
        PluginIntegration::new(registry)
    }
}

impl Default for PluginIntegrationBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_hook_result_default() {
        let result = ToolHookResult::default();
        assert!(result.should_continue);
        assert!(result.abort_reason.is_none());
        assert!(result.replacement.is_none());
    }

    #[test]
    fn test_session_hook_result_default() {
        let result = SessionHookResult::default();
        assert!(result.should_continue);
        assert!(result.system_prompt_additions.is_empty());
        assert!(result.greeting.is_none());
    }

    #[test]
    fn test_plugin_integration_builder() {
        let integration = PluginIntegrationBuilder::new().build();
        assert!(integration.has_tool_hooks());
        assert!(integration.has_permission_hooks());
    }

    #[test]
    fn test_plugin_integration_with_registry() {
        let registry = Arc::new(HookRegistry::new());
        let integration = PluginIntegrationBuilder::new()
            .with_registry(registry)
            .build();
        assert!(integration.has_tool_hooks());
    }

    #[tokio::test]
    async fn test_trigger_permission_ask_default() {
        let integration = PluginIntegrationBuilder::new().build();

        let result = integration
            .trigger_permission_ask("session-1", "file_read", "/tmp/test.txt", None)
            .await;

        assert!(result.is_ok());
        // Default should be Ask since no plugins are registered
        assert_eq!(result.unwrap(), PermissionDecision::Ask);
    }

    #[tokio::test]
    async fn test_trigger_tool_before_default() {
        let integration = PluginIntegrationBuilder::new().build();

        let result = integration
            .trigger_tool_before(
                "read_file",
                "session-1",
                serde_json::json!({"path": "/test.txt"}),
            )
            .await;

        assert!(result.is_ok());
        let hook_result = result.unwrap();
        assert!(hook_result.should_continue);
        // Args should be preserved
        assert!(hook_result.args.is_some());
    }

    #[tokio::test]
    async fn test_trigger_tool_after_default() {
        let integration = PluginIntegrationBuilder::new().build();

        let result = integration
            .trigger_tool_after("read_file", "session-1", true, 100, "file content")
            .await;

        assert!(result.is_ok());
        let hook_result = result.unwrap();
        assert!(hook_result.should_continue);
        assert_eq!(hook_result.output, Some("file content".to_string()));
    }

    #[tokio::test]
    async fn test_trigger_session_start_default() {
        let integration = PluginIntegrationBuilder::new().build();

        let result = integration
            .trigger_session_start(
                "session-1",
                std::path::Path::new("/workspace"),
                Some("gpt-4"),
                None,
                false,
            )
            .await;

        assert!(result.is_ok());
        let hook_result = result.unwrap();
        assert!(hook_result.should_continue);
        assert!(hook_result.system_prompt_additions.is_empty());
    }

    #[tokio::test]
    async fn test_trigger_session_end_default() {
        let integration = PluginIntegrationBuilder::new().build();

        let result = integration
            .trigger_session_end("session-1", 300, 10, Some(1000), true)
            .await;

        assert!(result.is_ok());
        let hook_result = result.unwrap();
        assert!(hook_result.should_continue);
    }

    #[tokio::test]
    async fn test_trigger_chat_message_default() {
        let integration = PluginIntegrationBuilder::new().build();

        let result = integration
            .trigger_chat_message("session-1", "user", "Hello, world!")
            .await;

        assert!(result.is_ok());
        // Content should be preserved when no plugins modify it
        assert_eq!(result.unwrap(), "Hello, world!");
    }
}
