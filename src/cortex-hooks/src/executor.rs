//! Hook executor for running hooks.

use crate::{Hook, HookContext, HookResult, HookType};
use std::collections::HashSet;
use std::fmt;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;
use tokio::process::Command;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

/// Internal error type for hook execution.
#[derive(Debug)]
enum HookExecutionError {
    /// Hook command timed out.
    Timeout,
    /// Failed to spawn the command.
    Spawn(std::io::Error),
    /// Hook has an empty command.
    EmptyCommand,
}

impl fmt::Display for HookExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HookExecutionError::Timeout => write!(f, "Hook execution timed out"),
            HookExecutionError::Spawn(e) => write!(f, "Failed to spawn hook command: {}", e),
            HookExecutionError::EmptyCommand => write!(f, "Hook has empty command"),
        }
    }
}

/// Executor for running hooks.
pub struct HookExecutor {
    /// Registered hooks.
    hooks: RwLock<Vec<Hook>>,
    /// Default timeout in seconds.
    default_timeout: u64,
    /// Maximum number of concurrent hook executions.
    max_concurrent: usize,
    /// Semaphore to limit concurrent executions.
    semaphore: Arc<Semaphore>,
    /// Set of hook IDs that have been executed with `once` flag.
    executed_once: Arc<RwLock<HashSet<String>>>,
}

impl HookExecutor {
    pub fn new() -> Self {
        Self {
            hooks: RwLock::new(Vec::new()),
            default_timeout: 30,
            max_concurrent: 10,
            semaphore: Arc::new(Semaphore::new(10)),
            executed_once: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.default_timeout = secs;
        self
    }

    /// Set the maximum number of concurrent hook executions.
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max;
        self.semaphore = Arc::new(Semaphore::new(max));
        self
    }

    /// Reset the "executed once" tracking (e.g., for a new session).
    pub async fn reset_once_tracking(&self) {
        self.executed_once.write().await.clear();
    }

    /// Register a hook.
    pub async fn register(&self, hook: Hook) {
        self.hooks.write().await.push(hook);
    }

    /// Register multiple hooks.
    pub async fn register_all(&self, hooks: Vec<Hook>) {
        self.hooks.write().await.extend(hooks);
    }

    /// Unregister a hook by ID.
    pub async fn unregister(&self, hook_id: &str) {
        self.hooks.write().await.retain(|h| h.id != hook_id);
    }

    /// Run hooks for a specific event type.
    pub async fn run(&self, hook_type: HookType, context: &HookContext) -> Vec<HookResult> {
        let hooks = self.hooks.read().await;
        let matching: Vec<_> = hooks
            .iter()
            .filter(|h| h.enabled && h.hook_type == hook_type)
            .filter(|h| {
                // Check file pattern match
                if let Some(ref path) = context.file_path {
                    if !h.matches_file(&path.to_string_lossy()) {
                        return false;
                    }
                }
                // Check tool matcher for tool-related hooks
                if let Some(ref tool_name) = context.tool_name {
                    if !h.matches_tool(tool_name) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        drop(hooks);

        let mut results = Vec::new();
        for hook in matching {
            let result = self.execute_hook(&hook, context).await;
            let should_continue = result.success || hook.continue_on_error;
            results.push(result);

            if !should_continue {
                break;
            }
        }

        results
    }

    /// Run hooks for tool-related events (PreToolUse, PostToolUse, PostToolUseFailure).
    pub async fn on_tool_use(
        &self,
        hook_type: HookType,
        tool_name: &str,
        tool_args: &str,
        tool_result: Option<&str>,
    ) -> Vec<HookResult> {
        let mut context = HookContext::new().with_tool(tool_name, tool_args);
        if let Some(result) = tool_result {
            context = context.with_tool_result(result);
        }
        self.run(hook_type, &context).await
    }

    /// Run hooks for subagent events.
    pub async fn on_subagent(
        &self,
        hook_type: HookType,
        agent_id: &str,
        agent_name: &str,
        parent_agent_id: Option<&str>,
    ) -> Vec<HookResult> {
        let mut context = HookContext::new().with_agent(agent_id, agent_name);
        if let Some(parent_id) = parent_agent_id {
            context = context.with_parent_agent(parent_id);
        }
        self.run(hook_type, &context).await
    }

    /// Run session start hooks.
    pub async fn on_session_start(&self, session_id: &str) -> Vec<HookResult> {
        let context = HookContext::new().with_session(session_id);
        self.run(HookType::SessionStart, &context).await
    }

    /// Run session end hooks.
    pub async fn on_session_end(&self, session_id: &str) -> Vec<HookResult> {
        let context = HookContext::new().with_session(session_id);
        self.run(HookType::SessionEnd, &context).await
    }

    /// Run file edited hooks.
    pub async fn on_file_edited(&self, file_path: &str) -> Vec<HookResult> {
        let context = HookContext::new().with_file(file_path);
        self.run(HookType::FileEdited, &context).await
    }

    /// Run file created hooks.
    pub async fn on_file_created(&self, file_path: &str) -> Vec<HookResult> {
        let context = HookContext::new().with_file(file_path);
        self.run(HookType::FileCreated, &context).await
    }

    /// Run session completed hooks.
    pub async fn on_session_completed(&self, session_id: &str) -> Vec<HookResult> {
        let context = HookContext::new().with_session(session_id);
        self.run(HookType::SessionCompleted, &context).await
    }

    /// Execute a single hook (handles sync/async and once logic).
    async fn execute_hook(&self, hook: &Hook, context: &HookContext) -> HookResult {
        // Check if this hook has the `once` flag and was already executed
        if hook.once {
            let executed = self.executed_once.read().await;
            if executed.contains(&hook.id) {
                debug!("Skipping hook {} (already executed once)", hook.id);
                return HookResult::skipped(&hook.id, "Already executed (once)");
            }
        }

        if hook.async_execution {
            self.execute_async(hook, context).await
        } else {
            self.execute_sync(hook, context).await
        }
    }

    /// Execute a hook asynchronously (fire-and-forget).
    async fn execute_async(&self, hook: &Hook, context: &HookContext) -> HookResult {
        let hook_clone = hook.clone();
        let hook_id = hook.id.clone();
        let context = context.clone();
        let executed_once = self.executed_once.clone();
        let semaphore = self.semaphore.clone();
        let timeout =
            std::time::Duration::from_secs(hook.timeout_secs.unwrap_or(self.default_timeout));

        debug!("Starting async hook {}", hook_id);

        tokio::spawn(async move {
            let _permit = match semaphore.acquire().await {
                Ok(p) => p,
                Err(e) => {
                    error!(
                        "Failed to acquire semaphore for hook {}: {}",
                        hook_clone.id, e
                    );
                    return;
                }
            };

            let result = Self::run_command(&hook_clone, &context, timeout).await;

            if hook_clone.once {
                executed_once.write().await.insert(hook_clone.id.clone());
            }

            match result {
                Ok((_, stderr, exit_code)) => {
                    if exit_code == 0 {
                        info!("Async hook {} completed successfully", hook_clone.id);
                    } else {
                        warn!(
                            "Async hook {} failed with code {}: {}",
                            hook_clone.id, exit_code, stderr
                        );
                    }
                }
                Err(e) => {
                    warn!("Async hook {} failed: {}", hook_clone.id, e);
                }
            }
        });

        // Return immediately without waiting
        HookResult::async_started(&hook_id)
    }

    /// Execute a hook synchronously (blocking).
    async fn execute_sync(&self, hook: &Hook, context: &HookContext) -> HookResult {
        let start = Instant::now();

        let _permit = match self.semaphore.acquire().await {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to acquire semaphore for hook {}: {}", hook.id, e);
                return HookResult::failure(&hook.id, format!("Semaphore error: {}", e), 0);
            }
        };

        let timeout =
            std::time::Duration::from_secs(hook.timeout_secs.unwrap_or(self.default_timeout));

        let result = Self::run_command(hook, context, timeout).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        if hook.once {
            self.executed_once.write().await.insert(hook.id.clone());
        }

        match result {
            Ok((stdout, stderr, exit_code)) => {
                if exit_code == 0 {
                    info!(
                        "Hook {} completed successfully in {}ms",
                        hook.id, duration_ms
                    );
                    HookResult::success(&hook.id, duration_ms)
                        .with_output(stdout, stderr)
                        .with_exit_code(exit_code)
                } else {
                    warn!(
                        "Hook {} failed with code {}: {}",
                        hook.id, exit_code, stderr
                    );
                    HookResult::failure(&hook.id, &stderr, duration_ms)
                        .with_output(stdout, stderr)
                        .with_exit_code(exit_code)
                }
            }
            Err(HookExecutionError::Timeout) => {
                error!("Hook {} timed out", hook.id);
                HookResult::timeout(&hook.id, duration_ms)
            }
            Err(HookExecutionError::Spawn(e)) => {
                error!("Hook {} execution error: {}", hook.id, e);
                HookResult::failure(&hook.id, e.to_string(), duration_ms)
            }
            Err(HookExecutionError::EmptyCommand) => {
                error!("Hook {} has empty command", hook.id);
                HookResult::failure(&hook.id, "Empty command", duration_ms)
            }
        }
    }

    /// Run the actual command.
    async fn run_command(
        hook: &Hook,
        context: &HookContext,
        timeout: std::time::Duration,
    ) -> Result<(String, String, i32), HookExecutionError> {
        let cmd = hook.build_command(context);

        if cmd.is_empty() {
            return Err(HookExecutionError::EmptyCommand);
        }

        debug!("Executing hook {}: {:?}", hook.id, cmd);

        let mut command = Command::new(&cmd[0]);
        if cmd.len() > 1 {
            command.args(&cmd[1..]);
        }

        // Set environment from hook config
        for (key, value) in &hook.environment {
            command.env(key, value);
        }

        // Add context to environment (using as_env for all context vars)
        for (key, value) in context.as_env() {
            command.env(&key, &value);
        }

        // Legacy environment variables for backwards compatibility
        if let Some(ref path) = context.file_path {
            command.env("CORTEX_FILE", path.to_string_lossy().to_string());
        }
        if let Some(ref session_id) = context.session_id {
            command.env("CORTEX_SESSION_ID", session_id);
        }
        if let Some(ref message_id) = context.message_id {
            command.env("CORTEX_MESSAGE_ID", message_id);
        }

        // Set working directory
        if let Some(ref cwd) = hook.cwd {
            command.current_dir(cwd);
        } else if let Some(ref path) = context.file_path {
            if let Some(parent) = path.parent() {
                command.current_dir(parent);
            }
        }

        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let result = tokio::time::timeout(timeout, command.output())
            .await
            .map_err(|_| HookExecutionError::Timeout)?
            .map_err(HookExecutionError::Spawn)?;

        let stdout = String::from_utf8_lossy(&result.stdout).to_string();
        let stderr = String::from_utf8_lossy(&result.stderr).to_string();
        let exit_code = result.status.code().unwrap_or(-1);

        Ok((stdout, stderr, exit_code))
    }

    /// Check if command exists.
    pub async fn command_exists(cmd: &str) -> bool {
        #[cfg(windows)]
        let result = Command::new("where")
            .arg(cmd)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        #[cfg(not(windows))]
        let result = Command::new("which")
            .arg(cmd)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        result
    }
}

impl Default for HookExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HookStatus;

    #[tokio::test]
    #[cfg_attr(windows, ignore = "Unix shell commands not available on Windows")]
    async fn test_hook_executor() {
        let executor = HookExecutor::new();

        let hook = Hook::new(
            "echo-test",
            HookType::FileEdited,
            vec!["echo".to_string(), "Hello".to_string()],
        );

        executor.register(hook).await;

        let test_file = std::env::temp_dir().join("test.txt");
        let results = executor.on_file_edited(test_file.to_str().unwrap()).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "Unix shell commands not available on Windows")]
    async fn test_hook_pattern_matching() {
        let executor = HookExecutor::new();

        let hook = Hook::new("rs-only", HookType::FileEdited, vec!["echo".to_string()])
            .with_pattern("*.rs");

        executor.register(hook).await;

        // Should match .rs files
        let test_rs_file = std::env::temp_dir().join("main.rs");
        let results = executor
            .on_file_edited(test_rs_file.to_str().unwrap())
            .await;
        assert_eq!(results.len(), 1);

        // Should not match .py files
        let test_py_file = std::env::temp_dir().join("main.py");
        let results = executor
            .on_file_edited(test_py_file.to_str().unwrap())
            .await;
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "Unix shell commands not available on Windows")]
    async fn test_async_hook_execution() {
        let executor = HookExecutor::new();
        let hook = Hook::new(
            "async-test",
            HookType::PostToolUse,
            vec!["echo".to_string(), "async".to_string()],
        )
        .async_exec();

        executor.register(hook).await;

        let context = HookContext::new().with_tool("Edit", "{}");
        let results = executor.run(HookType::PostToolUse, &context).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, HookStatus::AsyncStarted));
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "Unix shell commands not available on Windows")]
    async fn test_once_hook() {
        let executor = HookExecutor::new();
        let hook = Hook::new(
            "once-test",
            HookType::SessionStart,
            vec!["echo".to_string(), "once".to_string()],
        )
        .once();

        executor.register(hook).await;

        let context = HookContext::new().with_session("test-session");

        // First execution should succeed
        let results = executor.run(HookType::SessionStart, &context).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert!(matches!(results[0].status, HookStatus::Success));

        // Second execution should be skipped
        let results = executor.run(HookType::SessionStart, &context).await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, HookStatus::Skipped));
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "Unix shell commands not available on Windows")]
    async fn test_tool_matcher() {
        let executor = HookExecutor::new();
        let hook = Hook::new(
            "edit-only",
            HookType::PreToolUse,
            vec!["echo".to_string(), "edit".to_string()],
        )
        .with_tool_matcher("Edit|Create");

        executor.register(hook).await;

        // Should match Edit
        let results = executor
            .on_tool_use(HookType::PreToolUse, "Edit", "{}", None)
            .await;
        assert_eq!(results.len(), 1);

        // Should match Create
        let results = executor
            .on_tool_use(HookType::PreToolUse, "Create", "{}", None)
            .await;
        assert_eq!(results.len(), 1);

        // Should not match Execute
        let results = executor
            .on_tool_use(HookType::PreToolUse, "Execute", "{}", None)
            .await;
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_reset_once_tracking() {
        let executor = HookExecutor::new();
        let hook = Hook::new(
            "reset-test",
            HookType::SessionStart,
            vec!["echo".to_string(), "test".to_string()],
        )
        .once();

        executor.register(hook).await;

        let context = HookContext::new().with_session("test-session");

        // First execution
        let results = executor.run(HookType::SessionStart, &context).await;
        assert!(matches!(results[0].status, HookStatus::Success));

        // Second execution (skipped)
        let results = executor.run(HookType::SessionStart, &context).await;
        assert!(matches!(results[0].status, HookStatus::Skipped));

        // Reset tracking
        executor.reset_once_tracking().await;

        // Third execution (should work again)
        let results = executor.run(HookType::SessionStart, &context).await;
        assert!(matches!(results[0].status, HookStatus::Success));
    }
}
