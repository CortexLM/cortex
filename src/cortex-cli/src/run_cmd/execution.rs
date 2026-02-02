//! Main execution logic for the run command.

use anyhow::{Context, Result, bail};
use std::io::{self, IsTerminal, Read, Write};
use std::time::Duration;

use crate::styled_output::{print_success, print_warning};
use crate::utils::{TermColor, get_tool_display};
use cortex_common::resolve_model_alias;
use cortex_common::{resolve_model_with_info, warn_if_ambiguous_model};
use cortex_engine::{Session, list_sessions};
use cortex_protocol::{EventMsg, Op, Submission, UserInput};

use super::attachments::{FileAttachment, process_file_attachments};
use super::cli::{OutputFormat, RunCli};
use super::output::{copy_to_clipboard, send_notification};
use super::session::{SessionMode, resolve_session_id};
use super::system::check_file_descriptor_limits;

impl RunCli {
    /// Run the command.
    pub async fn run(self) -> Result<()> {
        // Check file descriptor limits early to provide helpful error message
        check_file_descriptor_limits()?;

        // Validate temperature if provided using epsilon-based comparison
        // to handle floating-point boundary issues (#2179)
        const EPSILON: f32 = 1e-6;
        if let Some(temp) = self.temperature {
            if !(-EPSILON..=2.0 + EPSILON).contains(&temp) {
                bail!("Temperature must be between 0.0 and 2.0, got {temp}");
            }
            // Warn about temperature=0 having inconsistent behavior across providers
            if temp == 0.0 {
                eprintln!(
                    "\x1b[1;33mNote:\x1b[0m temperature=0 behavior varies by provider. Some interpret it as \
                     'greedy/deterministic' while others treat it as 'use default'. \
                     Consider using a small value like 0.001 for consistent deterministic output."
                );
            }
        }

        // Validate top_p if provided using epsilon-based comparison
        if let Some(top_p) = self.top_p
            && (!(-EPSILON..=1.0 + EPSILON).contains(&top_p))
        {
            bail!("top-p must be between 0.0 and 1.0, got {top_p}");
        }

        // Validate top_k if provided
        if let Some(top_k) = self.top_k
            && top_k == 0
        {
            bail!("top-k must be a positive integer, got {top_k}");
        }

        // Warn if both temperature and top-p are specified (#2175)
        if self.temperature.is_some() && self.top_p.is_some() {
            eprintln!(
                "{}Warning:{} Using both --temperature and --top-p together may produce unpredictable results. \
                Most LLM providers recommend using only one sampling parameter at a time.",
                TermColor::Yellow.ansi_code(),
                TermColor::Default.ansi_code()
            );
        }

        // Warn about potential parameter compatibility issues
        if self.temperature.is_some() && self.top_k.is_some() {
            eprintln!(
                "Warning: Combining --temperature with --top-k may not be supported by all model providers. \
                If you encounter API errors, try using only one of these parameters."
            );
        }

        // Validate command is not empty if provided
        if let Some(ref cmd) = self.command
            && cmd.trim().is_empty()
        {
            bail!("Error: Command cannot be empty");
        }

        // Build the message from arguments
        let mut message = self
            .message
            .iter()
            .map(|arg| {
                if arg.contains(' ') {
                    format!("\"{}\"", arg.replace('"', "\\\""))
                } else {
                    arg.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        // Read from stdin if not a TTY (piped input)
        if !io::stdin().is_terminal() {
            let mut stdin_content = String::new();
            io::stdin().lock().read_to_string(&mut stdin_content)?;
            if !stdin_content.is_empty() {
                if !message.is_empty() {
                    message.push('\n');
                }
                message.push_str(&stdin_content);
            }
        }

        // Validate we have something to do
        if message.trim().is_empty() && self.command.is_none() {
            bail!("You must provide a message or a --command");
        }

        // Process file attachments
        let attachments = process_file_attachments(&self.files, self.cwd.as_ref()).await?;

        // Determine session handling
        let session_mode = if self.continue_session {
            SessionMode::ContinueLast
        } else if let Some(ref id) = self.session_id {
            SessionMode::Continue(id.clone())
        } else {
            let title = self.title.as_ref().map(|t| {
                if t.is_empty() {
                    // Use truncated prompt as title
                    if message.len() > 50 {
                        format!("{}...", &message[..50])
                    } else {
                        message.clone()
                    }
                } else {
                    t.clone()
                }
            });
            SessionMode::New { title }
        };

        // Execute based on whether we're attaching to a server or running locally
        if let Some(ref server_url) = self.attach {
            self.run_attached(server_url, &message, &attachments, session_mode)
                .await
        } else {
            self.run_local(&message, &attachments, session_mode).await
        }
    }

    /// Run attached to a remote server.
    async fn run_attached(
        &self,
        _server_url: &str,
        message: &str,
        attachments: &[FileAttachment],
        session_mode: SessionMode,
    ) -> Result<()> {
        // For now, we'll implement a basic HTTP client approach
        // In a full implementation, this would use a proper SDK client

        if self.verbose {
            eprintln!("Attaching to server: {_server_url}");
            eprintln!("Session mode: {session_mode:?}");
            eprintln!("Message length: {} chars", message.len());
            eprintln!("Attachments: {}", attachments.len());
        }

        // Server attachment not yet fully implemented - fall back to local execution
        print_warning("Server attachment not yet fully implemented. Running locally instead.");
        self.run_local(message, attachments, session_mode).await
    }

    /// Run locally with a new session.
    async fn run_local(
        &self,
        message: &str,
        attachments: &[FileAttachment],
        session_mode: SessionMode,
    ) -> Result<()> {
        // Handle dry-run mode - show token estimates without executing
        if self.dry_run {
            return self.run_dry_run(message, attachments).await;
        }

        // Use --output if provided, otherwise use --format
        let effective_format = self.output.unwrap_or(self.format);
        let is_json = matches!(effective_format, OutputFormat::Json | OutputFormat::Jsonl);
        let is_terminal = io::stdout().is_terminal();
        let streaming_enabled = self.is_streaming_enabled();

        // Handle --dry-run flag: show what would happen without executing
        if self.dry_run {
            println!("Dry Run Mode - Preview Only");
            println!("{}", "=".repeat(50));
            println!();
            println!(
                "Message: {}",
                if message.len() > 200 {
                    format!("{}...", &message[..200])
                } else {
                    message.to_string()
                }
            );
            println!();
            println!("Configuration:");
            println!("  Model: {}", self.model.as_deref().unwrap_or("default"));
            println!("  Agent: {}", self.agent.as_deref().unwrap_or("default"));
            println!("  Attachments: {}", attachments.len());
            println!("  Session Mode: {:?}", session_mode);
            println!("  Stream: {}", self.stream);
            println!("  Timeout: {}s", self.timeout);
            if let Some(temp) = self.temperature {
                println!("  Temperature: {}", temp);
            }
            if self.no_cache {
                println!("  Cache: disabled");
            }
            if self.retry > 0 {
                println!("  Retry: {} attempts", self.retry);
            }
            println!();
            println!("(No action taken in dry-run mode)");
            return Ok(());
        }

        // Create or resume session
        let mut config = cortex_engine::Config::default();

        // Set agent if provided
        if let Some(ref agent_name) = self.agent {
            config.current_agent = Some(agent_name.clone());
        }

        // Resolve model alias if provided (e.g., "sonnet" -> "anthropic/claude-sonnet-4-20250514")
        // Also warn if the model name was ambiguous and multiple models matched
        if let Some(ref model) = self.model {
            let resolution = resolve_model_with_info(model);
            warn_if_ambiguous_model(&resolution, model);
            config.model = resolution.model.clone();

            // Issue #2326: Warn if --stream is used with a model that may not support streaming
            // Known non-streaming or limited-streaming models
            let non_streaming_patterns = [
                "embedding",
                "text-embedding",
                "ada-002",
                "text-search",
                "text-similarity",
            ];
            let model_lower = resolution.model.to_lowercase();
            if streaming_enabled {
                let is_embedding_model = non_streaming_patterns
                    .iter()
                    .any(|p| model_lower.contains(p));
                if is_embedding_model {
                    eprintln!(
                        "{}Warning:{} Model '{}' appears to be an embedding model which does not support streaming. \
                        Response will be returned as a batch despite --stream flag.",
                        TermColor::Yellow.ansi_code(),
                        TermColor::Default.ansi_code(),
                        model
                    );
                }
            }
        }

        // Apply temperature override if provided
        if let Some(temp) = self.temperature {
            config.temperature = Some(temp);
        }

        // Initialize custom command registry if not already initialized
        let project_root = self.cwd.clone().or_else(|| std::env::current_dir().ok());
        let _custom_registry = cortex_engine::init_custom_command_registry(
            &config.cortex_home,
            project_root.as_deref(),
        );
        if let Err(e) = _custom_registry.scan().await {
            tracing::warn!("Failed to scan custom commands: {}", e);
        }

        let (mut session, handle) = Session::new(config.clone())?;

        // Handle session resumption if needed
        let session_id = match session_mode {
            SessionMode::ContinueLast => {
                let sessions = list_sessions(&config.cortex_home)?;
                if sessions.is_empty() {
                    bail!("No sessions found to continue");
                }
                let last_session = &sessions[0];
                if self.verbose {
                    eprintln!("Continuing session: {}", last_session.id);
                }
                // For now, we create a new session but log the continuation
                // Full implementation would load the session state
                uuid::Uuid::new_v4().to_string()
            }
            SessionMode::Continue(id) => {
                // Validate that the session exists before continuing
                let conversation_id = resolve_session_id(&id, &config.cortex_home)?;
                let validated_id = conversation_id.to_string();
                if self.verbose {
                    eprintln!("Continuing session: {validated_id}");
                }
                validated_id
            }
            SessionMode::New { title } => {
                let id = uuid::Uuid::new_v4().to_string();
                if let Some(ref t) = title
                    && self.verbose
                {
                    eprintln!("New session: {id} (title: {t})");
                }
                id
            }
        };

        // Spawn session task
        let session_task = tokio::spawn(async move { session.run().await });

        // Build user input parts
        let mut input_parts = Vec::new();

        // Add file attachments
        for attachment in attachments {
            // Read file content
            let content = std::fs::read_to_string(&attachment.path)
                .with_context(|| format!("Failed to read file: {}", attachment.path.display()))?;

            input_parts.push(UserInput::Text {
                text: format!(
                    "--- File: {} ---\n{}\n--- End of {} ---",
                    attachment.filename, content, attachment.filename
                ),
            });
        }

        // Add the main message
        if !message.is_empty() {
            input_parts.push(UserInput::Text {
                text: message.to_string(),
            });
        }

        // Pre-validate token count if max_tokens is specified
        // This prevents API errors by checking limits before making the request
        if let Some(max_tokens) = self.max_tokens {
            // Estimate prompt tokens (rough approximation: 4 chars per token)
            let total_text: String = input_parts
                .iter()
                .filter_map(|p| match p {
                    UserInput::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            let estimated_prompt_tokens = (total_text.len() / 4) as u32;

            // Default context limit for most models (can be made configurable)
            let context_limit: u32 = 128000; // Conservative default

            let total_tokens = estimated_prompt_tokens + max_tokens;
            if total_tokens > context_limit {
                let available_for_response = context_limit.saturating_sub(estimated_prompt_tokens);
                eprintln!(
                    "{}Warning:{} Token usage may exceed model context limit",
                    TermColor::Yellow.ansi_code(),
                    TermColor::Default.ansi_code()
                );
                eprintln!("  Estimated prompt tokens: ~{}", estimated_prompt_tokens);
                eprintln!("  Requested max_tokens: {}", max_tokens);
                eprintln!(
                    "  Total: ~{} (context limit: {})",
                    total_tokens, context_limit
                );
                eprintln!(
                    "  Suggestion: Reduce prompt or set --max-tokens {}",
                    available_for_response
                );

                // Don't bail, just warn - the actual API will give a definitive error
                // This is a pre-validation hint to help users
            } else if self.verbose {
                eprintln!(
                    "Token validation: ~{} prompt + {} max = ~{} (limit: {})",
                    estimated_prompt_tokens, max_tokens, total_tokens, context_limit
                );
            }
        }

        // If using a command, try to expand it from custom commands registry
        let final_input = if let Some(ref cmd) = self.command {
            // Try to get the custom command registry
            if let Some(registry) = cortex_engine::try_custom_command_registry() {
                // Try to execute the custom command
                let ctx = cortex_engine::TemplateContext::new(message.to_string()).with_cwd(
                    std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                );

                // Use blocking runtime to get the command
                let prompt = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(async { registry.execute(cmd, &ctx).await })
                });

                if let Some(result) = prompt {
                    vec![UserInput::Text {
                        text: result.prompt,
                    }]
                } else {
                    // Fallback to simple format if command not found
                    vec![UserInput::Text {
                        text: format!("Execute command: {} with arguments: {}", cmd, message),
                    }]
                }
            } else {
                // No registry, use simple format
                vec![UserInput::Text {
                    text: format!("Execute command: {} with arguments: {}", cmd, message),
                }]
            }
        } else {
            input_parts
        };

        // Send the submission
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::UserInput { items: final_input },
        };
        handle.submission_tx.send(submission).await?;

        // Process events
        let mut final_message = String::new();
        let mut event_count = 0u64;
        let mut streaming_started = false;
        let mut error_occurred = false;
        let mut task_completed = false;
        let mut interrupted = false;
        let mut response_truncated = false; // Track if response was truncated (#2174)

        // Set up timeout if specified
        let timeout_duration = if self.timeout > 0 {
            Some(Duration::from_secs(self.timeout))
        } else {
            None
        };

        let start_time = std::time::Instant::now();

        while let Ok(event) = handle.event_rx.recv().await {
            // Check timeout
            if let Some(timeout) = timeout_duration
                && start_time.elapsed() > timeout
            {
                eprintln!("Timeout reached after {} seconds", self.timeout);
                interrupted = true;
                break;
            }

            event_count += 1;

            // Output JSON event if in JSON mode (JSONL format with full event data)
            if is_json {
                // Serialize the full event using serde, which properly includes all data
                // and uses the correct type tags from the EventMsg enum's serde attributes
                let event_json = serde_json::json!({
                    "event_id": event_count,
                    "session_id": session_id,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                    "event": event.msg,
                });

                // Output as a single JSON line (JSONL format)
                println!("{}", serde_json::to_string(&event_json)?);
                // Flush immediately for piped output to ensure streaming works
                io::stdout().flush()?;
            }

            match &event.msg {
                EventMsg::AgentMessage(msg) => {
                    final_message = msg.message.clone();
                    // Check if response was truncated due to token limit (#2174)
                    if let Some(ref reason) = msg.finish_reason
                        && reason == "length"
                    {
                        response_truncated = true;
                        if !is_json {
                            eprintln!(
                                "{}Warning:{} Response was truncated due to max token limit.",
                                TermColor::Yellow.ansi_code(),
                                TermColor::Default.ansi_code()
                            );
                        }
                    }
                }
                EventMsg::AgentMessageDelta(delta) => {
                    // Handle streaming output
                    if streaming_enabled && !is_json {
                        if !streaming_started && is_terminal {
                            println!();
                            streaming_started = true;
                        }
                        print!("{}", delta.delta);
                        // Handle BrokenPipe (SIGPIPE) - stop processing if downstream closes
                        // This prevents wasting API tokens when output is piped to commands
                        // like `head` that close early
                        match io::stdout().flush() {
                            Ok(()) => {}
                            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                                // Pipe closed, stop processing to save API tokens
                                eprintln!("\nPipe closed, stopping generation to save API tokens.");
                                break;
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                    final_message.push_str(&delta.delta);
                }
                EventMsg::ExecCommandBegin(cmd_begin) => {
                    if !is_json && is_terminal && !self.quiet && !self.no_progress {
                        let display = get_tool_display("bash");
                        let title = cmd_begin.command.join(" ");
                        println!(
                            "{}|{} {:<7} {}{}",
                            display.color.ansi_code(),
                            TermColor::Default.ansi_code(),
                            display.name,
                            TermColor::Default.ansi_code(),
                            title
                        );
                    }
                }
                EventMsg::ExecCommandOutputDelta(_output_delta) => {
                    // Output delta is base64 encoded, skip for now in verbose mode
                }
                EventMsg::ExecCommandEnd(cmd_end) => {
                    if !is_json && is_terminal && self.verbose {
                        let exit_code = cmd_end.exit_code;
                        if exit_code != 0 {
                            eprintln!(
                                "{}Command exited with code: {}{}",
                                TermColor::Yellow.ansi_code(),
                                exit_code,
                                TermColor::Default.ansi_code()
                            );
                        }
                    }
                }
                EventMsg::McpToolCallBegin(mcp_begin) => {
                    if !is_json && is_terminal && !self.quiet && !self.no_progress {
                        let display = get_tool_display(&mcp_begin.invocation.tool);
                        println!(
                            "{}|{} {:<7} {}{}",
                            display.color.ansi_code(),
                            TermColor::Default.ansi_code(),
                            display.name,
                            TermColor::Default.ansi_code(),
                            mcp_begin.invocation.tool
                        );
                    }
                }
                EventMsg::McpToolCallEnd(_) => {
                    // Tool call completed
                }
                EventMsg::TaskComplete(_) => {
                    task_completed = true;
                    break;
                }
                EventMsg::Error(e) => {
                    error_occurred = true;
                    // Print truncation indicator if we were streaming and had partial content
                    if streaming_started && !final_message.is_empty() && !is_json {
                        println!();
                        eprintln!(
                            "{}⚠️  [RESPONSE TRUNCATED]{}",
                            TermColor::Yellow.ansi_code(),
                            TermColor::Default.ansi_code()
                        );
                    }
                    if is_json {
                        let err_json = serde_json::json!({
                            "type": "error",
                            "message": e.message,
                            "session_id": session_id,
                            "response_truncated": streaming_started && !final_message.is_empty(),
                        });
                        eprintln!("{}", serde_json::to_string(&err_json)?);
                    } else {
                        eprintln!(
                            "{}Error:{} {}",
                            TermColor::Red.ansi_code(),
                            TermColor::Default.ansi_code(),
                            e.message
                        );
                    }
                    break;
                }
                _ => {}
            }
        }

        // Verify task completion and warn about partial responses
        if !task_completed && !error_occurred {
            interrupted = true;
            if !is_json {
                eprintln!(
                    "{}Warning:{} Response may be incomplete. The task did not complete successfully.",
                    TermColor::Yellow.ansi_code(),
                    TermColor::Default.ansi_code()
                );
            }
        }

        // Final output handling
        // Note: Empty responses are valid - some models may legitimately return
        // empty content for certain queries. We don't treat this as an error.
        if streaming_enabled && streaming_started {
            println!();
            if is_terminal {
                println!();
            }
        } else if !streaming_enabled && !is_json {
            if is_terminal {
                println!();
            }
            if final_message.is_empty() {
                // Empty response is valid - model may have nothing to say
                // or the task was completed with tool calls only
                if self.verbose {
                    eprintln!("[no text response]");
                }
            } else {
                println!("{}", final_message);
            }
            if is_terminal {
                println!();
            }
        }

        // Handle JSON result output
        // This ensures valid JSON output even when interrupted, addressing the issue
        // where partial JSON output would be left unclosed on interruption.
        if matches!(effective_format, OutputFormat::Json) {
            let result = serde_json::json!({
                "type": "result",
                "session_id": session_id,
                "message": final_message,
                "events": event_count,
                "success": !error_occurred && !interrupted,
                "interrupted": interrupted,
                "complete": task_completed,
                "truncated": response_truncated,
                "finish_reason": if interrupted { "timeout" } else if response_truncated { "length" } else if error_occurred { "error" } else { "stop" },
            });
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        // Copy to clipboard if requested
        if self.copy && !final_message.is_empty() {
            if copy_to_clipboard(&final_message).is_ok() {
                if is_terminal {
                    println!(
                        "{}~{} Response copied to clipboard",
                        TermColor::Cyan.ansi_code(),
                        TermColor::Default.ansi_code()
                    );
                }
            } else {
                print_warning("Failed to copy to clipboard.");
            }
        }

        // Save to output file if requested
        if let Some(ref output_path) = self.output_file {
            // Create parent directories if they don't exist
            if let Some(parent) = output_path.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "Failed to create parent directory '{}' for output file. \
                             Please ensure you have write permissions to this location.",
                        parent.display()
                    )
                })?;
                if is_terminal {
                    eprintln!(
                        "{}~{} Created directory: {}",
                        TermColor::Cyan.ansi_code(),
                        TermColor::Default.ansi_code(),
                        parent.display()
                    );
                }
            }

            std::fs::write(output_path, &final_message).with_context(|| {
                format!("Failed to write output to '{}'", output_path.display())
            })?;

            if is_terminal {
                println!(
                    "{}~{} Response saved to: {}",
                    TermColor::Cyan.ansi_code(),
                    TermColor::Default.ansi_code(),
                    output_path.display()
                );
            }
        }

        // Send desktop notification if requested
        if self.notification {
            send_notification(&session_id, !error_occurred)?;
        }

        // Share session if requested
        if self.share {
            use cortex_share::ShareManager;
            let share_manager = ShareManager::new();
            match share_manager.share(&session_id).await {
                Ok(shared) => {
                    print_success(&format!("Session shared: {}", shared.url));
                }
                Err(e) => {
                    print_warning(&format!("Failed to share session: {}", e));
                }
            }
        }

        // Cleanup
        drop(handle);
        let _ = session_task.await;

        // Exit with appropriate code (#2174)
        if error_occurred {
            std::process::exit(1);
        }
        if response_truncated {
            // Exit with code 2 to indicate truncation (distinct from error code 1)
            std::process::exit(2);
        }

        Ok(())
    }

    /// Run in dry-run mode - show token estimates without executing.
    async fn run_dry_run(&self, message: &str, attachments: &[FileAttachment]) -> Result<()> {
        use cortex_engine::tokenizer::TokenCounter;

        let config = cortex_engine::Config::default();
        let model = self
            .model
            .as_ref()
            .map(|m| resolve_model_alias(m).to_string())
            .unwrap_or_else(|| config.model.clone());

        let mut counter = TokenCounter::for_model(&model);

        // Count user prompt tokens
        let user_prompt_tokens = counter.count(message);

        // Count attachment tokens
        let mut attachment_tokens = 0u32;
        for attachment in attachments {
            let content =
                std::fs::read_to_string(&attachment.path).unwrap_or_else(|_| String::new());
            attachment_tokens += counter.count(&content);
            // Add overhead for file markers
            attachment_tokens += 20; // Approximate overhead for "--- File: ... ---" markers
        }

        // Estimate system prompt tokens (typical system prompt is ~500-2000 tokens)
        // This is an approximation as the actual system prompt varies
        let system_prompt_tokens = 1500u32;

        // Estimate tool definition tokens
        // Each tool definition is approximately 100-200 tokens on average
        // Common tools: Execute, Read, Write, Edit, LS, Grep, Glob, etc.
        let tool_count = 15; // Approximate number of default tools
        let tool_tokens = tool_count * 150; // ~150 tokens per tool definition

        // Calculate totals
        let total_input_tokens =
            user_prompt_tokens + attachment_tokens + system_prompt_tokens + tool_tokens;

        // Output based on format
        if matches!(self.format, OutputFormat::Json | OutputFormat::Jsonl) {
            let output = serde_json::json!({
                "dry_run": true,
                "model": model,
                "token_estimates": {
                    "user_prompt": user_prompt_tokens,
                    "attachments": attachment_tokens,
                    "system_prompt": system_prompt_tokens,
                    "tool_definitions": tool_tokens,
                    "total_input": total_input_tokens,
                },
                "message_preview": if message.len() > 100 {
                    format!("{}...", &message[..100])
                } else {
                    message.to_string()
                },
                "attachment_count": attachments.len(),
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("Dry Run - Token Estimate");
            println!("{}", "=".repeat(50));
            println!();
            println!("Model: {}", model);
            println!();
            println!("Token Breakdown:");
            println!("  User prompt:      {:>8} tokens", user_prompt_tokens);
            if !attachments.is_empty() {
                println!(
                    "  Attachments:      {:>8} tokens ({} files)",
                    attachment_tokens,
                    attachments.len()
                );
            }
            println!(
                "  System prompt:    {:>8} tokens (estimated)",
                system_prompt_tokens
            );
            println!(
                "  Tool definitions: {:>8} tokens (estimated, {} tools)",
                tool_tokens, tool_count
            );
            println!("  {}", "-".repeat(30));
            println!("  Total input:      {:>8} tokens", total_input_tokens);
            println!();
            println!("Note: System prompt and tool definition token counts are estimates.");
            println!("Actual counts may vary based on agent configuration.");
            if !message.is_empty() {
                println!();
                println!("Message preview:");
                let preview = if message.len() > 200 {
                    format!("  {}...", &message[..200])
                } else {
                    format!("  {}", message)
                };
                println!("{}", preview);
            }
        }

        Ok(())
    }
}
