//! Modal handling: keyboard input for modal dialogs.

use anyhow::Result;

use crate::app::ActiveModal;
use crate::commands::CommandResult;
use crate::modal::ModalAction;
use crate::session::CortexSession;

use super::core::EventLoop;

impl EventLoop {
    /// Handles keyboard input for modal dialogs.
    /// Returns true if the key was handled by the modal.
    pub(super) async fn handle_modal_key(
        &mut self,
        key_event: crossterm::event::KeyEvent,
    ) -> Result<bool> {
        use crossterm::event::KeyCode;

        let Some(modal) = self.app_state.active_modal.as_mut() else {
            return Ok(false);
        };

        match modal {
            ActiveModal::ModelPicker => {
                match key_event.code {
                    KeyCode::Esc => {
                        self.app_state.close_modal();
                    }
                    KeyCode::Up => {
                        self.app_state.model_picker.select_prev();
                    }
                    KeyCode::Down => {
                        self.app_state.model_picker.select_next();
                    }
                    KeyCode::Enter => {
                        if let Some(model) = self.app_state.model_picker.selected_model() {
                            let model_id = model.id.clone();
                            self.app_state.model = model_id.clone();

                            if let Some(pm) = &self.provider_manager
                                && let Ok(mut manager) = pm.try_write()
                            {
                                let _ = manager.set_model(&model_id);
                            }

                            if let Ok(mut config) = crate::providers::config::CortexConfig::load() {
                                let _ = config.save_last_model(&self.app_state.provider, &model_id);
                            }

                            self.update_session_model(&model_id);

                            self.app_state.close_modal();
                        }
                    }
                    KeyCode::Char(c)
                        if key_event
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                            && c == 'l' =>
                    {
                        self.app_state.model_picker.clear_search();
                    }
                    KeyCode::Char(c) => {
                        self.app_state.model_picker.handle_char(c);
                    }
                    KeyCode::Backspace => {
                        self.app_state.model_picker.handle_backspace();
                    }
                    _ => {}
                }
                Ok(true)
            }
            ActiveModal::Form(form_state) => {
                match key_event.code {
                    KeyCode::Esc => {
                        self.app_state.close_modal();
                    }
                    KeyCode::Tab | KeyCode::Down => {
                        form_state.focus_next();
                    }
                    KeyCode::BackTab | KeyCode::Up => {
                        form_state.focus_prev();
                    }
                    KeyCode::Char(c) => {
                        form_state.handle_char(c);
                    }
                    KeyCode::Backspace => {
                        form_state.handle_backspace();
                    }
                    KeyCode::Enter => {
                        let on_submit_button = form_state.focus_index == form_state.fields.len();
                        let current_field_is_input = form_state.focus_index
                            < form_state.fields.len()
                            && matches!(
                                form_state.fields[form_state.focus_index].kind,
                                crate::widgets::form::FieldKind::Text
                                    | crate::widgets::form::FieldKind::Number
                                    | crate::widgets::form::FieldKind::Secret
                            );

                        if on_submit_button || (current_field_is_input && form_state.can_submit()) {
                            let command = form_state.command.clone();
                            let values: std::collections::HashMap<String, String> = form_state
                                .fields
                                .iter()
                                .map(|f| (f.key.clone(), f.value.clone()))
                                .collect();
                            self.app_state.close_modal();
                            self.handle_form_submission(&command, values);
                        } else {
                            form_state.toggle_current();
                        }
                    }
                    KeyCode::Left | KeyCode::Right => {
                        form_state.toggle_current();
                    }
                    _ => {}
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Handles form submission by reconstructing and executing the slash command.
    pub(super) fn handle_form_submission(
        &mut self,
        command: &str,
        values: std::collections::HashMap<String, String>,
    ) {
        let get = |key: &str| -> &str { values.get(key).map(|s| s.as_str()).unwrap_or("") };

        let cmd_str = match command {
            "rename" => {
                let name = get("name");
                if !name.is_empty() {
                    format!("/rename {}", name)
                } else {
                    return;
                }
            }
            "export" => {
                let format = get("format");
                if !format.is_empty() {
                    format!("/export {}", format)
                } else {
                    "/export markdown".to_string()
                }
            }
            "temperature" => {
                let value = get("value");
                if !value.is_empty() {
                    format!("/temperature {}", value)
                } else {
                    return;
                }
            }
            "model" => {
                let name = get("model");
                if !name.is_empty() {
                    format!("/model {}", name)
                } else {
                    return;
                }
            }
            _ => {
                let args: Vec<&str> = values
                    .values()
                    .map(|s| s.as_str())
                    .filter(|v| !v.is_empty())
                    .collect();

                if args.is_empty() {
                    format!("/{}", command)
                } else {
                    format!("/{} {}", command, args.join(" "))
                }
            }
        };

        let cmd_name = cmd_str.trim_start_matches('/');
        self.app_state.add_to_history(cmd_name);

        let result = self.command_executor.execute_str(&cmd_str);

        match result {
            CommandResult::Success => {}
            CommandResult::Message(msg) => {
                self.add_system_message(&msg);
            }
            CommandResult::Error(err) => {
                self.add_system_message(&format!("Error: {}", err));
            }
            CommandResult::SetValue(key, value) => {
                self.handle_set_value(&key, &value);
            }
            CommandResult::Clear => {
                self.app_state.clear_messages();
            }
            CommandResult::Quit => {
                self.app_state.set_quit();
            }
            CommandResult::NotFound(cmd) => {
                self.add_system_message(&format!("Unknown command: {}", cmd));
            }
            CommandResult::NeedsArgs(msg) => {
                self.add_system_message(&msg);
            }
            _ => {}
        }
    }

    /// Process a modal action returned by the modal stack.
    pub(super) async fn process_modal_action(&mut self, action: ModalAction) {
        match action {
            ModalAction::SelectModel(model_id) => {
                self.app_state.model = model_id.clone();
                if let Some(pm) = &self.provider_manager
                    && let Ok(mut manager) = pm.try_write()
                {
                    let _ = manager.set_model(&model_id);
                }
                if let Ok(mut config) = crate::providers::config::CortexConfig::load() {
                    let _ = config.save_last_model(&self.app_state.provider, &model_id);
                }
                self.update_session_model(&model_id);
            }
            ModalAction::SelectProvider(provider_id) => {
                self.app_state.provider = provider_id.clone();
                let (switch_result, new_model_for_session): (
                    Option<Result<String, String>>,
                    Option<String>,
                ) = if let Some(pm) = &self.provider_manager {
                    if let Ok(mut manager) = pm.try_write() {
                        if manager.set_provider(&provider_id).is_ok() {
                            let new_model = manager.current_model().to_string();
                            self.app_state.model = new_model.clone();

                            if let Ok(mut config) = crate::providers::config::CortexConfig::load() {
                                let _ = config.save_last_model(&provider_id, &new_model);
                            }

                            (
                                Some(Ok(format!(
                                    "Switched to provider: {} (model: {})",
                                    provider_id, new_model
                                ))),
                                Some(new_model),
                            )
                        } else {
                            (
                                Some(Err(format!(
                                    "Failed to switch to provider: {}",
                                    provider_id
                                ))),
                                None,
                            )
                        }
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                };

                if let Some(new_model) = new_model_for_session {
                    self.update_session_model(&new_model);
                }

                if let Some(result) = switch_result {
                    match result {
                        Ok(msg) => self.add_system_message(&msg),
                        Err(msg) => self.add_system_message(&msg),
                    }
                }
            }
            ModalAction::ConfigureProvider(_provider_id) => {
                self.app_state
                    .toasts
                    .warning("Please run `cortex login` to authenticate");
            }
            ModalAction::ExecuteCommand(cmd) => {
                let cmd_str = if cmd.starts_with('/') {
                    cmd.clone()
                } else {
                    format!("/{}", cmd)
                };

                let cmd_name = cmd_str.trim_start_matches('/');
                self.app_state.add_to_history(cmd_name);

                let result = self.command_executor.execute_str(&cmd_str);
                match result {
                    CommandResult::Success => {}
                    CommandResult::Message(msg) => {
                        self.add_system_message(&msg);
                    }
                    CommandResult::Error(err) => {
                        self.add_system_message(&format!("Error: {}", err));
                    }
                    CommandResult::Clear => {
                        self.app_state.clear_messages();
                    }
                    CommandResult::Quit => {
                        self.app_state.set_quit();
                    }
                    CommandResult::NotFound(cmd) => {
                        self.add_system_message(&format!("Unknown command: {}", cmd));
                    }
                    _ => {}
                }
            }
            ModalAction::SelectSession(path) => {
                let session_id = path.to_string_lossy().to_string();
                if let Ok(session) = CortexSession::load(&session_id) {
                    self.cortex_session = Some(session);
                    self.app_state.set_view(crate::app::AppView::Session);
                    self.add_system_message(&format!("Resumed session: {}", session_id));
                } else {
                    self.add_system_message(&format!("Failed to load session: {}", session_id));
                }
            }
            ModalAction::NewSession => {
                self.app_state.new_session();
                self.add_system_message("New session started");
            }
            ModalAction::Custom(data) => {
                // Handle theme selection from ThemeSelectorModal
                if let Some(theme_name) = data.strip_prefix("theme:") {
                    self.app_state.set_theme(theme_name);
                    // Persist theme preference to config
                    if let Ok(mut config) = crate::providers::config::CortexConfig::load() {
                        let _ = config.save_last_theme(theme_name);
                    }
                    self.app_state
                        .toasts
                        .success(format!("Theme changed to: {}", theme_name));
                } else {
                    tracing::debug!("Custom modal action: {}", data);
                }
            }
            _ => {
                // Handle other modal actions
                tracing::debug!("Unhandled modal action: {:?}", action);
            }
        }
    }

    /// Opens the MCP manager modal.
    pub fn open_mcp_manager(&mut self, servers: Vec<crate::modal::mcp_manager::McpServerInfo>) {
        use crate::modal::McpManagerModal;
        self.modal_stack
            .push(Box::new(McpManagerModal::new(servers)));
    }

    /// Opens the models modal with the new modal system.
    pub fn open_models_modal(&mut self) {
        use crate::modal::{ModelInfo, ModelsModal};
        let models: Vec<ModelInfo> = if let Some(ref pm) = self.provider_manager {
            if let Ok(manager) = pm.try_read() {
                let current_model = manager.current_model();
                let provider_name = manager.current_provider().to_string();
                manager
                    .available_models()
                    .iter()
                    .map(|m| ModelInfo {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        provider: provider_name.clone(),
                        context_length: Some(m.context_window),
                        description: None,
                        is_current: m.id == current_model,
                        credit_multiplier_input: m.credit_multiplier_input.clone(),
                        credit_multiplier_output: m.credit_multiplier_output.clone(),
                        price_version: m.price_version,
                    })
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let current = self.app_state.model.clone();
        self.modal_stack
            .push(Box::new(ModelsModal::new(models, Some(current))));
    }

    /// Opens the sessions modal with recent sessions.
    pub fn open_sessions_modal(&mut self) {
        use crate::modal::{SessionInfo, SessionsModal};

        match CortexSession::list_recent(20) {
            Ok(sessions) => {
                let session_infos: Vec<SessionInfo> = sessions
                    .into_iter()
                    .map(|s| SessionInfo {
                        path: std::path::PathBuf::from(&s.id),
                        name: if s.title.is_empty() {
                            "Untitled".to_string()
                        } else {
                            s.title
                        },
                        model: s.model,
                        created_at: s.created_at,
                        message_count: s.message_count as usize,
                    })
                    .collect();
                self.modal_stack
                    .push(Box::new(SessionsModal::new(session_infos)));
            }
            Err(e) => {
                self.add_system_message(&format!("Failed to list sessions: {}", e));
            }
        }
    }

    /// Handles inline form submission from interactive mode.
    pub(super) fn handle_inline_form_submission(
        &mut self,
        action_id: &str,
        values: std::collections::HashMap<String, String>,
    ) -> bool {
        match action_id {
            "mcp-add" | "mcp-add-stdio" => {
                let name = values.get("name").cloned().unwrap_or_default();
                let command = values.get("command").cloned().unwrap_or_default();
                let args_str = values.get("args").cloned().unwrap_or_default();

                if name.is_empty() || command.is_empty() {
                    self.app_state.toasts.error("Name and command are required");
                    return false;
                }

                let args: Vec<String> = if args_str.is_empty() {
                    Vec::new()
                } else {
                    args_str.split_whitespace().map(|s| s.to_string()).collect()
                };

                let server_info = crate::modal::mcp_manager::McpServerInfo {
                    name: name.clone(),
                    status: crate::modal::mcp_manager::McpStatus::Running,
                    tool_count: 0,
                    error: None,
                    requires_auth: false,
                };

                let stored_server = crate::mcp_storage::StoredMcpServer::new_stdio(
                    name.clone(),
                    command.clone(),
                    args.clone(),
                );
                if let Err(e) = self.save_mcp_server(&stored_server) {
                    tracing::error!("Failed to save MCP server config: {}", e);
                    self.app_state
                        .toasts
                        .error(format!("Failed to save MCP server: {}", e));
                    return false;
                }

                self.app_state.mcp_servers.push(server_info);
                self.app_state
                    .toasts
                    .success(format!("Added stdio MCP server: {}", name));

                self.reopen_mcp_panel();
                true
            }
            _ => {
                tracing::warn!("Unhandled inline form submission: {}", action_id);
                false
            }
        }
    }

    /// Re-opens the MCP panel to show the list of servers after adding a new one.
    pub(super) fn reopen_mcp_panel(&mut self) {
        use crate::interactive::builders::build_mcp_selector;
        let servers = self.app_state.mcp_servers.clone();
        let interactive = build_mcp_selector(&servers);
        self.app_state.enter_interactive_mode(interactive);
    }

    /// Handles interactive selection.
    pub(super) async fn handle_interactive_selection(
        &mut self,
        action: crate::interactive::InteractiveAction,
        item_id: String,
        _checked: Vec<String>,
    ) -> bool {
        use crate::interactive::InteractiveAction;

        match action {
            InteractiveAction::SetProvider => {
                if let Some(pm) = &self.provider_manager
                    && let Ok(mut manager) = pm.try_write()
                {
                    let _ = manager.set_provider(&item_id);
                    self.app_state.provider = item_id.clone();
                }
                return false;
            }
            InteractiveAction::SetModel => {
                if let Some(pm) = &self.provider_manager
                    && let Ok(mut manager) = pm.try_write()
                {
                    let _ = manager.set_model(&item_id);
                    self.app_state.model = item_id.clone();
                }
                self.update_session_model(&item_id);
                return false;
            }
            InteractiveAction::ToggleSetting => {
                if item_id.starts_with("__cat_") {
                    self.reopen_settings_menu();
                    return true;
                }
                match item_id.as_str() {
                    "compact" => {
                        self.app_state.compact_mode = !self.app_state.compact_mode;
                    }
                    "debug" => {
                        self.app_state.debug_mode = !self.app_state.debug_mode;
                    }
                    "sandbox" => {
                        self.app_state.sandbox_mode = !self.app_state.sandbox_mode;
                    }
                    "sound" => {
                        self.app_state.sound_enabled = !self.app_state.sound_enabled;
                    }
                    _ => {}
                };
                self.reopen_settings_menu();
                return true;
            }
            _ => {
                tracing::debug!("Unhandled interactive action: {:?}", action);
            }
        }
        false
    }

    /// Re-opens the settings menu with current state.
    fn reopen_settings_menu(&mut self) {
        use crate::interactive::builders::{SettingsSnapshot, build_settings_selector};

        let current_selected = self
            .app_state
            .get_interactive_state()
            .map(|s| s.selected)
            .unwrap_or(0);

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
        let mut interactive = build_settings_selector(snapshot, Some(terminal_height));

        if current_selected < interactive.items.len() {
            interactive.selected = current_selected;
        }

        self.app_state.enter_interactive_mode(interactive);
    }

    /// Process pending actions from the card handler.
    pub(super) fn process_card_actions(&mut self) {
        let actions = self.card_handler.take_actions();
        for _action in actions {
            tracing::debug!("Card action received");
            // Handle card actions - implementation in the main module
        }
    }
}
