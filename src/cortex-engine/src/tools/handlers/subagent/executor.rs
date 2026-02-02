//! Subagent executor - runs agents in isolated sessions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, mpsc};
use tokio::time::timeout;
use uuid::Uuid;

use crate::agent::{
    AgentConfig, AgentEvent, Orchestrator, OrchestratorTurnResult, SandboxPolicy, TurnStatus,
};
use crate::agents::{Agent, AgentRegistry};
use crate::client::ModelClient;
use crate::error::{CortexError, Result};
use crate::tools::registry::ToolRegistry;

use super::progress::{ProgressEvent, SubagentProgress};
use super::result::{
    FileChange, FileChangeType, SubagentResult, SubagentResultBuilder, TokenUsageBreakdown,
};
use super::types::{SubagentConfig, SubagentSession, SubagentStatus};

/// Executor for running subagents in isolated sessions.
pub struct SubagentExecutor {
    /// Model client.
    client: Arc<dyn ModelClient>,
    /// Tool registry.
    tools: Arc<ToolRegistry>,
    /// Agent registry.
    agent_registry: Arc<AgentRegistry>,
    /// Active sessions.
    sessions: RwLock<HashMap<String, SubagentSession>>,
    /// Default model.
    default_model: String,
    /// Default sandbox policy.
    default_sandbox_policy: SandboxPolicy,
    /// Maximum concurrent subagents.
    max_concurrent: usize,
    /// Active subagent count.
    active_count: RwLock<usize>,
}

impl SubagentExecutor {
    /// Create a new subagent executor.
    pub fn new(
        client: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
        agent_registry: Arc<AgentRegistry>,
        default_model: impl Into<String>,
    ) -> Self {
        Self {
            client,
            tools,
            agent_registry,
            sessions: RwLock::new(HashMap::new()),
            default_model: default_model.into(),
            default_sandbox_policy: SandboxPolicy::Prompt,
            max_concurrent: 3,
            active_count: RwLock::new(0),
        }
    }

    /// Set sandbox policy.
    pub fn with_sandbox_policy(mut self, policy: SandboxPolicy) -> Self {
        self.default_sandbox_policy = policy;
        self
    }

    /// Set max concurrent subagents.
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max;
        self
    }

    /// Execute a subagent with the given configuration.
    pub async fn execute(
        &self,
        config: SubagentConfig,
        progress_tx: mpsc::UnboundedSender<ProgressEvent>,
    ) -> Result<SubagentResult> {
        // Check concurrent limit
        {
            let count = *self.active_count.read().await;
            if count >= self.max_concurrent {
                return Err(CortexError::RateLimit(format!(
                    "Maximum concurrent subagents ({}) reached",
                    self.max_concurrent
                )));
            }
        }

        // Check if continuing existing session
        if let Some(ref session_id) = config.continue_session_id {
            return self
                .continue_session(session_id, &config.prompt, progress_tx)
                .await;
        }

        // Create new session - use provided session_id or generate a new one
        let session_id = config.session_id.clone().unwrap_or_else(|| {
            format!(
                "sub_{}",
                Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("unknown")
            )
        });
        let session = SubagentSession::new(
            &session_id,
            config.parent_session_id.clone(),
            config.agent_type.clone(),
            &config.description,
            config.working_dir.clone(),
        );

        // Store session
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), session.clone());
        }

        // Increment active count
        {
            let mut count = self.active_count.write().await;
            *count += 1;
        }

        // Run the subagent
        let result = self.run_subagent(session, config, progress_tx).await;

        // Decrement active count
        {
            let mut count = self.active_count.write().await;
            *count = count.saturating_sub(1);
        }

        result
    }

    /// Continue an existing session.
    async fn continue_session(
        &self,
        session_id: &str,
        additional_prompt: &str,
        progress_tx: mpsc::UnboundedSender<ProgressEvent>,
    ) -> Result<SubagentResult> {
        // Get existing session
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };

        let mut session = match session {
            Some(s) => s,
            None => {
                return Err(CortexError::NotFound(format!(
                    "Session not found: {}",
                    session_id
                )));
            }
        };

        // Check if session can be continued
        if !session.status.can_resume() && session.status != SubagentStatus::Completed {
            return Err(CortexError::InvalidInput(format!(
                "Session {} cannot be continued (status: {})",
                session_id, session.status
            )));
        }

        // Update session status
        session.set_status(SubagentStatus::Running);
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.to_string(), session.clone());
        }

        // Create config for continuation
        let config = SubagentConfig::new(
            session.agent_type.clone(),
            format!("Continue: {}", session.description),
            additional_prompt,
            session.working_dir.clone(),
        )
        .with_parent_session(session.parent_id.clone().unwrap_or_default());

        // Increment active count
        {
            let mut count = self.active_count.write().await;
            *count += 1;
        }

        // Run continuation
        let result = self.run_subagent(session, config, progress_tx).await;

        // Decrement active count
        {
            let mut count = self.active_count.write().await;
            *count = count.saturating_sub(1);
        }

        result
    }

    /// Run a subagent.
    async fn run_subagent(
        &self,
        mut session: SubagentSession,
        config: SubagentConfig,
        progress_tx: mpsc::UnboundedSender<ProgressEvent>,
    ) -> Result<SubagentResult> {
        let _start_time = Instant::now();
        let session_id = session.id.clone();

        // Create progress tracker
        let mut progress = SubagentProgress::new(
            &session_id,
            config.agent_type.clone(),
            &config.description,
            progress_tx.clone(),
        );

        progress.set_status(SubagentStatus::Running);
        session.set_status(SubagentStatus::Running);

        // Look up custom agent from registry if this is a Custom type
        let custom_agent = if let Some(agent_name) = config.agent_type.custom_name() {
            match self.agent_registry.get(agent_name).await {
                Some(agent) => {
                    tracing::info!(agent_name = agent_name, "Using custom agent from registry");
                    Some(agent)
                }
                None => {
                    // Agent not found in registry - fail with helpful error
                    return Err(CortexError::NotFound(format!(
                        "Custom agent '{}' not found in registry. Available agents: {:?}",
                        agent_name,
                        self.agent_registry.list_names().await
                    )));
                }
            }
        } else {
            None
        };

        // Build system prompt - use base prompt WITHOUT task details
        // Task details will be sent as user message
        let system_prompt = if let Some(ref agent) = custom_agent {
            // Use the custom agent's base system prompt
            agent.system_prompt.clone()
        } else {
            config.build_base_system_prompt()
        };

        // Build user message containing the task
        // Tasks are conversational - sent as user messages rather than system config
        let user_task_message = if let Some(ref _agent) = custom_agent {
            // For custom agents, format task with context
            let mut message = format!(
                "## Task\n{}\n\n## Instructions\n{}",
                config.description, config.prompt
            );
            if let Some(ref context) = config.context {
                message.push_str("\n\n## Additional Context\n");
                message.push_str(context);
            }
            message.push_str("\n\nPlease complete this task and provide a clear summary of your findings or actions when done.");
            message
        } else {
            config.build_user_message()
        };

        // Determine model - custom agent can override
        let model = if let Some(ref agent) = custom_agent {
            agent.effective_model(&self.default_model)
        } else {
            config
                .model
                .clone()
                .unwrap_or_else(|| self.default_model.clone())
        };

        // Determine max iterations - custom agent can override
        let max_iterations = if let Some(ref agent) = custom_agent {
            agent
                .metadata
                .max_turns
                .unwrap_or(config.effective_max_iterations())
        } else {
            config.effective_max_iterations()
        };

        let agent_config = AgentConfig {
            model,
            max_tool_iterations: max_iterations,
            max_output_tokens: 16384,
            tool_timeout: Duration::from_secs(120),
            sandbox_policy: self.default_sandbox_policy,
            auto_approve_safe: true, // Subagents auto-approve safe operations
            streaming: true,         // Enable streaming for progressive feedback
            system_prompt: Some(system_prompt.clone()),
            working_directory: config.working_dir.clone(),
            ..AgentConfig::default()
        };

        // Custom agent tool restrictions planned for future implementation
        // This would require modifying the tool registry or orchestrator to filter tools

        // Create orchestrator for the subagent
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let orchestrator = Orchestrator::new(
            self.client.clone(),
            self.tools.clone(),
            agent_config,
            event_tx,
        );

        // Initialize the orchestrator
        orchestrator.initialize(Some(&system_prompt)).await;

        // Set up event forwarding
        let progress_tx_clone = progress_tx.clone();
        let session_id_clone = session_id.clone();
        let event_handler = tokio::spawn(async move {
            let mut files_modified = Vec::new();
            let mut turn_number: u32 = 0; // Track actual turn number
            while let Some(event) = event_rx.recv().await {
                match &event {
                    AgentEvent::Thinking => {
                        // Increment turn number each time model is called
                        turn_number += 1;
                        let _ = progress_tx_clone.send(ProgressEvent::Thinking {
                            session_id: session_id_clone.clone(),
                            turn_number,
                        });
                    }
                    AgentEvent::TextDelta { content } => {
                        let _ = progress_tx_clone.send(ProgressEvent::TextOutput {
                            session_id: session_id_clone.clone(),
                            content: content.clone(),
                            is_partial: true,
                        });
                    }
                    AgentEvent::ToolCallStarted {
                        id,
                        name,
                        arguments,
                    } => {
                        let _ = progress_tx_clone.send(ProgressEvent::ToolCallStarted {
                            session_id: session_id_clone.clone(),
                            tool_name: name.clone(),
                            tool_id: id.clone(),
                            arguments_preview: arguments.chars().take(200).collect(),
                        });
                    }
                    AgentEvent::ToolCallCompleted { id, name, result } => {
                        // Track file modifications
                        if matches!(
                            name.as_str(),
                            "Create" | "Edit" | "ApplyPatch" | "MultiEdit"
                        ) {
                            // Extract file path from result if possible
                            if let Some(path) = extract_file_path(&result.output) {
                                files_modified.push(path);
                            }
                        }

                        let _ = progress_tx_clone.send(ProgressEvent::ToolCallCompleted {
                            session_id: session_id_clone.clone(),
                            tool_name: name.clone(),
                            tool_id: id.clone(),
                            success: result.success,
                            output_preview: result.output.chars().take(200).collect(),
                            duration_ms: 0, // Not tracked at event level
                        });
                    }
                    AgentEvent::ToolCallPending {
                        id,
                        name,
                        arguments,
                        risk_level,
                    } => {
                        let _ = progress_tx_clone.send(ProgressEvent::ToolCallPending {
                            session_id: session_id_clone.clone(),
                            tool_name: name.clone(),
                            tool_id: id.clone(),
                            arguments: arguments.clone(),
                            risk_level: format!("{:?}", risk_level),
                        });
                    }
                    AgentEvent::Error {
                        message,
                        recoverable,
                    } => {
                        if *recoverable {
                            let _ = progress_tx_clone.send(ProgressEvent::Warning {
                                session_id: session_id_clone.clone(),
                                message: message.clone(),
                            });
                        } else {
                            let _ = progress_tx_clone.send(ProgressEvent::Failed {
                                session_id: session_id_clone.clone(),
                                error: message.clone(),
                                recoverable: false,
                            });
                        }
                    }
                    _ => {}
                }
            }
            files_modified
        });

        // Create turn context with the full user task message
        // This sends the task as a user message, not embedded in system prompt
        let turn_id = session.turns_completed as u64 + 1;
        let mut turn_ctx = crate::agent::TurnContext::new(
            turn_id,
            session_id.clone(),
            user_task_message, // Use the formatted user message with task details
            config.working_dir.clone(),
        );

        // Execute with optional timeout (no timeout by default)
        let mut turn_result = if let Some(timeout_duration) = config.effective_timeout() {
            // With timeout
            match timeout(timeout_duration, orchestrator.process_turn(&mut turn_ctx)).await {
                Ok(result) => result,
                Err(_) => {
                    progress.fail("Execution timed out", true);
                    session.set_status(SubagentStatus::TimedOut);
                    return Err(CortexError::Timeout);
                }
            }
        } else {
            // No timeout - run until completion
            orchestrator.process_turn(&mut turn_ctx).await
        };

        // MANDATORY: Request explicit summary if the response doesn't contain one
        // This ensures subagents always provide structured output for the orchestrator
        if let Ok(ref result) = turn_result {
            if result.status == TurnStatus::Completed && !has_summary_output(&result.response) {
                tracing::info!(
                    session_id = %session_id,
                    "Subagent output missing summary, requesting explicit summary turn"
                );

                // Request a summary turn
                let summary_prompt = SUMMARY_REQUEST_PROMPT.to_string();

                let summary_turn_id = session.turns_completed as u64 + 2;
                let mut summary_turn_ctx = crate::agent::TurnContext::new(
                    summary_turn_id,
                    session_id.clone(),
                    summary_prompt,
                    config.working_dir.clone(),
                );

                // Execute summary turn with a reasonable timeout
                let summary_result = timeout(
                    Duration::from_secs(60),
                    orchestrator.process_turn(&mut summary_turn_ctx),
                )
                .await;

                // Update turn_result with the summary if successful
                if let Ok(Ok(summary_response)) = summary_result {
                    if summary_response.status == TurnStatus::Completed
                        && !summary_response.response.is_empty()
                    {
                        tracing::info!(
                            session_id = %session_id,
                            "Received explicit summary from subagent"
                        );
                        // Combine original response with summary
                        turn_result = Ok(OrchestratorTurnResult {
                            turn_id: result.turn_id,
                            status: TurnStatus::Completed,
                            response: format!(
                                "{}\n\n{}",
                                result.response, summary_response.response
                            ),
                            tool_calls: result.tool_calls.clone(),
                            token_usage: result.token_usage.clone(),
                            duration: result.duration,
                        });
                        // Update token counts
                        turn_ctx.tokens.input_tokens += summary_turn_ctx.tokens.input_tokens;
                        turn_ctx.tokens.output_tokens += summary_turn_ctx.tokens.output_tokens;
                        turn_ctx.tokens.cached_tokens += summary_turn_ctx.tokens.cached_tokens;
                        turn_ctx.tokens.reasoning_tokens +=
                            summary_turn_ctx.tokens.reasoning_tokens;
                    }
                }
            }
        }

        // CRITICAL: Drop orchestrator to close the event channel
        // This allows event_handler to exit its recv() loop
        // Without this, we'd have a deadlock:
        // - event_handler.await waits for the task to finish
        // - The task waits for event_rx.recv() to return None
        // - recv() returns None only when all senders (event_tx) are dropped
        // - event_tx is owned by orchestrator, which won't drop until after await
        drop(orchestrator);

        // Wait for event handler to finish (now safe since channel is closed)
        let files_modified = event_handler.await.unwrap_or_default();

        // Process result
        // Extract status for better error messages when success=false but no error
        let (success, output, error, status_info) = match &turn_result {
            Ok(result) => {
                let success = result.status == TurnStatus::Completed;
                let status_info = if !success {
                    Some(format!("{:?}", result.status))
                } else {
                    None
                };
                (success, result.response.clone(), None, status_info)
            }
            Err(e) => (false, String::new(), Some(e.to_string()), None),
        };

        // Update session
        session.record_turn(
            turn_ctx.tool_results.len() as u32,
            turn_ctx.tokens.total_tokens as u64,
        );
        for path in &files_modified {
            session.record_file_modified(path);
        }
        session.set_status(if success {
            SubagentStatus::Completed
        } else {
            // Any non-success state should be marked as Failed
            SubagentStatus::Failed
        });

        // Store updated session
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), session.clone());
        }

        // Build token usage breakdown
        let mut token_usage = TokenUsageBreakdown::default();
        token_usage.add_turn(
            turn_ctx.tokens.input_tokens as u64,
            turn_ctx.tokens.output_tokens as u64,
            turn_ctx.tokens.cached_tokens as u64,
            turn_ctx.tokens.reasoning_tokens as u64,
        );

        // Build file changes
        let file_changes: Vec<FileChange> = files_modified
            .into_iter()
            .map(|path| FileChange::new(path, FileChangeType::Modified))
            .collect();

        // Record completion or failure
        if success {
            progress.complete(&output);
        } else if let Some(ref err) = error {
            progress.fail(err, false);
        } else {
            // Handle non-success without explicit error (interrupted, cancelled, etc.)
            // This can happen when turn_result is Ok but status != Completed
            let error_msg = if let Some(ref status) = status_info {
                format!("Task ended with status: {}", status)
            } else {
                "Task did not complete successfully".to_string()
            };
            progress.fail(&error_msg, true);
        }

        // Build result
        let mut builder = SubagentResultBuilder::new(session)
            .success(success)
            .output(&output)
            .tokens(token_usage);

        // Add error to result - either from explicit error or from status_info
        if let Some(err) = error {
            builder = builder.error(err);
        } else if let Some(ref status) = status_info {
            builder = builder.error(format!("Task ended with status: {}", status));
        }

        for change in file_changes {
            builder = builder.file_changed(change);
        }

        // Allow continuation if partially complete
        if success || turn_ctx.tool_iterations < config.effective_max_iterations() {
            builder = builder.continuable();
        }

        Ok(builder.build())
    }

    /// Get a session by ID.
    pub async fn get_session(&self, session_id: &str) -> Option<SubagentSession> {
        self.sessions.read().await.get(session_id).cloned()
    }

    /// List all sessions.
    pub async fn list_sessions(&self) -> Vec<SubagentSession> {
        self.sessions.read().await.values().cloned().collect()
    }

    /// Get active session count.
    pub async fn active_count(&self) -> usize {
        *self.active_count.read().await
    }

    /// Cancel a session.
    pub async fn cancel_session(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.set_status(SubagentStatus::Cancelled);
            Ok(())
        } else {
            Err(CortexError::NotFound(format!(
                "Session not found: {}",
                session_id
            )))
        }
    }

    /// Get available subagent types with descriptions.
    pub fn available_types(&self) -> Vec<SubagentTypeInfo> {
        vec![
            SubagentTypeInfo {
                name: "code".to_string(),
                description: "General-purpose coding agent with full tool access. Use for implementing features, fixing bugs, and writing code.".to_string(),
                allowed_tools: None,
                denied_tools: vec![],
            },
            SubagentTypeInfo {
                name: "research".to_string(),
                description: "Read-only research agent for investigation. Use for understanding code, finding patterns, and gathering information. Cannot modify files.".to_string(),
                allowed_tools: Some(vec!["Read", "Grep", "Glob", "LS", "FetchUrl", "WebSearch"].into_iter().map(String::from).collect()),
                denied_tools: vec!["Create", "Edit", "ApplyPatch", "MultiEdit", "Execute"].into_iter().map(String::from).collect(),
            },
            SubagentTypeInfo {
                name: "refactor".to_string(),
                description: "Refactoring agent for code improvements. Use for restructuring, renaming, and cleaning up code.".to_string(),
                allowed_tools: None,
                denied_tools: vec![],
            },
            SubagentTypeInfo {
                name: "test".to_string(),
                description: "Testing agent for writing and running tests. Use for creating test cases and improving coverage.".to_string(),
                allowed_tools: None,
                denied_tools: vec![],
            },
            SubagentTypeInfo {
                name: "documentation".to_string(),
                description: "Documentation agent for writing docs. Use for creating README files, API docs, and comments.".to_string(),
                allowed_tools: None,
                denied_tools: vec![],
            },
            SubagentTypeInfo {
                name: "security".to_string(),
                description: "Security audit agent. Use for finding vulnerabilities, checking configurations, and reviewing access controls.".to_string(),
                allowed_tools: Some(vec!["Read", "Grep", "Glob", "LS", "Execute"].into_iter().map(String::from).collect()),
                denied_tools: vec![],
            },
            SubagentTypeInfo {
                name: "architect".to_string(),
                description: "Architecture planning agent. Use for designing systems, planning refactors, and making technical decisions. Cannot modify files.".to_string(),
                allowed_tools: Some(vec!["Read", "Grep", "Glob", "LS", "WebSearch"].into_iter().map(String::from).collect()),
                denied_tools: vec!["Create", "Edit", "ApplyPatch", "MultiEdit", "Execute"].into_iter().map(String::from).collect(),
            },
            SubagentTypeInfo {
                name: "reviewer".to_string(),
                description: "Code review agent. Use for reviewing changes, finding bugs, and suggesting improvements. Cannot modify files.".to_string(),
                allowed_tools: Some(vec!["Read", "Grep", "Glob", "LS"].into_iter().map(String::from).collect()),
                denied_tools: vec!["Create", "Edit", "ApplyPatch", "MultiEdit", "Execute"].into_iter().map(String::from).collect(),
            },
        ]
    }

    /// Get custom agents from the registry.
    pub async fn custom_agents(&self) -> Vec<Agent> {
        self.agent_registry.list().await
    }
}

/// Information about a subagent type.
#[derive(Debug, Clone)]
pub struct SubagentTypeInfo {
    /// Type name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Allowed tools (None = all).
    pub allowed_tools: Option<Vec<String>>,
    /// Denied tools.
    pub denied_tools: Vec<String>,
}

/// Extract file path from tool output.
/// Prompt used to request an explicit summary from a subagent when none was provided.
/// Ensures structured output from agents for orchestrator consumption.
const SUMMARY_REQUEST_PROMPT: &str = r#"You have completed your work but did not provide a summary. Please provide a final summary NOW using EXACTLY this format:

## Summary for Orchestrator

### Tasks Completed
- [List each task you completed with brief outcome]

### Key Findings/Changes
- [Main discoveries or modifications made]

### Files Modified (if any)
- [List of files with type of change]

### Recommendations (if applicable)
- [Any follow-up actions or suggestions]

### Status: COMPLETED

DO NOT use any tools. Just provide the summary based on the work you have already done."#;

/// Check if the response contains a proper summary for the orchestrator.
/// Returns true if summary markers are present, false otherwise.
fn has_summary_output(response: &str) -> bool {
    // Empty responses definitely don't have a summary
    if response.trim().is_empty() {
        return false;
    }

    // Check for key summary markers that indicate structured output
    let summary_markers = [
        "## Summary for Orchestrator",
        "### Tasks Completed",
        "### Key Findings",
        "### Status: COMPLETED",
        "Status: COMPLETED",
        // Also accept some variations
        "## Summary",
        "### Summary",
        "## Final Summary",
        "### Final Summary",
    ];

    let response_lower = response.to_lowercase();
    summary_markers
        .iter()
        .any(|marker| response_lower.contains(&marker.to_lowercase()))
}

fn extract_file_path(output: &str) -> Option<String> {
    // Try to extract path from common patterns
    // "Created file: path/to/file"
    // "Edited path/to/file"
    // "Wrote N bytes to path/to/file"

    let patterns = [
        "Created file: ",
        "Created: ",
        "Edited ",
        "Modified ",
        "Wrote ",
        "to ",
    ];

    for pattern in patterns {
        if let Some(idx) = output.find(pattern) {
            let rest = &output[idx + pattern.len()..];
            // Extract until whitespace or end
            let path: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
            if !path.is_empty() && (path.contains('/') || path.contains('\\') || path.contains('.'))
            {
                return Some(path);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_file_path() {
        assert_eq!(
            extract_file_path("Created file: src/main.rs"),
            Some("src/main.rs".to_string())
        );
        assert_eq!(
            extract_file_path("Wrote 100 bytes to config.json"),
            Some("config.json".to_string())
        );
        assert_eq!(
            extract_file_path("Successfully edited src/lib.rs"),
            None // "edited" doesn't match "Edited "
        );
        assert_eq!(extract_file_path("No path here"), None);
    }

    #[test]
    fn test_has_summary_output_with_proper_summary() {
        let response_with_summary = r#"
## Summary for Orchestrator

### Tasks Completed
- Analyzed the codebase structure

### Key Findings
- Found 10 modules

### Status: COMPLETED
"#;
        assert!(has_summary_output(response_with_summary));
    }

    #[test]
    fn test_has_summary_output_with_variation() {
        // Test case-insensitive matching
        let response = "## summary\nSome content here";
        assert!(has_summary_output(response));

        // Test "Status: COMPLETED" alone
        let response2 = "Work done.\n\nStatus: COMPLETED";
        assert!(has_summary_output(response2));
    }

    #[test]
    fn test_has_summary_output_empty() {
        assert!(!has_summary_output(""));
        assert!(!has_summary_output("   "));
        assert!(!has_summary_output("\n\n"));
    }

    #[test]
    fn test_has_summary_output_no_markers() {
        let response_without_summary = "I analyzed the code and found some issues.";
        assert!(!has_summary_output(response_without_summary));

        let response_partial = "Here are some findings:\n- Item 1\n- Item 2";
        assert!(!has_summary_output(response_partial));
    }

    #[test]
    fn test_subagent_type_info() {
        let executor = SubagentExecutor::new(
            Arc::new(MockClient::new()),
            Arc::new(ToolRegistry::new()),
            Arc::new(AgentRegistry::new(&std::path::PathBuf::from("/tmp"), None)),
            "gpt-4o",
        );

        let types = executor.available_types();
        assert!(!types.is_empty());
        assert!(types.iter().any(|t| t.name == "code"));
        assert!(types.iter().any(|t| t.name == "research"));
    }

    // Mock client for testing
    struct MockClient {
        capabilities: crate::client::types::ModelCapabilities,
    }

    impl MockClient {
        fn new() -> Self {
            Self {
                capabilities: crate::client::types::ModelCapabilities::default(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ModelClient for MockClient {
        fn model(&self) -> &str {
            "mock-model"
        }

        fn provider(&self) -> &str {
            "mock-provider"
        }

        fn capabilities(&self) -> &crate::client::types::ModelCapabilities {
            &self.capabilities
        }

        async fn complete(
            &self,
            _request: crate::client::types::CompletionRequest,
        ) -> crate::error::Result<crate::client::ResponseStream> {
            Err(crate::error::CortexError::Internal("Mock".into()))
        }

        async fn complete_sync(
            &self,
            _request: crate::client::types::CompletionRequest,
        ) -> crate::error::Result<crate::client::types::CompletionResponse> {
            Err(crate::error::CortexError::Internal("Mock".into()))
        }
    }
}
