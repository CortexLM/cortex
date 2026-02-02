//! Tasks view for displaying and managing background agents.
//!
//! This view provides a table-based display of all background agents,
//! showing their status, progress, and allowing user interaction.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};

use cortex_agents::background::{AgentStatus, RunningAgentInfo};
use cortex_common::{format_duration, truncate_first_line, truncate_id_default};

/// View for displaying background tasks/agents.
pub struct TasksView {
    /// List of agents to display.
    agents: Vec<RunningAgentInfo>,
    /// Currently selected agent index.
    selected: usize,
    /// Scroll offset for long lists.
    scroll_offset: usize,
}

impl Default for TasksView {
    fn default() -> Self {
        Self::new()
    }
}

impl TasksView {
    /// Creates a new empty tasks view.
    pub fn new() -> Self {
        Self {
            agents: Vec::new(),
            selected: 0,
            scroll_offset: 0,
        }
    }

    /// Updates the list of agents.
    pub fn set_agents(&mut self, agents: Vec<RunningAgentInfo>) {
        self.agents = agents;
        // Adjust selection if needed
        if self.selected >= self.agents.len() && !self.agents.is_empty() {
            self.selected = self.agents.len() - 1;
        }
    }

    /// Returns the number of agents.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Returns true if there are no agents.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Moves selection up.
    pub fn select_previous(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Moves selection down.
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.agents.len() {
            self.selected += 1;
        }
    }

    /// Returns the currently selected agent.
    pub fn selected_agent(&self) -> Option<&RunningAgentInfo> {
        self.agents.get(self.selected)
    }

    /// Returns the ID of the selected agent.
    pub fn selected_agent_id(&self) -> Option<&str> {
        self.selected_agent().map(|a| a.id.as_str())
    }

    /// Renders the tasks view.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        // Clear the area
        frame.render_widget(Clear, area);

        // Create outer block
        let block = Block::default()
            .title(" Background Tasks ")
            .title_style(Style::default().fg(Color::Cyan).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.agents.is_empty() {
            // Show empty state
            let empty_text = Paragraph::new("No background tasks running.\n\nPress Ctrl+B to run the current prompt in background.")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);

            // Center vertically
            let vertical_center = inner.height / 2;
            let text_area = Rect {
                x: inner.x,
                y: inner.y + vertical_center.saturating_sub(1),
                width: inner.width,
                height: 3,
            };
            frame.render_widget(empty_text, text_area);
            return;
        }

        // Calculate visible rows
        let header_height = 1;
        let visible_rows = (inner.height as usize).saturating_sub(header_height + 1);

        // Adjust scroll offset to keep selection visible
        let scroll_offset = if self.selected < self.scroll_offset {
            self.selected
        } else if self.selected >= self.scroll_offset + visible_rows {
            self.selected - visible_rows + 1
        } else {
            self.scroll_offset
        };

        // Create table header
        let header = Row::new(vec![
            Cell::from("ID").style(Style::default().fg(Color::Yellow).bold()),
            Cell::from("Status").style(Style::default().fg(Color::Yellow).bold()),
            Cell::from("Task").style(Style::default().fg(Color::Yellow).bold()),
            Cell::from("Duration").style(Style::default().fg(Color::Yellow).bold()),
            Cell::from("Tokens").style(Style::default().fg(Color::Yellow).bold()),
        ])
        .height(1)
        .bottom_margin(1);

        // Create table rows
        let rows: Vec<Row> = self
            .agents
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_rows)
            .map(|(idx, agent)| {
                let is_selected = idx == self.selected;
                let style = if is_selected {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    Cell::from(truncate_id_default(&agent.id).into_owned()).style(style),
                    Cell::from(status_badge(&agent.status)),
                    Cell::from(truncate_first_line(&agent.task, 40).into_owned()).style(style),
                    Cell::from(format_duration(agent.duration())).style(style),
                    Cell::from(format!("{}", agent.tokens_used)).style(style),
                ])
                .style(style)
            })
            .collect();

        // Define column widths
        let widths = [
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Percentage(45),
            Constraint::Length(10),
            Constraint::Length(10),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

        frame.render_widget(table, inner);

        // Render help text at bottom
        let help_area = Rect {
            x: inner.x,
            y: inner.y + inner.height - 1,
            width: inner.width,
            height: 1,
        };

        let help_text = Paragraph::new("↑↓ Navigate  Enter: Details  c: Cancel  Esc: Close")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(help_text, help_area);
    }
}

/// Formats a status as a styled badge.
fn status_badge(status: &AgentStatus) -> Span<'static> {
    match status {
        AgentStatus::Initializing => Span::styled("○ Init", Style::default().fg(Color::Gray)),
        AgentStatus::Running => Span::styled("● Running", Style::default().fg(Color::Yellow)),
        AgentStatus::Completed => Span::styled("✓ Done", Style::default().fg(Color::Green)),
        AgentStatus::Failed => Span::styled("✗ Failed", Style::default().fg(Color::Red)),
        AgentStatus::Cancelled => Span::styled("○ Cancelled", Style::default().fg(Color::DarkGray)),
        AgentStatus::TimedOut => Span::styled("⏱ Timeout", Style::default().fg(Color::Magenta)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn make_test_agent(id: &str, task: &str, status: AgentStatus) -> RunningAgentInfo {
        RunningAgentInfo {
            id: id.to_string(),
            task: task.to_string(),
            agent_type: "general".to_string(),
            status,
            started_at: Instant::now(),
            tokens_used: 100,
            last_progress: None,
        }
    }

    #[test]
    fn test_tasks_view_empty() {
        let view = TasksView::new();
        assert!(view.is_empty());
        assert_eq!(view.agent_count(), 0);
    }

    #[test]
    fn test_tasks_view_set_agents() {
        let mut view = TasksView::new();
        let agents = vec![
            make_test_agent("bg-1", "Task 1", AgentStatus::Running),
            make_test_agent("bg-2", "Task 2", AgentStatus::Completed),
        ];
        view.set_agents(agents);

        assert!(!view.is_empty());
        assert_eq!(view.agent_count(), 2);
    }

    #[test]
    fn test_tasks_view_navigation() {
        let mut view = TasksView::new();
        let agents = vec![
            make_test_agent("bg-1", "Task 1", AgentStatus::Running),
            make_test_agent("bg-2", "Task 2", AgentStatus::Running),
            make_test_agent("bg-3", "Task 3", AgentStatus::Running),
        ];
        view.set_agents(agents);

        assert_eq!(view.selected, 0);

        view.select_next();
        assert_eq!(view.selected, 1);

        view.select_next();
        assert_eq!(view.selected, 2);

        // Should not go beyond last
        view.select_next();
        assert_eq!(view.selected, 2);

        view.select_previous();
        assert_eq!(view.selected, 1);

        view.select_previous();
        assert_eq!(view.selected, 0);

        // Should not go below 0
        view.select_previous();
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn test_tasks_view_selected_agent() {
        let mut view = TasksView::new();
        let agents = vec![
            make_test_agent("bg-1", "Task 1", AgentStatus::Running),
            make_test_agent("bg-2", "Task 2", AgentStatus::Completed),
        ];
        view.set_agents(agents);

        assert_eq!(view.selected_agent_id(), Some("bg-1"));

        view.select_next();
        assert_eq!(view.selected_agent_id(), Some("bg-2"));
    }

    #[test]
    fn test_format_duration_integration() {
        // Tests now use cortex_common::format_duration
        let short = format_duration(Duration::from_secs(5));
        assert!(short.contains("5"));

        let medium = format_duration(Duration::from_secs(65));
        assert!(medium.contains("1"));

        let long = format_duration(Duration::from_secs(3665));
        assert!(long.contains("1"));
    }

    #[test]
    fn test_truncate_integration() {
        // Tests now use cortex_common::truncate_* functions
        assert_eq!(truncate_first_line("short", 10).as_ref(), "short");
        assert_eq!(truncate_first_line("line1\nline2", 20).as_ref(), "line1");

        assert_eq!(truncate_id_default("bg-1").as_ref(), "bg-1");
    }
}
