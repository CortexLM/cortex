//! Hook dispatcher for executing hooks in priority order.

use std::sync::Arc;

use super::chat_hooks::{ChatMessageInput, ChatMessageOutput};
use super::permission_hooks::{PermissionAskInput, PermissionAskOutput, PermissionDecision};
use super::registry::HookRegistry;
use super::tool_hooks::{
    ToolExecuteAfterInput, ToolExecuteAfterOutput, ToolExecuteBeforeInput, ToolExecuteBeforeOutput,
};
use super::types::HookResult;
use crate::Result;

/// Dispatcher for executing hooks.
pub struct HookDispatcher {
    registry: Arc<HookRegistry>,
}

impl HookDispatcher {
    /// Create a new dispatcher.
    pub fn new(registry: Arc<HookRegistry>) -> Self {
        Self { registry }
    }

    /// Trigger tool.execute.before hooks.
    pub async fn trigger_tool_execute_before(
        &self,
        input: ToolExecuteBeforeInput,
    ) -> Result<ToolExecuteBeforeOutput> {
        let mut output = ToolExecuteBeforeOutput::new(input.args.clone());
        let hooks = self.registry.tool_execute_before.read().await;

        for registered in hooks.iter() {
            // Check pattern match
            if let Some(pattern) = registered.hook.pattern() {
                if !Self::matches_pattern(&input.tool, pattern) {
                    continue;
                }
            }

            registered.hook.execute(&input, &mut output).await?;

            // Check if we should stop
            match &output.result {
                HookResult::Skip | HookResult::Abort { .. } | HookResult::Replace { .. } => break,
                HookResult::Continue => {}
            }
        }

        Ok(output)
    }

    /// Trigger tool.execute.after hooks.
    pub async fn trigger_tool_execute_after(
        &self,
        input: ToolExecuteAfterInput,
        tool_output: String,
    ) -> Result<ToolExecuteAfterOutput> {
        let mut output = ToolExecuteAfterOutput::new(tool_output);
        let hooks = self.registry.tool_execute_after.read().await;

        for registered in hooks.iter() {
            // Check pattern match
            if let Some(pattern) = registered.hook.pattern() {
                if !Self::matches_pattern(&input.tool, pattern) {
                    continue;
                }
            }

            registered.hook.execute(&input, &mut output).await?;

            match &output.result {
                HookResult::Skip | HookResult::Abort { .. } | HookResult::Replace { .. } => break,
                HookResult::Continue => {}
            }
        }

        Ok(output)
    }

    /// Trigger chat.message hooks.
    pub async fn trigger_chat_message(
        &self,
        input: ChatMessageInput,
        content: String,
    ) -> Result<ChatMessageOutput> {
        let mut output = ChatMessageOutput::new(content);
        let hooks = self.registry.chat_message.read().await;

        for registered in hooks.iter() {
            registered.hook.execute(&input, &mut output).await?;

            match &output.result {
                HookResult::Skip | HookResult::Abort { .. } | HookResult::Replace { .. } => break,
                HookResult::Continue => {}
            }
        }

        Ok(output)
    }

    /// Trigger permission.ask hooks.
    pub async fn trigger_permission_ask(
        &self,
        input: PermissionAskInput,
    ) -> Result<PermissionAskOutput> {
        let mut output = PermissionAskOutput::ask();
        let hooks = self.registry.permission_ask.read().await;

        for registered in hooks.iter() {
            registered.hook.execute(&input, &mut output).await?;

            // Stop if a decision was made
            if output.decision != PermissionDecision::Ask {
                break;
            }
        }

        Ok(output)
    }

    /// Check if a tool name matches a pattern.
    fn matches_pattern(tool: &str, pattern: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        if pattern.contains('*') {
            // Simple glob matching
            let parts: Vec<&str> = pattern.split('*').collect();
            if parts.len() == 2 {
                let prefix = parts[0];
                let suffix = parts[1];
                return tool.starts_with(prefix) && tool.ends_with(suffix);
            }
        }

        tool == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_matching() {
        assert!(HookDispatcher::matches_pattern("read", "read"));
        assert!(HookDispatcher::matches_pattern("read", "*"));
        assert!(HookDispatcher::matches_pattern("read_file", "read*"));
        assert!(HookDispatcher::matches_pattern("async_read", "*read"));
        assert!(!HookDispatcher::matches_pattern("write", "read"));
    }
}
