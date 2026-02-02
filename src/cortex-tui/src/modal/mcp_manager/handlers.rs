//! MCP Manager event handlers
//!
//! Contains all key event handling for the MCP Manager modal.

use super::registry::{get_local_registry_entries, get_registry_server_config};
use super::state::McpManagerModal;
use super::types::{
    AddHttpServerFocus, AddStdioServerFocus, McpMode, McpServerSource, McpStatus, RegistryLoadState,
};
use crate::modal::{ModalAction, ModalResult};
use crate::widgets::selection_list::SelectionResult;
use crossterm::event::{KeyCode, KeyEvent};

impl McpManagerModal {
    /// Handle key events in list mode
    pub(crate) fn handle_list_key(&mut self, key: KeyEvent) -> ModalResult {
        match key.code {
            KeyCode::Char('a') if key.modifiers.is_empty() => {
                // Start the add server wizard - first step: choose source
                self.mode = McpMode::ChooseSource { selected: 0 };
                ModalResult::Continue
            }
            KeyCode::Char('d') if key.modifiers.is_empty() => {
                if let Some(server) = self.selected_server() {
                    self.mode = McpMode::ConfirmDelete {
                        server_name: server.name.clone(),
                    };
                }
                ModalResult::Continue
            }
            KeyCode::Char('r') if key.modifiers.is_empty() => {
                if let Some(server) = self.selected_server() {
                    return ModalResult::Action(ModalAction::RestartMcpServer(server.name.clone()));
                }
                ModalResult::Continue
            }
            KeyCode::Char('s') if key.modifiers.is_empty() => {
                if let Some(server) = self.selected_server() {
                    let action = match server.status {
                        McpStatus::Running => ModalAction::StopMcpServer(server.name.clone()),
                        McpStatus::Stopped | McpStatus::Error => {
                            ModalAction::StartMcpServer(server.name.clone())
                        }
                        _ => return ModalResult::Continue,
                    };
                    return ModalResult::Action(action);
                }
                ModalResult::Continue
            }
            KeyCode::Char('k') if key.modifiers.is_empty() => {
                if let Some(server) = self.selected_server() {
                    self.mode = McpMode::SetAuth {
                        server_name: server.name.clone(),
                        api_key: String::new(),
                    };
                }
                ModalResult::Continue
            }
            KeyCode::Esc => ModalResult::Close,
            _ => {
                // Delegate to selection list
                match self.list.handle_key(key) {
                    SelectionResult::Selected(_) => ModalResult::Continue, // Just highlight, don't close
                    SelectionResult::Cancelled => ModalResult::Close,
                    SelectionResult::None => ModalResult::Continue,
                }
            }
        }
    }

    /// Handle key events in choose source mode (Step 1)
    pub(crate) fn handle_choose_source_key(&mut self, key: KeyEvent) -> ModalResult {
        if let McpMode::ChooseSource { ref mut selected } = self.mode {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    *selected = if *selected == 0 { 1 } else { 0 };
                    ModalResult::Continue
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = if *selected == 1 { 0 } else { 1 };
                    ModalResult::Continue
                }
                KeyCode::Enter => {
                    let source = if *selected == 0 {
                        McpServerSource::Custom
                    } else {
                        McpServerSource::Registry
                    };
                    match source {
                        McpServerSource::Custom => {
                            // Go to step 2: choose transport type
                            self.mode = McpMode::ChooseTransport {
                                _source: source,
                                selected: 0,
                            };
                        }
                        McpServerSource::Registry => {
                            // Go to registry selection with local entries preloaded
                            self.mode = McpMode::SelectFromRegistry {
                                selected: 0,
                                search_query: String::new(),
                                entries: get_local_registry_entries(),
                                _load_state: RegistryLoadState::Loaded,
                            };
                        }
                    }
                    ModalResult::Continue
                }
                KeyCode::Esc => {
                    self.mode = McpMode::List;
                    ModalResult::Continue
                }
                _ => ModalResult::Continue,
            }
        } else {
            ModalResult::Continue
        }
    }

    /// Handle key events in choose transport mode (Step 2)
    pub(crate) fn handle_choose_transport_key(&mut self, key: KeyEvent) -> ModalResult {
        if let McpMode::ChooseTransport {
            _source: _,
            ref mut selected,
        } = self.mode
        {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    *selected = if *selected == 0 { 1 } else { 0 };
                    ModalResult::Continue
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = if *selected == 1 { 0 } else { 1 };
                    ModalResult::Continue
                }
                KeyCode::Enter => {
                    if *selected == 0 {
                        // Stdio transport
                        self.mode = McpMode::AddStdioServer {
                            name: String::new(),
                            command: String::new(),
                            args: String::new(),
                            focus: AddStdioServerFocus::Name,
                        };
                    } else {
                        // HTTP transport
                        self.mode = McpMode::AddHttpServer {
                            name: String::new(),
                            url: String::new(),
                            focus: AddHttpServerFocus::Name,
                        };
                    }
                    ModalResult::Continue
                }
                KeyCode::Esc => {
                    // Go back to step 1
                    self.mode = McpMode::ChooseSource { selected: 0 };
                    ModalResult::Continue
                }
                _ => ModalResult::Continue,
            }
        } else {
            ModalResult::Continue
        }
    }

    /// Handle key events in add stdio server mode (Step 3a)
    pub(crate) fn handle_add_stdio_server_key(&mut self, key: KeyEvent) -> ModalResult {
        if let McpMode::AddStdioServer {
            ref mut name,
            ref mut command,
            ref mut args,
            ref mut focus,
        } = self.mode
        {
            match key.code {
                KeyCode::Tab => {
                    *focus = match focus {
                        AddStdioServerFocus::Name => AddStdioServerFocus::Command,
                        AddStdioServerFocus::Command => AddStdioServerFocus::Args,
                        AddStdioServerFocus::Args => AddStdioServerFocus::Name,
                    };
                    ModalResult::Continue
                }
                KeyCode::BackTab => {
                    *focus = match focus {
                        AddStdioServerFocus::Name => AddStdioServerFocus::Args,
                        AddStdioServerFocus::Command => AddStdioServerFocus::Name,
                        AddStdioServerFocus::Args => AddStdioServerFocus::Command,
                    };
                    ModalResult::Continue
                }
                KeyCode::Enter => {
                    if !name.is_empty() && !command.is_empty() {
                        let args_vec: Vec<String> =
                            args.split_whitespace().map(|s| s.to_string()).collect();
                        return ModalResult::Action(ModalAction::AddMcpServer {
                            name: name.clone(),
                            command: command.clone(),
                            args: args_vec,
                        });
                    }
                    ModalResult::Continue
                }
                KeyCode::Esc => {
                    // Go back to step 2
                    self.mode = McpMode::ChooseTransport {
                        _source: McpServerSource::Custom,
                        selected: 0,
                    };
                    ModalResult::Continue
                }
                KeyCode::Char(c) => {
                    match focus {
                        AddStdioServerFocus::Name => name.push(c),
                        AddStdioServerFocus::Command => command.push(c),
                        AddStdioServerFocus::Args => args.push(c),
                    }
                    ModalResult::Continue
                }
                KeyCode::Backspace => {
                    match focus {
                        AddStdioServerFocus::Name => {
                            name.pop();
                        }
                        AddStdioServerFocus::Command => {
                            command.pop();
                        }
                        AddStdioServerFocus::Args => {
                            args.pop();
                        }
                    }
                    ModalResult::Continue
                }
                _ => ModalResult::Continue,
            }
        } else {
            ModalResult::Continue
        }
    }

    /// Handle key events in add HTTP server mode (Step 3b)
    pub(crate) fn handle_add_http_server_key(&mut self, key: KeyEvent) -> ModalResult {
        if let McpMode::AddHttpServer {
            ref mut name,
            ref mut url,
            ref mut focus,
        } = self.mode
        {
            match key.code {
                KeyCode::Tab | KeyCode::BackTab => {
                    *focus = match focus {
                        AddHttpServerFocus::Name => AddHttpServerFocus::Url,
                        AddHttpServerFocus::Url => AddHttpServerFocus::Name,
                    };
                    ModalResult::Continue
                }
                KeyCode::Enter => {
                    if !name.is_empty() && !url.is_empty() {
                        return ModalResult::Action(ModalAction::AddMcpServerHttp {
                            name: name.clone(),
                            url: url.clone(),
                        });
                    }
                    ModalResult::Continue
                }
                KeyCode::Esc => {
                    // Go back to step 2
                    self.mode = McpMode::ChooseTransport {
                        _source: McpServerSource::Custom,
                        selected: 1,
                    };
                    ModalResult::Continue
                }
                KeyCode::Char(c) => {
                    match focus {
                        AddHttpServerFocus::Name => name.push(c),
                        AddHttpServerFocus::Url => url.push(c),
                    }
                    ModalResult::Continue
                }
                KeyCode::Backspace => {
                    match focus {
                        AddHttpServerFocus::Name => {
                            name.pop();
                        }
                        AddHttpServerFocus::Url => {
                            url.pop();
                        }
                    }
                    ModalResult::Continue
                }
                _ => ModalResult::Continue,
            }
        } else {
            ModalResult::Continue
        }
    }

    /// Handle key events in select from registry mode (Step 3c)
    pub(crate) fn handle_select_from_registry_key(&mut self, key: KeyEvent) -> ModalResult {
        if let McpMode::SelectFromRegistry {
            ref mut selected,
            ref mut search_query,
            ref entries,
            _load_state: _,
        } = self.mode
        {
            // Filter entries based on search
            let filtered: Vec<_> = if search_query.is_empty() {
                entries.iter().collect()
            } else {
                let query_lower = search_query.to_lowercase();
                entries
                    .iter()
                    .filter(|e| {
                        e.name.to_lowercase().contains(&query_lower)
                            || e.description.to_lowercase().contains(&query_lower)
                            || e.tags
                                .iter()
                                .any(|t| t.to_lowercase().contains(&query_lower))
                            || e.category
                                .as_ref()
                                .map(|c| c.to_lowercase().contains(&query_lower))
                                .unwrap_or(false)
                    })
                    .collect()
            };

            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                    ModalResult::Continue
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if *selected + 1 < filtered.len() {
                        *selected += 1;
                    }
                    ModalResult::Continue
                }
                KeyCode::Enter => {
                    if let Some(entry) = filtered.get(*selected) {
                        let (command, args) = get_registry_server_config(&entry.name);
                        return ModalResult::Action(ModalAction::AddMcpServer {
                            name: entry.name.clone(),
                            command,
                            args,
                        });
                    }
                    ModalResult::Continue
                }
                KeyCode::Esc => {
                    // Go back to step 1
                    self.mode = McpMode::ChooseSource { selected: 1 };
                    ModalResult::Continue
                }
                KeyCode::Char(c) => {
                    search_query.push(c);
                    *selected = 0; // Reset selection when search changes
                    ModalResult::Continue
                }
                KeyCode::Backspace => {
                    search_query.pop();
                    *selected = 0; // Reset selection when search changes
                    ModalResult::Continue
                }
                _ => ModalResult::Continue,
            }
        } else {
            ModalResult::Continue
        }
    }

    /// Handle key events in confirm delete mode
    pub(crate) fn handle_confirm_delete_key(&mut self, key: KeyEvent) -> ModalResult {
        if let McpMode::ConfirmDelete { ref server_name } = self.mode {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let name = server_name.clone();
                    self.mode = McpMode::List;
                    ModalResult::Action(ModalAction::RemoveMcpServer(name))
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.mode = McpMode::List;
                    ModalResult::Continue
                }
                _ => ModalResult::Continue,
            }
        } else {
            ModalResult::Continue
        }
    }

    /// Handle key events in set auth mode
    pub(crate) fn handle_set_auth_key(&mut self, key: KeyEvent) -> ModalResult {
        if let McpMode::SetAuth {
            ref server_name,
            ref mut api_key,
        } = self.mode
        {
            match key.code {
                KeyCode::Enter => {
                    if !api_key.is_empty() {
                        let action = ModalAction::AuthMcpServer {
                            name: server_name.clone(),
                            api_key: api_key.clone(),
                        };
                        self.mode = McpMode::List;
                        return ModalResult::Action(action);
                    }
                    ModalResult::Continue
                }
                KeyCode::Esc => {
                    self.mode = McpMode::List;
                    ModalResult::Continue
                }
                KeyCode::Char(c) => {
                    api_key.push(c);
                    ModalResult::Continue
                }
                KeyCode::Backspace => {
                    api_key.pop();
                    ModalResult::Continue
                }
                _ => ModalResult::Continue,
            }
        } else {
            ModalResult::Continue
        }
    }
}
