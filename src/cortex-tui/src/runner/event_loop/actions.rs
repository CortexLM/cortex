//! Action handling: keyboard actions mapped from key events.

use anyhow::Result;

use crate::actions::KeyAction;
use crate::app::{AppView, FocusTarget};

use super::core::EventLoop;

impl EventLoop {
    /// Handles a mapped action from user input.
    ///
    /// This method translates high-level actions into state changes and
    /// backend communication.
    pub(super) async fn handle_action(&mut self, action: KeyAction) -> Result<()> {
        match action {
            KeyAction::Quit => {
                self.app_state.set_quit();
            }

            KeyAction::Submit => {
                let text = self.app_state.input.submit();
                if !text.is_empty() {
                    // Check for slash commands first
                    if let Some(stripped) = text.strip_prefix('/') {
                        // Check if command exists before executing
                        let cmd_name = stripped.split_whitespace().next().unwrap_or("");
                        if self.command_executor.registry().exists(cmd_name) {
                            // Valid command - record and execute
                            self.app_state.add_to_history(cmd_name);
                            let result = self.command_executor.execute_str(&text);
                            self.handle_command_result(result).await?;
                        } else {
                            // Unknown command - send as regular message
                            self.send_text_message(text).await?;
                        }
                    } else {
                        // Regular text message
                        self.send_text_message(text).await?;
                    }
                }
            }

            KeyAction::Cancel => {
                // Priority order: queued messages > pending approval > streaming
                if self.app_state.has_queued_messages() {
                    // Cancel queued messages first
                    let count = self.app_state.queued_count();
                    self.app_state.clear_message_queue();
                    self.add_system_message(&format!("Cancelled {} queued message(s)", count));
                } else if self.app_state.pending_approval.is_some() {
                    self.app_state.reject();
                } else if self.app_state.streaming.is_streaming {
                    // Cancel streaming - try new system first, then legacy
                    if self.streaming_task.is_some() {
                        self.cancel_streaming();
                    } else if let Some(ref bridge) = self.session_bridge {
                        bridge.interrupt().await?;
                    }
                }
            }

            KeyAction::Approve => {
                self.handle_approve().await?;
            }

            KeyAction::Reject => {
                self.handle_reject().await?;
            }

            KeyAction::ApproveSession => {
                self.handle_approve_session().await?;
            }

            KeyAction::ApproveAlways => {
                self.handle_approve_always().await?;
            }

            // Focus actions
            KeyAction::FocusNext => self.app_state.focus_next(),
            KeyAction::FocusPrev => self.app_state.focus_prev(),
            KeyAction::FocusInput => self.app_state.set_focus(FocusTarget::Input),
            KeyAction::FocusChat => self.app_state.set_focus(FocusTarget::Chat),
            KeyAction::FocusSidebar => self.app_state.set_focus(FocusTarget::Sidebar),

            // Scroll actions
            KeyAction::ScrollUp => self.app_state.scroll_chat(-1),
            KeyAction::ScrollDown => self.app_state.scroll_chat(1),
            KeyAction::ScrollPageUp => {
                // Use terminal height - 1 for standard page scroll (provides context overlap)
                let page_size = (self.app_state.terminal_size.1 as i32)
                    .saturating_sub(1)
                    .max(1);
                self.app_state.scroll_chat(-page_size);
            }
            KeyAction::ScrollPageDown => {
                // Use terminal height - 1 for standard page scroll (provides context overlap)
                let page_size = (self.app_state.terminal_size.1 as i32)
                    .saturating_sub(1)
                    .max(1);
                self.app_state.scroll_chat(page_size);
            }
            KeyAction::ScrollToTop => self.app_state.scroll_chat_to_top(),
            KeyAction::ScrollToBottom => self.app_state.scroll_chat_to_bottom(),

            // View actions
            KeyAction::ToggleSidebar => self.app_state.toggle_sidebar(),
            KeyAction::Help => {
                // Open the new help modal
                use crate::modal::HelpModal;
                self.modal_stack.push(Box::new(HelpModal::new()));
            }

            // Model/Provider switching - open modals
            KeyAction::SwitchModel => {
                self.open_models_modal();
            }

            // Modal shortcuts (new unified modal system)
            KeyAction::OpenCommandPalette => {
                use crate::modal::CommandsModal;
                self.modal_stack.push(Box::new(CommandsModal::new()));
            }
            KeyAction::OpenSessions => {
                self.open_sessions_modal();
            }
            KeyAction::OpenMcp => {
                // Open MCP manager using interactive mode (like settings panel)
                use crate::interactive::builders::build_mcp_selector;
                let servers = self.app_state.mcp_servers.clone();
                let interactive = build_mcp_selector(&servers);
                self.app_state.enter_interactive_mode(interactive);
            }

            // Transcript
            KeyAction::ViewTranscript => {
                self.handle_transcript();
            }

            // Permission and tool actions
            KeyAction::CyclePermissionMode => {
                self.app_state.cycle_permission_mode();
                self.sync_permission_mode();
            }
            KeyAction::ToggleToolDetails => {
                // Toggle all tool calls collapsed state
                for call in &mut self.app_state.tool_calls {
                    call.toggle_collapsed();
                }
            }

            // Input history navigation
            KeyAction::HistoryPrev => {
                self.app_state.input.history_prev();
            }
            KeyAction::HistoryNext => {
                self.app_state.input.history_next();
            }

            // No action
            KeyAction::None => {}

            // Handle other actions
            _ => {
                tracing::debug!("Unhandled action: {:?}", action);
            }
        }

        Ok(())
    }

    /// Handle approve action
    async fn handle_approve(&mut self) -> Result<()> {
        use crate::views::tool_call::ToolStatus;

        if let Some(approval) = self.app_state.approve() {
            // New provider system: execute the tool in background
            if self.provider_manager.is_some() {
                let tool_call_id = approval.tool_call_id.clone();
                let tool_name = approval.tool_name.clone();

                // Get arguments from the stored JSON
                let arguments = approval
                    .tool_args_json
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({}));

                // Update tool status
                self.app_state
                    .update_tool_status(&tool_call_id, ToolStatus::Running);

                // Route Task tool to subagent spawner
                if tool_name == "Task" || tool_name == "task" {
                    self.spawn_subagent(tool_call_id, arguments);
                } else {
                    // Spawn tool execution in background (non-blocking!)
                    // The result will come back via tool_rx channel
                    self.spawn_tool_execution(tool_call_id, tool_name, arguments);
                }
            } else if let Some(ref bridge) = self.session_bridge {
                // Legacy: send approval to session bridge
                let call_id = serde_json::from_str::<serde_json::Value>(&approval.tool_args)
                    .ok()
                    .and_then(|v| {
                        v.get("call_id")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                bridge
                    .send_approval(call_id, cortex_protocol::ReviewDecision::Approved)
                    .await?;
            }
        }
        Ok(())
    }

    /// Handle reject action
    async fn handle_reject(&mut self) -> Result<()> {
        if let Some(approval) = self.app_state.reject() {
            // Update tool status to failed
            self.app_state.update_tool_result(
                &approval.tool_call_id,
                "Tool execution rejected by user".to_string(),
                false,
                "Rejected".to_string(),
            );

            if let Some(ref bridge) = self.session_bridge {
                // Legacy: send rejection to session bridge
                let call_id = serde_json::from_str::<serde_json::Value>(&approval.tool_args)
                    .ok()
                    .and_then(|v| {
                        v.get("call_id")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                bridge
                    .send_approval(call_id, cortex_protocol::ReviewDecision::Denied)
                    .await?;
            }
        }
        Ok(())
    }

    /// Handle approve for session action
    async fn handle_approve_session(&mut self) -> Result<()> {
        use crate::views::tool_call::ToolStatus;

        // Approve and allow this tool for the rest of the session
        if let Some(approval) = self.app_state.approve() {
            let tool_name = approval.tool_name.clone();

            // Add to session-allowed tools
            self.permission_manager.allow_for_session(&tool_name);

            if self.provider_manager.is_some() {
                let tool_call_id = approval.tool_call_id.clone();
                let arguments = approval
                    .tool_args_json
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({}));

                self.app_state
                    .update_tool_status(&tool_call_id, ToolStatus::Running);

                self.add_system_message(&format!(
                    "'{}' will be auto-approved for this session",
                    tool_name
                ));

                // Route Task tool to subagent spawner
                if tool_name == "Task" || tool_name == "task" {
                    self.spawn_subagent(tool_call_id, arguments);
                } else {
                    // Spawn tool execution in background (non-blocking!)
                    self.spawn_tool_execution(tool_call_id, tool_name, arguments);
                }
            }
        }
        Ok(())
    }

    /// Handle approve always action
    async fn handle_approve_always(&mut self) -> Result<()> {
        use crate::views::tool_call::ToolStatus;

        // Approve and always allow this tool
        if let Some(approval) = self.app_state.approve() {
            let tool_name = approval.tool_name.clone();

            // Add to always-allowed tools
            self.permission_manager.allow_always(&tool_name);

            if self.provider_manager.is_some() {
                let tool_call_id = approval.tool_call_id.clone();
                let arguments = approval
                    .tool_args_json
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({}));

                self.app_state
                    .update_tool_status(&tool_call_id, ToolStatus::Running);

                self.add_system_message(&format!("'{}' will always be auto-approved", tool_name));

                // Route Task tool to subagent spawner
                if tool_name == "Task" || tool_name == "task" {
                    self.spawn_subagent(tool_call_id, arguments);
                } else {
                    // Spawn tool execution in background (non-blocking!)
                    self.spawn_tool_execution(tool_call_id, tool_name, arguments);
                }
            }
        }
        Ok(())
    }

    /// Sends a text message to the AI provider.
    ///
    /// Handles busy state (queuing) and routes to the appropriate provider system.
    pub(super) async fn send_text_message(&mut self, text: String) -> Result<()> {
        if self.app_state.is_busy() {
            // System is busy - queue the message
            self.app_state.queue_message(text);
            let count = self.app_state.queued_count();
            tracing::debug!("{} message(s) queued", count);
        } else {
            // System is free - send immediately
            self.app_state.scroll_chat_to_bottom();

            if self.provider_manager.is_some() {
                self.handle_submit_with_provider(text).await?;
            } else if let Some(ref bridge) = self.session_bridge {
                let message = cortex_core::widgets::Message::user(&text);
                self.app_state.add_message(message);
                bridge.send_message(text).await?;
                self.app_state.set_view(AppView::Session);
            }
        }
        Ok(())
    }
}
