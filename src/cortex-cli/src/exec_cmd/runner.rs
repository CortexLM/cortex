//! Execution runner for exec mode.

use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use cortex_common::resolve_model_alias;
use cortex_engine::Session;
use cortex_protocol::{
    AskForApproval, ConversationId, Event, EventMsg, Op, SandboxPolicy, Submission, UserInput,
};

use super::autonomy::AutonomyLevel;
use super::cli::ExecCli;
use super::helpers::{
    collect_files_by_pattern, ensure_utf8_locale, fetch_url_content, get_git_diff, read_clipboard,
    validate_path_environment,
};
use super::jsonrpc::{JsonRpcRequest, JsonRpcResponse, event_to_jsonrpc};
use super::output::{ExecInputFormat, ExecOutputFormat};

impl ExecCli {
    /// Run the exec command.
    pub async fn run(self) -> Result<()> {
        // Validate mutually exclusive tool flags
        if !self.enabled_tools.is_empty() && !self.disabled_tools.is_empty() {
            bail!(
                "Cannot specify both --enabled-tools and --disabled-tools. Choose one to filter tools."
            );
        }

        // Ensure UTF-8 locale for proper text handling
        let _ = ensure_utf8_locale();

        // Validate PATH environment
        let path_warnings = validate_path_environment();
        for warning in &path_warnings {
            if self.verbose {
                eprintln!("{}", warning);
            }
        }

        // Handle --list-tools
        if self.list_tools {
            return self.list_available_tools().await;
        }

        // Build the prompt
        let prompt = self.build_prompt().await?;

        if prompt.is_empty() {
            bail!("No prompt provided. Use positional argument, --file, or pipe via stdin.");
        }

        // Echo prompt if requested
        if self.echo {
            println!("--- Prompt ---");
            println!("{}", prompt);
            println!("--- End Prompt ---\n");
        }

        // Determine autonomy level
        let autonomy = if self.skip_permissions {
            None // Skip all checks
        } else {
            Some(self.autonomy.unwrap_or_default())
        };

        // Handle different input formats
        match self.input_format {
            ExecInputFormat::StreamJsonrpc => self.run_multiturn(prompt, autonomy).await,
            ExecInputFormat::Text => self.run_single(prompt, autonomy).await,
        }
    }

    /// Build the prompt from various sources.
    pub(crate) async fn build_prompt(&self) -> Result<String> {
        let mut prompt = String::new();
        let mut context_sections: Vec<String> = Vec::new();

        // Read from file if specified
        if let Some(ref file_path) = self.file {
            let content = tokio::fs::read_to_string(file_path)
                .await
                .with_context(|| format!("Failed to read prompt file: {}", file_path.display()))?;
            prompt.push_str(&content);
        }

        // Add command line prompt
        if !self.prompt.is_empty() {
            if !prompt.is_empty() {
                prompt.push('\n');
            }
            prompt.push_str(&self.prompt.join(" "));
        }

        // Read from stdin if not a TTY
        if !io::stdin().is_terminal() {
            let mut stdin_content = String::new();
            io::stdin().lock().read_to_string(&mut stdin_content)?;
            if !stdin_content.is_empty() {
                if !prompt.is_empty() {
                    prompt.push('\n');
                }
                prompt.push_str(&stdin_content);
            }
        }

        // Read from clipboard if requested
        if self.clipboard
            && let Ok(clipboard_content) = read_clipboard()
            && !clipboard_content.is_empty()
        {
            context_sections.push(format!(
                "--- Clipboard Content ---\n{}\n--- End Clipboard ---",
                clipboard_content
            ));
        }

        // Fetch URLs and include content
        for url in &self.urls {
            match fetch_url_content(url).await {
                Ok(content) => {
                    context_sections.push(format!(
                        "--- Content from {} ---\n{}\n--- End URL Content ---",
                        url, content
                    ));
                }
                Err(e) => {
                    eprintln!("Warning: Failed to fetch URL {}: {}", url, e);
                }
            }
        }

        // Include git diff if requested
        if self.git_diff {
            let cwd = self
                .cwd
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            if let Ok(diff) = get_git_diff(&cwd)
                && !diff.is_empty()
            {
                context_sections.push(format!("--- Git Diff ---\n{}\n--- End Git Diff ---", diff));
            }
        }

        // Handle include/exclude patterns for file context
        if !self.include_patterns.is_empty() || !self.exclude_patterns.is_empty() {
            let cwd = self
                .cwd
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            if let Ok(files_content) =
                collect_files_by_pattern(&cwd, &self.include_patterns, &self.exclude_patterns)
                && !files_content.is_empty()
            {
                context_sections.push(files_content);
            }
        }

        // Build final prompt with context sections
        let mut final_prompt = String::new();

        // Add context sections first
        if !context_sections.is_empty() {
            final_prompt.push_str(&context_sections.join("\n\n"));
            final_prompt.push_str("\n\n");
        }

        // Add main prompt
        final_prompt.push_str(prompt.trim());

        // Add suffix if provided
        if let Some(ref suffix) = self.suffix {
            final_prompt.push_str("\n\n[Suffix for completion insertion: ");
            final_prompt.push_str(suffix);
            final_prompt.push(']');
        }

        Ok(final_prompt.trim().to_string())
    }

    /// List available tools for the selected model.
    pub(crate) async fn list_available_tools(&self) -> Result<()> {
        use cortex_engine::tools::ToolRouter;

        let router = ToolRouter::new();
        let tools = router.get_tool_definitions();

        // Filter by enabled/disabled
        let filtered_tools: Vec<_> = tools
            .iter()
            .filter(|t| {
                if !self.enabled_tools.is_empty() {
                    return self
                        .enabled_tools
                        .iter()
                        .any(|e| e.eq_ignore_ascii_case(&t.name));
                }
                if !self.disabled_tools.is_empty() {
                    return !self
                        .disabled_tools
                        .iter()
                        .any(|d| d.eq_ignore_ascii_case(&t.name));
                }
                true
            })
            .collect();

        match self.output_format {
            ExecOutputFormat::Json | ExecOutputFormat::StreamJson => {
                let tools_json: Vec<_> = filtered_tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&tools_json)?);
            }
            _ => {
                println!("Available tools ({}):", filtered_tools.len());
                println!("{:-<60}", "");
                for tool in filtered_tools {
                    println!(
                        "  {:<20} - {}",
                        tool.name,
                        tool.description.chars().take(50).collect::<String>()
                    );
                }
            }
        }

        Ok(())
    }

    /// Run single-shot execution.
    pub(crate) async fn run_single(
        &self,
        prompt: String,
        autonomy: Option<AutonomyLevel>,
    ) -> Result<()> {
        let start_time = Instant::now();
        let _is_json = matches!(self.output_format, ExecOutputFormat::Json);
        let is_stream = matches!(
            self.output_format,
            ExecOutputFormat::StreamJson | ExecOutputFormat::Debug
        );

        // Build configuration
        let mut config = cortex_engine::Config::default();

        // Apply working directory
        let cwd = self
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        config.cwd = cwd.clone();

        // Apply model if specified
        if let Some(ref model) = self.model {
            config.model = resolve_model_alias(model).to_string();
        }

        // Apply autonomy settings
        if let Some(level) = autonomy {
            config.approval_policy = level.to_approval_policy();
            config.sandbox_policy = level.to_sandbox_policy(&cwd);
        } else if self.skip_permissions {
            config.approval_policy = AskForApproval::Never;
            config.sandbox_policy = SandboxPolicy::DangerFullAccess;
        }

        // Initialize custom command registry
        let project_root = Some(cwd.clone());
        let _custom_registry = cortex_engine::init_custom_command_registry(
            &config.cortex_home,
            project_root.as_deref(),
        );
        if let Err(e) = _custom_registry.scan().await {
            tracing::warn!("Failed to scan custom commands: {}", e);
        }

        // Create session
        let (mut session, handle) = Session::new(config.clone())?;

        // Spawn session task
        let session_task = tokio::spawn(async move { session.run().await });

        // Emit init event for stream formats
        if is_stream {
            let init_event = serde_json::json!({
                "type": "system",
                "subtype": "init",
                "cwd": cwd.display().to_string(),
                "session_id": handle.conversation_id.to_string(),
                "model": config.model,
                "timestamp": chrono::Utc::now().timestamp_millis(),
            });
            println!("{}", serde_json::to_string(&init_event)?);
        }

        // Build user input
        let input_items = self.build_input_items(&prompt).await?;

        // Send the submission
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::UserInput { items: input_items },
        };
        handle.submission_tx.send(submission).await?;

        // Process events
        let result = self.process_events(&handle, start_time, autonomy).await;

        // Cleanup
        drop(handle);
        let _ = session_task.await;

        result
    }

    /// Build input items from prompt and images.
    pub(crate) async fn build_input_items(&self, prompt: &str) -> Result<Vec<UserInput>> {
        let mut items = Vec::new();

        // Add images first
        for image_path in &self.images {
            let resolved_path = if image_path.is_absolute() {
                image_path.clone()
            } else {
                std::env::current_dir()?.join(image_path)
            };

            if !resolved_path.exists() {
                bail!("File not found: {}", image_path.display());
            }

            let image_bytes = tokio::fs::read(&resolved_path)
                .await
                .with_context(|| format!("Failed to read image: {}", resolved_path.display()))?;

            use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
            let base64_data = BASE64.encode(&image_bytes);

            let media_type = resolved_path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| match ext.to_lowercase().as_str() {
                    "png" => "image/png",
                    "jpg" | "jpeg" => "image/jpeg",
                    "gif" => "image/gif",
                    "webp" => "image/webp",
                    _ => "application/octet-stream",
                })
                .unwrap_or("application/octet-stream")
                .to_string();

            items.push(UserInput::Image {
                data: base64_data,
                media_type,
            });
        }

        // Add text prompt
        items.push(UserInput::Text {
            text: prompt.to_string(),
        });

        Ok(items)
    }

    /// Process events from the session.
    pub(crate) async fn process_events(
        &self,
        handle: &cortex_engine::SessionHandle,
        start_time: Instant,
        autonomy: Option<AutonomyLevel>,
    ) -> Result<()> {
        let _is_json = matches!(self.output_format, ExecOutputFormat::Json);
        let is_stream = matches!(
            self.output_format,
            ExecOutputFormat::StreamJson | ExecOutputFormat::Debug
        );
        let is_text = matches!(self.output_format, ExecOutputFormat::Text);

        let timeout = if self.timeout > 0 {
            Some(Duration::from_secs(self.timeout))
        } else {
            None
        };

        let mut final_message = String::new();
        let mut num_turns = 0u64;
        let mut error_occurred = false;
        let mut error_message = None;
        let mut tool_calls_count = 0u64;

        loop {
            // Check timeout
            if let Some(t) = timeout
                && start_time.elapsed() > t
            {
                error_occurred = true;
                error_message = Some(format!("Timeout after {} seconds", self.timeout));
                break;
            }

            // Wait for event with timeout
            let event = tokio::select! {
                result = handle.event_rx.recv() => {
                    match result {
                        Ok(e) => e,
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => continue,
            };

            // Emit stream events
            if is_stream {
                self.emit_stream_event(&event, &handle.conversation_id)?;
            }

            match &event.msg {
                EventMsg::AgentMessage(msg) => {
                    final_message = msg.message.clone();
                }
                EventMsg::AgentMessageDelta(delta) => {
                    if is_text {
                        print!("{}", delta.delta);
                        io::stdout().flush()?;
                    }
                    final_message.push_str(&delta.delta);
                }
                EventMsg::TaskStarted(_) => {
                    num_turns += 1;
                    if num_turns > self.max_turns as u64 {
                        error_occurred = true;
                        error_message = Some(format!("Max turns ({}) exceeded", self.max_turns));
                        break;
                    }
                }
                EventMsg::TaskComplete(_) => {
                    break;
                }
                EventMsg::ExecCommandBegin(cmd_begin) => {
                    tool_calls_count += 1;
                    if is_text && self.verbose {
                        let cmd_str = cmd_begin.command.join(" ");
                        eprintln!("\x1b[1;34m[EXEC]\x1b[0m {}", cmd_str);
                    }
                }
                EventMsg::McpToolCallBegin(mcp_begin) => {
                    tool_calls_count += 1;
                    if is_text && self.verbose {
                        eprintln!(
                            "\x1b[1;36m[TOOL]\x1b[0m {} ({})",
                            mcp_begin.invocation.tool, mcp_begin.invocation.server
                        );
                    }
                }
                EventMsg::ExecApprovalRequest(approval) => {
                    // Check autonomy level using allows_risk for proper validation
                    if let Some(level) = autonomy {
                        let command_str = approval.command.join(" ");
                        let risk_level = approval
                            .sandbox_assessment
                            .as_ref()
                            .map(|a| match a.risk_level {
                                cortex_protocol::SandboxRiskLevel::Low => "low",
                                cortex_protocol::SandboxRiskLevel::Medium => "medium",
                                cortex_protocol::SandboxRiskLevel::High => "high",
                            })
                            .unwrap_or("low");

                        if !level.allows_risk(risk_level, &command_str) {
                            // Command not allowed at this autonomy level
                            error_occurred = true;
                            error_message = Some(format!(
                                "Permission denied: Command '{}' (risk: {}) not allowed in {} mode. \
                                Use --auto with higher autonomy level to enable.",
                                command_str, risk_level, level
                            ));
                            break;
                        }
                    }

                    // Auto-approve based on autonomy level (already set via approval_policy)
                    if is_text && self.verbose {
                        eprintln!(
                            "\x1b[1;33m[APPROVAL]\x1b[0m Auto-approving: {}",
                            approval.command.join(" ")
                        );
                    }
                }
                EventMsg::Error(e) => {
                    error_occurred = true;
                    error_message = Some(e.message.clone());
                    if is_text {
                        eprintln!("\x1b[1;31m[ERROR]\x1b[0m {}", e.message);
                    }
                    break;
                }
                EventMsg::Warning(w) => {
                    if is_text && self.verbose {
                        eprintln!("\x1b[1;33m[WARN]\x1b[0m {}", w.message);
                    }
                }
                _ => {}
            }
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        // Output final result
        match self.output_format {
            ExecOutputFormat::Text => {
                if !final_message.is_empty() && !final_message.ends_with('\n') {
                    println!();
                }
            }
            ExecOutputFormat::Json => {
                let result = if error_occurred {
                    serde_json::json!({
                        "type": "result",
                        "subtype": "error",
                        "is_error": true,
                        "error": error_message,
                        "duration_ms": duration_ms,
                        "num_turns": num_turns,
                        "session_id": handle.conversation_id.to_string(),
                    })
                } else {
                    serde_json::json!({
                        "type": "result",
                        "subtype": "success",
                        "is_error": false,
                        "result": final_message,
                        "duration_ms": duration_ms,
                        "num_turns": num_turns,
                        "session_id": handle.conversation_id.to_string(),
                    })
                };
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
            ExecOutputFormat::StreamJson | ExecOutputFormat::Debug => {
                // Emit completion event
                let completion = serde_json::json!({
                    "type": "completion",
                    "finalText": final_message,
                    "numTurns": num_turns,
                    "durationMs": duration_ms,
                    "session_id": handle.conversation_id.to_string(),
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                    "toolCalls": tool_calls_count,
                });
                println!("{}", serde_json::to_string(&completion)?);
            }
            ExecOutputFormat::StreamJsonrpc => {
                // Already handled in multi-turn mode
            }
        }

        if error_occurred {
            std::process::exit(1);
        }

        Ok(())
    }

    /// Emit a stream event in JSONL format.
    pub(crate) fn emit_stream_event(
        &self,
        event: &Event,
        session_id: &ConversationId,
    ) -> Result<()> {
        let event_json = match &event.msg {
            EventMsg::AgentMessage(msg) => {
                serde_json::json!({
                    "type": "message",
                    "role": "assistant",
                    "id": msg.id,
                    "text": msg.message,
                    "session_id": session_id.to_string(),
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                })
            }
            EventMsg::AgentMessageDelta(delta) => {
                serde_json::json!({
                    "type": "delta",
                    "content": delta.delta,
                    "session_id": session_id.to_string(),
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                })
            }
            EventMsg::ExecCommandBegin(cmd) => {
                serde_json::json!({
                    "type": "tool_call",
                    "id": cmd.call_id,
                    "toolName": "Execute",
                    "parameters": {
                        "command": cmd.command.join(" "),
                        "cwd": cmd.cwd.display().to_string(),
                    },
                    "session_id": session_id.to_string(),
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                })
            }
            EventMsg::ExecCommandEnd(cmd) => {
                serde_json::json!({
                    "type": "tool_result",
                    "id": cmd.call_id,
                    "toolName": "Execute",
                    "isError": cmd.exit_code != 0,
                    "value": cmd.formatted_output,
                    "exitCode": cmd.exit_code,
                    "durationMs": cmd.duration_ms,
                    "session_id": session_id.to_string(),
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                })
            }
            EventMsg::McpToolCallBegin(mcp) => {
                serde_json::json!({
                    "type": "tool_call",
                    "id": mcp.call_id,
                    "toolName": mcp.invocation.tool,
                    "server": mcp.invocation.server,
                    "parameters": mcp.invocation.arguments,
                    "session_id": session_id.to_string(),
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                })
            }
            EventMsg::McpToolCallEnd(mcp) => {
                let (is_error, value) = match &mcp.result {
                    Ok(result) => (
                        result.is_error.unwrap_or(false),
                        serde_json::to_value(&result.content).unwrap_or_default(),
                    ),
                    Err(e) => (true, serde_json::json!(e)),
                };
                serde_json::json!({
                    "type": "tool_result",
                    "id": mcp.call_id,
                    "toolName": mcp.invocation.tool,
                    "isError": is_error,
                    "value": value,
                    "durationMs": mcp.duration_ms,
                    "session_id": session_id.to_string(),
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                })
            }
            EventMsg::Error(e) => {
                serde_json::json!({
                    "type": "error",
                    "message": e.message,
                    "session_id": session_id.to_string(),
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                })
            }
            EventMsg::AgentReasoning(r) => {
                serde_json::json!({
                    "type": "reasoning",
                    "text": r.text,
                    "session_id": session_id.to_string(),
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                })
            }
            _ => return Ok(()), // Skip other events
        };

        println!("{}", serde_json::to_string(&event_json)?);
        io::stdout().flush()?;
        Ok(())
    }

    /// Run multi-turn execution via stream-jsonrpc.
    pub(crate) async fn run_multiturn(
        &self,
        initial_prompt: String,
        autonomy: Option<AutonomyLevel>,
    ) -> Result<()> {
        // Build configuration
        let mut config = cortex_engine::Config::default();

        // Apply working directory
        let cwd = self
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        config.cwd = cwd.clone();

        // Apply model if specified
        if let Some(ref model) = self.model {
            config.model = resolve_model_alias(model).to_string();
        }

        // Apply autonomy settings
        if let Some(level) = autonomy {
            config.approval_policy = level.to_approval_policy();
            config.sandbox_policy = level.to_sandbox_policy(&cwd);
        } else if self.skip_permissions {
            config.approval_policy = AskForApproval::Never;
            config.sandbox_policy = SandboxPolicy::DangerFullAccess;
        }

        // Initialize custom command registry
        let project_root = Some(cwd.clone());
        let _custom_registry = cortex_engine::init_custom_command_registry(
            &config.cortex_home,
            project_root.as_deref(),
        );
        if let Err(e) = _custom_registry.scan().await {
            tracing::warn!("Failed to scan custom commands: {}", e);
        }

        // Create session
        let (mut session, handle) = Session::new(config.clone())?;

        // Spawn session task
        let session_task = tokio::spawn(async move { session.run().await });

        // Emit initialization response
        let init_response = JsonRpcResponse::result(
            serde_json::json!(null),
            serde_json::json!({
                "type": "initialized",
                "session_id": handle.conversation_id.to_string(),
                "model": config.model,
                "cwd": cwd.display().to_string(),
            }),
        );
        println!("{}", serde_json::to_string(&init_response)?);

        // Send initial prompt if provided
        if !initial_prompt.is_empty() {
            let input_items = self.build_input_items(&initial_prompt).await?;
            let submission = Submission {
                id: uuid::Uuid::new_v4().to_string(),
                op: Op::UserInput { items: input_items },
            };
            handle.submission_tx.send(submission).await?;
        }

        // Spawn event handler
        let event_handle = handle.clone();
        let _output_format = self.output_format;
        let event_task = tokio::spawn(async move {
            while let Ok(event) = event_handle.event_rx.recv().await {
                let notification = event_to_jsonrpc(&event, &event_handle.conversation_id);
                if let Ok(json) = serde_json::to_string(&notification) {
                    println!("{}", json);
                    let _ = io::stdout().flush();
                }

                // Check for completion
                if matches!(event.msg, EventMsg::TaskComplete(_)) {
                    break;
                }
            }
        });

        // Read JSONL from stdin for multi-turn
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            if line.trim().is_empty() {
                continue;
            }

            // Parse JSON-RPC request
            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let error_response = JsonRpcResponse::error(
                        serde_json::json!(null),
                        -32700,
                        format!("Parse error: {}", e),
                    );
                    println!("{}", serde_json::to_string(&error_response)?);
                    continue;
                }
            };

            // Handle the request
            match request.method.as_str() {
                "message" | "user_input" => {
                    let text = request
                        .params
                        .get("text")
                        .or_else(|| request.params.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if !text.is_empty() {
                        let submission = Submission {
                            id: request
                                .id
                                .as_ref()
                                .and_then(|id| id.as_str())
                                .map(String::from)
                                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                            op: Op::UserInput {
                                items: vec![UserInput::Text {
                                    text: text.to_string(),
                                }],
                            },
                        };
                        handle.submission_tx.send(submission).await?;
                    }
                }
                "interrupt" | "cancel" => {
                    let submission = Submission {
                        id: uuid::Uuid::new_v4().to_string(),
                        op: Op::Interrupt,
                    };
                    handle.submission_tx.send(submission).await?;
                }
                "shutdown" | "exit" => {
                    let submission = Submission {
                        id: uuid::Uuid::new_v4().to_string(),
                        op: Op::Shutdown,
                    };
                    handle.submission_tx.send(submission).await?;
                    break;
                }
                _ => {
                    let error_response = JsonRpcResponse::error(
                        request.id.clone().unwrap_or(serde_json::json!(null)),
                        -32601,
                        format!("Method not found: {}", request.method),
                    );
                    println!("{}", serde_json::to_string(&error_response)?);
                }
            }
        }

        // Cleanup
        drop(handle);
        let _ = event_task.await;
        let _ = session_task.await;

        Ok(())
    }
}
