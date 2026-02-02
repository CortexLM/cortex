//! MCP Manager rendering
//!
//! Contains all rendering methods for the MCP Manager modal.

use super::registry::RegistryEntry;
use super::state::McpManagerModal;
use super::types::{AddHttpServerFocus, AddStdioServerFocus, McpMode, McpStatus};
use crate::modal::render_search_bar;
use crate::widgets::action_bar::ActionBar;
use cortex_core::style::{
    BORDER, CYAN_PRIMARY, ERROR, SUCCESS, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED, WARNING,
};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear, Widget},
};

impl McpManagerModal {
    /// Render the list view with improved UX
    pub(crate) fn render_list(&self, area: Rect, buf: &mut Buffer) {
        // Clear background
        Clear.render(area, buf);

        let block = Block::default()
            .title(" MCP Servers ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(CYAN_PRIMARY))
            .style(Style::default().bg(SURFACE_1));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 3 {
            return;
        }

        // Layout: search bar, separator, server list, separator, action bar
        let chunks = Layout::vertical([
            Constraint::Length(1), // Search bar
            Constraint::Length(1), // Separator
            Constraint::Min(1),    // Server list
            Constraint::Length(1), // Separator
            Constraint::Length(1), // Action bar
        ])
        .split(inner);

        // Render search bar
        let search_area = chunks[0];
        render_search_bar(
            search_area,
            buf,
            self.list.search_query(),
            "Search servers...",
        );

        // Render top separator
        let sep_line = "─".repeat(inner.width as usize);
        buf.set_string(
            chunks[1].x,
            chunks[1].y,
            &sep_line,
            Style::default().fg(BORDER),
        );

        // Render server list
        let list_area = chunks[2];
        if self.servers.is_empty() {
            let text = "No MCP servers configured. Press 'a' to add one.";
            buf.set_string(
                list_area.x + 1,
                list_area.y,
                text,
                Style::default().fg(TEXT_DIM),
            );
        } else {
            self.render_server_rows(list_area, buf);
        }

        // Render bottom separator
        buf.set_string(
            chunks[3].x,
            chunks[3].y,
            &sep_line,
            Style::default().fg(BORDER),
        );

        // Render action bar
        let action_bar = self.build_action_bar();
        let bar_area = chunks[4];
        action_bar.render(bar_area, buf);
    }

    /// Render individual server rows with improved formatting
    pub(crate) fn render_server_rows(&self, area: Rect, buf: &mut Buffer) {
        // Calculate column widths
        // Format:  > [*] name          Status     N tools   [+] OK
        let status_col_width = 10u16; // "Running" etc.
        let tools_col_width = 10u16; // "12 tools"
        let right_status_width = 18u16; // "[+] OK" or "[x] Auth required"

        for (i, server) in self.servers.iter().enumerate() {
            if i as u16 >= area.height {
                break;
            }

            let y = area.y + i as u16;
            let is_selected = self.list.selected_index() == Some(i);

            // Selection indicator
            let selector = if is_selected { " > " } else { "   " };
            let selector_style =
                Style::default().fg(if is_selected { CYAN_PRIMARY } else { TEXT_DIM });
            buf.set_string(area.x, y, selector, selector_style);

            // Status icon with color
            let icon = server.status.icon();
            let icon_style = Style::default().fg(server.status.color());
            buf.set_string(area.x + 3, y, icon, icon_style);

            // Server name
            let name_style = if is_selected {
                Style::default()
                    .fg(CYAN_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT)
            };
            let name_max_len = (area.width as usize)
                .saturating_sub(5) // selector + icon + space
                .saturating_sub(status_col_width as usize)
                .saturating_sub(tools_col_width as usize)
                .saturating_sub(right_status_width as usize);
            let display_name: String = if server.name.len() > name_max_len {
                format!("{}…", &server.name[..name_max_len.saturating_sub(1)])
            } else {
                server.name.clone()
            };
            buf.set_string(area.x + 5, y, &display_name, name_style);

            // Status text column
            let status_text = server.status.text();
            let status_style = Style::default().fg(server.status.color());
            let status_x =
                area.x + area.width - right_status_width - tools_col_width - status_col_width;
            buf.set_string(status_x, y, status_text, status_style);

            // Tool count column
            let tools_text = if server.tool_count > 0 {
                format!("{:>3} tools", server.tool_count)
            } else {
                "  - tools".to_string()
            };
            let tools_style = Style::default().fg(TEXT_DIM);
            let tools_x = area.x + area.width - right_status_width - tools_col_width;
            buf.set_string(tools_x, y, &tools_text, tools_style);

            // Right side status indicator
            let (right_text, right_style) = match server.status {
                McpStatus::Running => ("[+] OK", Style::default().fg(SUCCESS)),
                McpStatus::Error if server.requires_auth => {
                    ("[x] Auth required", Style::default().fg(ERROR))
                }
                McpStatus::Error => ("[x] Error", Style::default().fg(ERROR)),
                McpStatus::Starting => ("...", Style::default().fg(WARNING)),
                McpStatus::Stopped => ("", Style::default()),
            };
            if !right_text.is_empty() {
                let right_x = area.x + area.width - right_text.len() as u16 - 1;
                buf.set_string(right_x, y, right_text, right_style);
            }
        }
    }

    /// Render the choose source view (Step 1)
    pub(crate) fn render_choose_source(&self, area: Rect, buf: &mut Buffer) {
        if let McpMode::ChooseSource { selected } = self.mode {
            Clear.render(area, buf);

            let block = Block::default()
                .title(" Add MCP Server - Choose Source ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(CYAN_PRIMARY))
                .style(Style::default().bg(SURFACE_1));

            let inner = block.inner(area);
            block.render(area, buf);

            // Options
            let options = [
                ("Custom", "Configure a server manually"),
                ("Registry", "Choose from predefined MCP servers"),
            ];

            for (i, (name, desc)) in options.iter().enumerate() {
                let y = inner.y + i as u16 * 2;
                if y >= inner.y + inner.height {
                    break;
                }

                let is_selected = selected == i;
                let selector = if is_selected { " > " } else { "   " };
                let selector_style =
                    Style::default().fg(if is_selected { CYAN_PRIMARY } else { TEXT_DIM });
                buf.set_string(inner.x, y, selector, selector_style);

                let name_style = if is_selected {
                    Style::default()
                        .fg(CYAN_PRIMARY)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(TEXT)
                };
                buf.set_string(inner.x + 3, y, name, name_style);

                // Description
                if y + 1 < inner.y + inner.height {
                    buf.set_string(inner.x + 3, y + 1, desc, Style::default().fg(TEXT_DIM));
                }
            }

            // Action bar
            if inner.height >= 6 {
                let bar = ActionBar::new()
                    .hint("↑↓", "select")
                    .hint("Enter", "confirm")
                    .hint("Esc", "cancel");
                let bar_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
                bar.render(bar_area, buf);
            }
        }
    }

    /// Render the choose transport view (Step 2)
    pub(crate) fn render_choose_transport(&self, area: Rect, buf: &mut Buffer) {
        if let McpMode::ChooseTransport {
            _source: _,
            selected,
        } = self.mode
        {
            Clear.render(area, buf);

            let block = Block::default()
                .title(" Add MCP Server - Choose Transport ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(CYAN_PRIMARY))
                .style(Style::default().bg(SURFACE_1));

            let inner = block.inner(area);
            block.render(area, buf);

            // Options
            let options = [
                ("Stdio", "Local subprocess (npx, uvx, binary)"),
                ("HTTP", "Remote HTTP server (URL endpoint)"),
            ];

            for (i, (name, desc)) in options.iter().enumerate() {
                let y = inner.y + i as u16 * 2;
                if y >= inner.y + inner.height {
                    break;
                }

                let is_selected = selected == i;
                let selector = if is_selected { " > " } else { "   " };
                let selector_style =
                    Style::default().fg(if is_selected { CYAN_PRIMARY } else { TEXT_DIM });
                buf.set_string(inner.x, y, selector, selector_style);

                let name_style = if is_selected {
                    Style::default()
                        .fg(CYAN_PRIMARY)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(TEXT)
                };
                buf.set_string(inner.x + 3, y, name, name_style);

                // Description
                if y + 1 < inner.y + inner.height {
                    buf.set_string(inner.x + 3, y + 1, desc, Style::default().fg(TEXT_DIM));
                }
            }

            // Action bar
            if inner.height >= 6 {
                let bar = ActionBar::new()
                    .hint("↑↓", "select")
                    .hint("Enter", "confirm")
                    .hint("Esc", "back");
                let bar_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
                bar.render(bar_area, buf);
            }
        }
    }

    /// Render the stdio server form (Step 3a)
    pub(crate) fn render_add_stdio_server(&self, area: Rect, buf: &mut Buffer) {
        if let McpMode::AddStdioServer {
            ref name,
            ref command,
            ref args,
            focus,
        } = self.mode
        {
            Clear.render(area, buf);

            let block = Block::default()
                .title(" Add MCP Server ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(CYAN_PRIMARY))
                .style(Style::default().bg(SURFACE_1));

            let inner = block.inner(area);
            block.render(area, buf);

            let chunks = Layout::vertical([
                Constraint::Length(2), // Name field
                Constraint::Length(2), // Command field
                Constraint::Length(2), // Args field
                Constraint::Length(1), // Separator
                Constraint::Length(1), // Action bar
            ])
            .split(inner);

            // Name field
            let name_focused = focus == AddStdioServerFocus::Name;
            let name_label_style = if name_focused {
                Style::default().fg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT_DIM)
            };
            buf.set_string(chunks[0].x, chunks[0].y, "Name:", name_label_style);
            let name_style = if name_focused {
                Style::default().fg(TEXT).add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(TEXT)
            };
            buf.set_string(chunks[0].x + 6, chunks[0].y, name, name_style);
            if name_focused && name.is_empty() {
                buf.set_string(
                    chunks[0].x + 6,
                    chunks[0].y,
                    "server-name",
                    Style::default().fg(TEXT_MUTED),
                );
            }

            // Command field
            let cmd_focused = focus == AddStdioServerFocus::Command;
            let cmd_label_style = if cmd_focused {
                Style::default().fg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT_DIM)
            };
            buf.set_string(chunks[1].x, chunks[1].y, "Command:", cmd_label_style);
            let cmd_style = if cmd_focused {
                Style::default().fg(TEXT).add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(TEXT)
            };
            buf.set_string(chunks[1].x + 9, chunks[1].y, command, cmd_style);
            if cmd_focused && command.is_empty() {
                buf.set_string(
                    chunks[1].x + 9,
                    chunks[1].y,
                    "npx or path/to/binary",
                    Style::default().fg(TEXT_MUTED),
                );
            }

            // Args field
            let args_focused = focus == AddStdioServerFocus::Args;
            let args_label_style = if args_focused {
                Style::default().fg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT_DIM)
            };
            buf.set_string(chunks[2].x, chunks[2].y, "Args:", args_label_style);
            let args_style = if args_focused {
                Style::default().fg(TEXT).add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(TEXT)
            };
            buf.set_string(chunks[2].x + 6, chunks[2].y, args, args_style);
            if args_focused && args.is_empty() {
                buf.set_string(
                    chunks[2].x + 6,
                    chunks[2].y,
                    "arg1 arg2 ...",
                    Style::default().fg(TEXT_MUTED),
                );
            }

            // Separator
            let sep_line = "─".repeat(inner.width as usize);
            buf.set_string(
                chunks[3].x,
                chunks[3].y,
                &sep_line,
                Style::default().fg(BORDER),
            );

            // Action bar for form
            let form_bar = ActionBar::new()
                .hint("Tab", "next")
                .hint("Enter", "save")
                .hint("Esc", "back");
            form_bar.render(chunks[4], buf);
        }
    }

    /// Render the HTTP server form (Step 3b)
    pub(crate) fn render_add_http_server(&self, area: Rect, buf: &mut Buffer) {
        if let McpMode::AddHttpServer {
            ref name,
            ref url,
            focus,
        } = self.mode
        {
            Clear.render(area, buf);

            let block = Block::default()
                .title(" Add MCP Server - HTTP ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(CYAN_PRIMARY))
                .style(Style::default().bg(SURFACE_1));

            let inner = block.inner(area);
            block.render(area, buf);

            let chunks = Layout::vertical([
                Constraint::Length(2), // Name field
                Constraint::Length(2), // URL field
                Constraint::Length(1), // Separator
                Constraint::Length(1), // Action bar
            ])
            .split(inner);

            // Name field
            let name_focused = focus == AddHttpServerFocus::Name;
            let name_label_style = if name_focused {
                Style::default().fg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT_DIM)
            };
            buf.set_string(chunks[0].x, chunks[0].y, "Name:", name_label_style);
            let name_style = if name_focused {
                Style::default().fg(TEXT).add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(TEXT)
            };
            buf.set_string(chunks[0].x + 6, chunks[0].y, name, name_style);
            if name_focused && name.is_empty() {
                buf.set_string(
                    chunks[0].x + 6,
                    chunks[0].y,
                    "server-name",
                    Style::default().fg(TEXT_MUTED),
                );
            }

            // URL field
            let url_focused = focus == AddHttpServerFocus::Url;
            let url_label_style = if url_focused {
                Style::default().fg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT_DIM)
            };
            buf.set_string(chunks[1].x, chunks[1].y, "URL:", url_label_style);
            let url_style = if url_focused {
                Style::default().fg(TEXT).add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(TEXT)
            };
            buf.set_string(chunks[1].x + 5, chunks[1].y, url, url_style);
            if url_focused && url.is_empty() {
                buf.set_string(
                    chunks[1].x + 5,
                    chunks[1].y,
                    "https://mcp-server.example.com",
                    Style::default().fg(TEXT_MUTED),
                );
            }

            // Separator
            let sep_line = "─".repeat(inner.width as usize);
            buf.set_string(
                chunks[2].x,
                chunks[2].y,
                &sep_line,
                Style::default().fg(BORDER),
            );

            // Action bar for form
            let form_bar = ActionBar::new()
                .hint("Tab", "next")
                .hint("Enter", "save")
                .hint("Esc", "back");
            form_bar.render(chunks[3], buf);
        }
    }

    /// Render the registry selection view (Step 3c)
    pub(crate) fn render_select_from_registry(&self, area: Rect, buf: &mut Buffer) {
        if let McpMode::SelectFromRegistry {
            selected,
            ref search_query,
            ref entries,
            _load_state: _,
        } = self.mode
        {
            Clear.render(area, buf);

            let block = Block::default()
                .title(" Add MCP Server - Registry ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(CYAN_PRIMARY))
                .style(Style::default().bg(SURFACE_1));

            let inner = block.inner(area);
            block.render(area, buf);

            if inner.height < 3 {
                return;
            }

            // Layout: search bar, separator, server list, separator, action bar
            let chunks = Layout::vertical([
                Constraint::Length(1), // Search bar
                Constraint::Length(1), // Separator
                Constraint::Min(1),    // Server list
                Constraint::Length(1), // Separator
                Constraint::Length(1), // Action bar
            ])
            .split(inner);

            // Render search bar
            render_search_bar(chunks[0], buf, search_query, "Search servers...");

            // Render top separator
            let sep_line = "─".repeat(inner.width as usize);
            buf.set_string(
                chunks[1].x,
                chunks[1].y,
                &sep_line,
                Style::default().fg(BORDER),
            );

            // Render server list
            let list_area = chunks[2];
            let filtered: Vec<&RegistryEntry> = if search_query.is_empty() {
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

            for (i, entry) in filtered.iter().enumerate() {
                if i as u16 >= list_area.height {
                    break;
                }

                let y = list_area.y + i as u16;
                let is_selected = selected == i;

                // Selection indicator
                let selector = if is_selected { " > " } else { "   " };
                let selector_style =
                    Style::default().fg(if is_selected { CYAN_PRIMARY } else { TEXT_DIM });
                buf.set_string(list_area.x, y, selector, selector_style);

                // Server name
                let name_style = if is_selected {
                    Style::default()
                        .fg(CYAN_PRIMARY)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(TEXT)
                };
                buf.set_string(list_area.x + 3, y, &entry.name, name_style);

                // Category indicator if available
                let mut desc_start_x = list_area.x + 3 + entry.name.len() as u16 + 1;
                if let Some(ref category) = entry.category {
                    let cat_text = format!("[{}]", category);
                    buf.set_string(desc_start_x, y, &cat_text, Style::default().fg(WARNING));
                    desc_start_x += cat_text.len() as u16 + 1;
                }

                // Required env indicator
                if !entry.required_env.is_empty() {
                    let env_indicator = "●";
                    buf.set_string(desc_start_x, y, env_indicator, Style::default().fg(WARNING));
                    desc_start_x += 2;
                }

                // Description (on same line, after name and indicators)
                if desc_start_x < list_area.x + list_area.width {
                    let max_desc_len = (list_area.x + list_area.width - desc_start_x) as usize;
                    let truncated_desc = if entry.description.len() > max_desc_len {
                        format!(
                            "{}...",
                            &entry.description[..max_desc_len.saturating_sub(3)]
                        )
                    } else {
                        entry.description.clone()
                    };
                    buf.set_string(
                        desc_start_x,
                        y,
                        &truncated_desc,
                        Style::default().fg(TEXT_DIM),
                    );
                }
            }

            // Render bottom separator
            buf.set_string(
                chunks[3].x,
                chunks[3].y,
                &sep_line,
                Style::default().fg(BORDER),
            );

            // Action bar
            let bar = ActionBar::new()
                .hint("↑↓", "select")
                .hint("Enter", "add")
                .hint("Esc", "back");
            bar.render(chunks[4], buf);
        }
    }

    /// Render the confirm delete dialog
    pub(crate) fn render_confirm_delete(&self, area: Rect, buf: &mut Buffer) {
        if let McpMode::ConfirmDelete { ref server_name } = self.mode {
            Clear.render(area, buf);

            let block = Block::default()
                .title(" Confirm Delete ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(WARNING))
                .style(Style::default().bg(SURFACE_1));

            let inner = block.inner(area);
            block.render(area, buf);

            // Warning message
            let text = format!("Delete server '{}'?", server_name);
            buf.set_string(inner.x + 1, inner.y, &text, Style::default().fg(TEXT));

            // Subtext
            buf.set_string(
                inner.x + 1,
                inner.y + 1,
                "This action cannot be undone.",
                Style::default().fg(TEXT_DIM),
            );

            // Action bar
            if inner.height >= 4 {
                let bar = ActionBar::new()
                    .danger('y', "Yes, Delete")
                    .action('n', "No, Cancel");
                let bar_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
                bar.render(bar_area, buf);
            }
        }
    }

    /// Render the set auth form
    pub(crate) fn render_set_auth(&self, area: Rect, buf: &mut Buffer) {
        if let McpMode::SetAuth {
            ref server_name,
            ref api_key,
        } = self.mode
        {
            Clear.render(area, buf);

            let block = Block::default()
                .title(format!(" API Key for {} ", server_name))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(CYAN_PRIMARY))
                .style(Style::default().bg(SURFACE_1));

            let inner = block.inner(area);
            block.render(area, buf);

            // API Key field
            buf.set_string(
                inner.x + 1,
                inner.y,
                "API Key:",
                Style::default().fg(TEXT_DIM),
            );

            // Mask the API key for security
            let masked: String = if api_key.is_empty() {
                "Enter your API key...".to_string()
            } else {
                "*".repeat(api_key.len())
            };
            let key_style = if api_key.is_empty() {
                Style::default().fg(TEXT_MUTED)
            } else {
                Style::default().fg(TEXT)
            };
            buf.set_string(inner.x + 10, inner.y, &masked, key_style);

            // Action bar
            if inner.height >= 3 {
                let bar = ActionBar::new().hint("Enter", "save").hint("Esc", "cancel");
                let bar_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
                bar.render(bar_area, buf);
            }
        }
    }
}
