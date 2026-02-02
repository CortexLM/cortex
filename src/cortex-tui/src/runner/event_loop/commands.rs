//! Command handling: slash commands and result processing.
//!
//! This module handles the execution of slash commands and the processing
//! of their results. Due to the large number of command handlers, they are
//! delegated to the main event_loop module in the original monolithic file.
//! This file provides the command result handling infrastructure.

use anyhow::Result;

use crate::app::AppView;
use crate::commands::{CommandResult, FormRegistry, ModalType, ViewType};
use crate::session::{CortexSession, ExportFormat, default_export_filename, export_session};

use super::core::EventLoop;

impl EventLoop {
    /// Handles a command result from the CommandExecutor.
    pub(super) async fn handle_command_result(&mut self, result: CommandResult) -> Result<()> {
        match result {
            CommandResult::Success => {}

            CommandResult::Message(msg) => {
                self.add_system_message(&msg);
            }

            CommandResult::Error(err) => {
                self.add_system_message(&format!("Error: {}", err));
            }

            CommandResult::Quit => {
                self.app_state.set_quit();
            }

            CommandResult::Clear => {
                self.app_state.clear_messages();
            }

            CommandResult::NewSession => {
                self.app_state.new_session();
            }

            CommandResult::ResumeSession(id) => {
                if let Ok(session) = CortexSession::load(&id) {
                    self.cortex_session = Some(session);
                    self.app_state.set_view(AppView::Session);
                    self.add_system_message(&format!("Resumed session: {}", id));
                } else {
                    self.add_system_message(&format!("Failed to load session: {}", id));
                }
            }

            CommandResult::Toggle(feature) => {
                self.handle_toggle(&feature);
            }

            CommandResult::SetValue(key, value) => {
                self.handle_set_value(&key, &value);
            }

            CommandResult::OpenModal(modal_type) => {
                self.handle_open_modal(modal_type).await;
            }

            CommandResult::SwitchView(view_type) => match view_type {
                ViewType::Session => self.app_state.set_view(AppView::Session),
                ViewType::Settings => self.app_state.set_view(AppView::Settings),
                ViewType::Help => self.app_state.set_view(AppView::Help),
            },

            CommandResult::Async(cmd_string) => {
                self.handle_async_command(&cmd_string).await?;
            }

            CommandResult::NotFound(cmd) => {
                self.add_system_message(&format!("Unknown command: {}", cmd));
            }

            CommandResult::NeedsArgs(msg) => {
                self.add_system_message(&msg);
            }
        }

        Ok(())
    }

    /// Handle toggle commands
    fn handle_toggle(&mut self, feature: &str) {
        match feature {
            "sidebar" => {
                self.app_state.toggle_sidebar();
                let state = if self.app_state.sidebar_visible {
                    "shown"
                } else {
                    "hidden"
                };
                self.app_state.toasts.info(format!("Sidebar {}", state));
            }
            "compact" => {
                self.app_state.toggle_compact();
                let state = if self.app_state.compact_mode {
                    "on"
                } else {
                    "off"
                };
                self.app_state
                    .toasts
                    .info(format!("Compact mode: {}", state));
            }
            "favorite" => {
                if self.cortex_session.is_some() {
                    let key = "session_favorite".to_string();
                    let is_fav = self
                        .app_state
                        .settings
                        .get(&key)
                        .map(|v| v == "true")
                        .unwrap_or(false);
                    self.app_state.settings.insert(key, (!is_fav).to_string());
                    if !is_fav {
                        self.app_state.toasts.success("Marked as favorite");
                    } else {
                        self.app_state.toasts.info("Favorite removed");
                    }
                } else {
                    self.app_state.toasts.error("No active session");
                }
            }
            "debug" => {
                self.app_state.toggle_debug();
                let state = if self.app_state.debug_mode {
                    "on"
                } else {
                    "off"
                };
                self.app_state.toasts.info(format!("Debug mode: {}", state));
            }
            "sandbox" => {
                self.app_state.toggle_sandbox();
                let state = if self.app_state.sandbox_mode {
                    "on"
                } else {
                    "off"
                };
                self.app_state
                    .toasts
                    .info(format!("Sandbox mode: {}", state));
            }
            "auto" => {
                let is_yolo = matches!(
                    self.app_state.permission_mode,
                    crate::permissions::PermissionMode::Yolo
                );
                if is_yolo {
                    self.app_state.permission_mode = crate::permissions::PermissionMode::High;
                    self.app_state.toasts.info("Auto-approve: OFF");
                } else {
                    self.app_state.permission_mode = crate::permissions::PermissionMode::Yolo;
                    self.app_state.toasts.success("Auto-approve: ON");
                }
            }
            _ => {
                self.app_state
                    .toasts
                    .error(format!("Unknown toggle: {}", feature));
            }
        }
    }

    /// Handle open modal command
    async fn handle_open_modal(&mut self, modal_type: ModalType) {
        match modal_type {
            ModalType::Help(topic) => {
                use crate::modal::HelpModal;
                self.modal_stack
                    .push(Box::new(HelpModal::with_topic(topic)));
            }
            ModalType::Settings => {
                use crate::interactive::builders::{SettingsSnapshot, build_settings_selector};
                let snapshot = SettingsSnapshot {
                    compact_mode: self.app_state.compact_mode,
                    timestamps: self.app_state.timestamps_enabled,
                    line_numbers: self.app_state.line_numbers_enabled,
                    word_wrap: self.app_state.word_wrap_enabled,
                    syntax_highlight: self.app_state.syntax_highlight_enabled,
                    auto_approve: matches!(
                        self.app_state.permission_mode,
                        crate::permissions::PermissionMode::Yolo
                    ),
                    sandbox_mode: self.app_state.sandbox_mode,
                    streaming_enabled: self.app_state.streaming_enabled,
                    auto_scroll: self.app_state.auto_scroll_enabled,
                    sound: self.app_state.sound_enabled,
                    thinking_enabled: self.app_state.thinking_budget.is_some(),
                    debug_mode: self.app_state.debug_mode,
                    context_aware: self.app_state.context_aware_enabled,
                    co_author: self.app_state.co_author_enabled,
                    auto_commit: self.app_state.auto_commit_enabled,
                    sign_commits: self.app_state.sign_commits_enabled,
                    cloud_sync: self.app_state.cloud_sync_enabled,
                    auto_save: self.app_state.auto_save_enabled,
                    session_history: self.app_state.session_history_enabled,
                    telemetry: self.app_state.telemetry_enabled,
                    analytics: self.app_state.analytics_enabled,
                };
                let terminal_height = self.app_state.terminal_size.1;
                let interactive = build_settings_selector(snapshot, Some(terminal_height));
                self.app_state.enter_interactive_mode(interactive);
            }
            ModalType::ModelPicker => {
                let (models, current_model) = if let Some(ref pm) = self.provider_manager {
                    if let Ok(manager) = pm.try_read() {
                        let models = manager.available_models();
                        let current = manager.current_model().to_string();

                        if models.is_empty() {
                            tracing::warn!(
                                "No models available. Auth token present: {}, cached_models: {}",
                                manager.is_authenticated(),
                                manager.available_models().len()
                            );
                        }

                        (models, Some(current))
                    } else {
                        (Vec::new(), None)
                    }
                } else {
                    (Vec::new(), None)
                };

                if models.is_empty() {
                    self.add_system_message(
                        "No models available. Run /login to authenticate or check your connection.",
                    );
                }

                let interactive = crate::interactive::builders::build_model_selector(
                    models,
                    current_model.as_deref(),
                );
                self.app_state.enter_interactive_mode(interactive);
            }
            ModalType::CommandPalette => {
                use crate::modal::CommandsModal;
                self.modal_stack.push(Box::new(CommandsModal::new()));
            }
            ModalType::Sessions => {
                self.open_sessions_modal();
            }
            ModalType::McpManager => {
                use crate::interactive::builders::build_mcp_selector;
                let servers = self.app_state.mcp_servers.clone();
                let interactive = build_mcp_selector(&servers);
                self.app_state.enter_interactive_mode(interactive);
            }
            ModalType::Form(cmd_name) => match cmd_name.as_str() {
                "temperature" => {
                    let interactive = crate::interactive::builders::build_temperature_selector(
                        self.app_state.temperature,
                    );
                    self.app_state.enter_interactive_mode(interactive);
                }
                "scroll" => {
                    let interactive = crate::interactive::builders::build_scroll_selector();
                    self.app_state.enter_interactive_mode(interactive);
                }
                _ => {
                    let registry = FormRegistry::new();
                    if let Some(form_state) = registry.get_form(&cmd_name) {
                        self.app_state.active_modal =
                            Some(crate::app::ActiveModal::Form(form_state));
                    } else {
                        let usage = match cmd_name.as_str() {
                            "rename" => "/rename <new name>",
                            "remove" => "/remove <file>...",
                            "search" => "/search <pattern>",
                            "mention" => "/mention <file|symbol>",
                            "tokens" => "/tokens <number>",
                            "goto" => "/goto <message number>",
                            "delete" => "/delete <session-id>",
                            "eval" => "/eval <expression>",
                            _ => &cmd_name,
                        };
                        self.app_state.toasts.info(format!("Usage: {}", usage));
                    }
                }
            },
            ModalType::ApprovalPicker => {
                let current = self.app_state.approval_mode_string();
                let interactive =
                    crate::interactive::builders::build_approval_selector(Some(&current));
                self.app_state.enter_interactive_mode(interactive);
            }
            ModalType::LogLevelPicker => {
                let current = self.app_state.log_level.clone();
                let interactive =
                    crate::interactive::builders::build_log_level_selector(Some(&current));
                self.app_state.enter_interactive_mode(interactive);
            }
            ModalType::Timeline => {
                self.app_state
                    .toasts
                    .info("Timeline: Use scroll to navigate messages");
            }
            ModalType::ThemePicker => {
                let current = self.app_state.settings.get("theme").map(|s| s.as_str());
                let interactive = crate::interactive::builders::build_theme_selector(current);
                self.app_state.enter_interactive_mode(interactive);
            }
            ModalType::Fork => {
                if let Some(ref session) = self.cortex_session {
                    let fork_name = format!("{} (fork)", session.title());
                    self.app_state
                        .toasts
                        .success(format!("Forked: {}", fork_name));
                } else {
                    self.app_state.toasts.error("No active session to fork");
                }
            }
            ModalType::FilePicker => {
                let cwd = std::env::current_dir().unwrap_or_default();
                let interactive = crate::interactive::builders::build_file_browser(&cwd);
                self.app_state.enter_interactive_mode(interactive);
            }
            ModalType::Export(format) => {
                if let Some(fmt) = format {
                    match fmt.as_str() {
                        "md" | "markdown" => {
                            let _ = self.handle_export(ExportFormat::Markdown).await;
                        }
                        "json" => {
                            let _ = self.handle_export(ExportFormat::Json).await;
                        }
                        "txt" | "text" => {
                            let _ = self.handle_export(ExportFormat::Text).await;
                        }
                        _ => {
                            self.app_state
                                .toasts
                                .error(format!("Unknown format: {}", fmt));
                        }
                    }
                } else {
                    let interactive = crate::interactive::builders::build_export_selector();
                    self.app_state.enter_interactive_mode(interactive);
                }
            }
            ModalType::Confirm(msg) => {
                self.app_state.toasts.info(&msg);
            }
            ModalType::Login => {
                self.start_login_flow().await;
            }
            ModalType::Upgrade => {
                self.app_state.toasts.info("Checking for updates...");
            }
            ModalType::Agents => {
                let cwd = std::env::current_dir().ok();
                let terminal_height = self.app_state.terminal_size.1;
                let interactive = crate::interactive::builders::build_agents_selector(
                    cwd.as_deref(),
                    Some(terminal_height),
                );
                self.app_state.enter_interactive_mode(interactive);
            }
            _ => {
                self.app_state
                    .toasts
                    .error(format!("Not implemented: {:?}", modal_type));
            }
        }
    }

    /// Handle async commands
    pub(super) async fn handle_async_command(&mut self, cmd: &str) -> Result<()> {
        // This is a stub - the full implementation is in the main event_loop module
        // For now, we handle basic commands here
        tracing::debug!("Async command: {}", cmd);

        match cmd {
            "providers:list" => {
                self.handle_providers_list();
            }
            "session:info" => {
                self.handle_session_info();
            }
            "transcript" => {
                self.handle_transcript();
            }
            "history" => {
                self.handle_history();
            }
            _ => {
                self.app_state
                    .toasts
                    .warning(format!("Command not yet implemented: {}", cmd));
            }
        }

        Ok(())
    }

    /// Handle export command
    pub(super) async fn handle_export(&mut self, format: ExportFormat) -> Result<()> {
        let Some(session) = &self.cortex_session else {
            self.add_system_message("No active session to export.");
            return Ok(());
        };

        let filename = default_export_filename(session, format);
        let content = match export_session(session, format) {
            Ok(c) => c,
            Err(e) => {
                self.add_system_message(&format!("Export failed: {}", e));
                return Ok(());
            }
        };

        let export_dir = dirs::document_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let export_path = export_dir.join(&filename);

        self.add_system_message(&format!("Exporting session as {}...", format.name()));

        let export_path_clone = export_path.clone();

        let result =
            tokio::task::spawn_blocking(move || std::fs::write(&export_path_clone, content)).await;

        if let Some(last_msg) = self.app_state.messages.last()
            && last_msg.content.starts_with("Exporting session as")
        {
            self.app_state.messages.pop();
        }

        match result {
            Ok(Ok(())) => {
                tracing::info!(
                    path = %export_path.display(),
                    format = %format.name(),
                    "Session exported successfully"
                );
                self.add_system_message(&format!(
                    "Session exported as {} to:\n{}",
                    format.name(),
                    export_path.display()
                ));
            }
            Ok(Err(e)) => {
                tracing::error!(error = %e, "Export failed");
                self.add_system_message(&format!("Export failed: {}", e));
            }
            Err(e) => {
                tracing::error!(error = %e, "Export task failed");
                self.add_system_message(&format!("Export failed: {}", e));
            }
        }

        Ok(())
    }

    /// Handle providers list command
    fn handle_providers_list(&mut self) {
        let mut output = String::from("Provider:\n\n");

        let is_authenticated = cortex_login::has_valid_auth();

        if is_authenticated {
            output.push_str("  [x] Cortex - Authenticated\n\n");
        } else {
            output.push_str("  [ ] Cortex - Not authenticated\n\n");
            output.push_str("Run /login to authenticate.");
        }

        self.add_system_message(&output);
    }

    /// Handle session info command
    fn handle_session_info(&mut self) {
        if let Some(ref session) = self.cortex_session {
            let mut output = String::from("Session Info:\n");
            output.push_str(&format!("  ID: {}\n", session.id()));
            output.push_str(&format!("  Title: {}\n", session.title()));
            output.push_str(&format!("  Provider: {}\n", session.meta.provider));
            output.push_str(&format!("  Model: {}\n", session.meta.model));
            output.push_str(&format!("  Messages: {}\n", session.message_count()));
            output.push_str(&format!("  Tokens: {}\n", session.format_tokens()));
            output.push_str(&format!(
                "  Created: {}\n",
                session.meta.created_at.format("%Y-%m-%d %H:%M")
            ));
            self.add_system_message(&output);
        } else {
            self.add_system_message("No active session.");
        }
    }

    /// Handle transcript command
    pub(super) fn handle_transcript(&mut self) {
        if let Some(ref session) = self.cortex_session {
            let mut output = "=== Session Transcript ===\n".to_string();
            output.push_str(&format!("Title: {}\n", session.title()));
            output.push_str(&format!("Messages: {}\n\n", session.message_count()));

            for (i, msg) in session.messages().iter().enumerate() {
                let role = match msg.role.as_str() {
                    "user" => "User",
                    "assistant" => "Assistant",
                    "system" => "System",
                    _ => "Unknown",
                };
                output.push_str(&format!("[{}] {}:\n{}\n\n", i + 1, role, msg.content));
            }

            self.add_system_message(&output);
        } else {
            self.add_system_message("No active session.");
        }
    }

    /// Handle history command
    fn handle_history(&mut self) {
        if self.app_state.command_history.is_empty() {
            self.add_system_message("No command history yet. Try running some commands first!");
            return;
        }

        let mut output = String::from("📜 Command History\n");
        output.push_str(&"─".repeat(40));
        output.push('\n');

        for (i, cmd) in self
            .app_state
            .command_history
            .iter()
            .rev()
            .take(20)
            .enumerate()
        {
            output.push_str(&format!("{}. /{}\n", i + 1, cmd));
        }

        self.add_system_message(&output);
    }

    /// Handle set value commands
    pub(super) fn handle_set_value(&mut self, key: &str, value: &str) {
        match key {
            "model" => {
                self.app_state.model = value.to_string();
                if let Some(pm) = &self.provider_manager
                    && let Ok(mut manager) = pm.try_write()
                {
                    let _ = manager.set_model(value);
                }
                self.update_session_model(value);
                self.add_system_message(&format!("Model set to: {}", value));
            }
            "provider" => {
                self.app_state.provider = value.to_string();
                if let Some(pm) = &self.provider_manager
                    && let Ok(mut manager) = pm.try_write()
                {
                    let _ = manager.set_provider(value);
                }
                self.add_system_message(&format!("Provider set to: {}", value));
            }
            "session_name" => {
                if let Some(ref mut session) = self.cortex_session {
                    session.set_title(value);
                }
                self.app_state.toasts.success(format!("Renamed: {}", value));
            }
            "temperature" => {
                if let Ok(temp) = value.parse::<f32>() {
                    let temp = temp.clamp(0.0, 2.0);
                    self.app_state.temperature = temp;
                    self.app_state
                        .toasts
                        .success(format!("Temperature: {:.1}", temp));
                } else {
                    self.app_state.toasts.error("Invalid temperature value");
                }
            }
            _ => {
                self.app_state
                    .settings
                    .insert(key.to_string(), value.to_string());
            }
        }
    }
}
