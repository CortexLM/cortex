//! Forge validation dashboard view for the TUI.
//!
//! This view provides a dashboard for monitoring Forge orchestration system
//! validation runs, showing agent status, findings, and overall progress.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table};

use cortex_agents::forge::Severity;

/// Display status for an agent in the Forge system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentRunStatus {
    /// Agent hasn't started yet.
    #[default]
    Pending,
    /// Agent is currently running.
    Running,
    /// Agent completed with no issues.
    Passed,
    /// Agent completed with errors.
    Failed,
    /// Agent completed with warnings.
    Warning,
    /// Agent was skipped (dependency failed).
    Skipped,
}

impl AgentRunStatus {
    /// Returns true if this status represents completion.
    pub fn is_complete(&self) -> bool {
        matches!(
            self,
            AgentRunStatus::Passed
                | AgentRunStatus::Failed
                | AgentRunStatus::Warning
                | AgentRunStatus::Skipped
        )
    }

    /// Returns the status label.
    pub fn label(&self) -> &'static str {
        match self {
            AgentRunStatus::Pending => "Pending",
            AgentRunStatus::Running => "Running",
            AgentRunStatus::Passed => "Passed",
            AgentRunStatus::Failed => "Failed",
            AgentRunStatus::Warning => "Warning",
            AgentRunStatus::Skipped => "Skipped",
        }
    }
}

/// Display information for an agent.
#[derive(Debug, Clone)]
pub struct AgentDisplayInfo {
    /// Agent unique identifier.
    pub id: String,
    /// Human-readable agent name.
    pub name: String,
    /// Current execution status.
    pub status: AgentRunStatus,
    /// Number of findings produced by this agent.
    pub findings_count: usize,
    /// Execution duration in milliseconds (if complete).
    pub duration_ms: Option<u64>,
}

impl AgentDisplayInfo {
    /// Create a new agent display info.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status: AgentRunStatus::Pending,
            findings_count: 0,
            duration_ms: None,
        }
    }
}

/// Display representation of a finding.
#[derive(Debug, Clone)]
pub struct FindingDisplay {
    /// ID of the agent that produced this finding.
    pub agent_id: String,
    /// Severity level.
    pub severity: Severity,
    /// Finding message.
    pub message: String,
    /// File location (if applicable).
    pub location: Option<String>,
}

impl FindingDisplay {
    /// Create a new finding display.
    pub fn new(
        agent_id: impl Into<String>,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            severity,
            message: message.into(),
            location: None,
        }
    }

    /// Set the file location.
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }
}

/// Active panel in the Forge view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForgePanel {
    /// Agent list panel.
    #[default]
    Agents,
    /// Findings panel.
    Findings,
}

/// Forge validation dashboard view.
///
/// Displays agent execution status, findings, and overall progress for
/// the Forge orchestration system.
pub struct ForgeView {
    /// List of agents with their display info.
    agents: Vec<AgentDisplayInfo>,
    /// List of findings from all agents.
    findings: Vec<FindingDisplay>,
    /// Currently selected agent index.
    selected_agent: usize,
    /// Currently selected finding index.
    selected_finding: usize,
    /// Scroll offset for findings list.
    scroll_offset: usize,
    /// Overall validation status (None if still running).
    overall_status: Option<AgentRunStatus>,
    /// Whether validation is currently running.
    is_running: bool,
    /// Currently active panel.
    active_panel: ForgePanel,
}

impl Default for ForgeView {
    fn default() -> Self {
        Self::new()
    }
}

impl ForgeView {
    /// Creates a new empty Forge view.
    pub fn new() -> Self {
        Self {
            agents: Vec::new(),
            findings: Vec::new(),
            selected_agent: 0,
            selected_finding: 0,
            scroll_offset: 0,
            overall_status: None,
            is_running: false,
            active_panel: ForgePanel::Agents,
        }
    }

    /// Updates the list of agents.
    pub fn set_agents(&mut self, agents: Vec<AgentDisplayInfo>) {
        self.agents = agents;
        if self.selected_agent >= self.agents.len() && !self.agents.is_empty() {
            self.selected_agent = self.agents.len() - 1;
        }
    }

    /// Adds a finding to the view.
    pub fn add_finding(&mut self, finding: FindingDisplay) {
        self.findings.push(finding);
    }

    /// Clears all findings.
    pub fn clear_findings(&mut self) {
        self.findings.clear();
        self.selected_finding = 0;
        self.scroll_offset = 0;
    }

    /// Updates the status of a specific agent.
    pub fn set_agent_status(
        &mut self,
        agent_id: &str,
        status: AgentRunStatus,
        duration_ms: Option<u64>,
    ) {
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == agent_id) {
            agent.status = status;
            agent.duration_ms = duration_ms;
        }
    }

    /// Increments the findings count for an agent.
    pub fn increment_agent_findings(&mut self, agent_id: &str) {
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == agent_id) {
            agent.findings_count += 1;
        }
    }

    /// Sets the overall validation status.
    pub fn set_overall_status(&mut self, status: AgentRunStatus) {
        self.overall_status = Some(status);
        self.is_running = false;
    }

    /// Sets the running state.
    pub fn set_running(&mut self, running: bool) {
        self.is_running = running;
        if running {
            self.overall_status = None;
        }
    }

    /// Returns whether validation is running.
    pub fn is_running(&self) -> bool {
        self.is_running
    }

    /// Returns the overall status.
    pub fn overall_status(&self) -> Option<AgentRunStatus> {
        self.overall_status
    }

    /// Moves selection to the previous item in the active panel.
    pub fn select_previous(&mut self) {
        match self.active_panel {
            ForgePanel::Agents => self.select_previous_agent(),
            ForgePanel::Findings => self.select_previous_finding(),
        }
    }

    /// Moves selection to the next item in the active panel.
    pub fn select_next(&mut self) {
        match self.active_panel {
            ForgePanel::Agents => self.select_next_agent(),
            ForgePanel::Findings => self.select_next_finding(),
        }
    }

    /// Moves selection to the previous agent.
    pub fn select_previous_agent(&mut self) {
        if self.selected_agent > 0 {
            self.selected_agent -= 1;
        }
    }

    /// Moves selection to the next agent.
    pub fn select_next_agent(&mut self) {
        if self.selected_agent + 1 < self.agents.len() {
            self.selected_agent += 1;
        }
    }

    /// Moves selection to the previous finding.
    fn select_previous_finding(&mut self) {
        if self.selected_finding > 0 {
            self.selected_finding -= 1;
        }
    }

    /// Moves selection to the next finding.
    fn select_next_finding(&mut self) {
        if self.selected_finding + 1 < self.findings.len() {
            self.selected_finding += 1;
        }
    }

    /// Returns the currently selected agent.
    pub fn selected_agent(&self) -> Option<&AgentDisplayInfo> {
        self.agents.get(self.selected_agent)
    }

    /// Returns the currently selected finding.
    pub fn selected_finding(&self) -> Option<&FindingDisplay> {
        self.findings.get(self.selected_finding)
    }

    /// Switches to the next panel.
    pub fn switch_panel(&mut self) {
        self.active_panel = match self.active_panel {
            ForgePanel::Agents => ForgePanel::Findings,
            ForgePanel::Findings => ForgePanel::Agents,
        };
    }

    /// Returns the active panel.
    pub fn active_panel(&self) -> ForgePanel {
        self.active_panel
    }

    /// Returns the number of agents.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Returns the number of findings.
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }

    /// Gets findings filtered by severity.
    pub fn findings_by_severity(&self, severity: Severity) -> Vec<&FindingDisplay> {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .collect()
    }

    /// Counts findings by severity.
    pub fn count_by_severity(&self) -> (usize, usize, usize, usize) {
        let critical = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Critical)
            .count();
        let errors = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count();
        let warnings = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count();
        let infos = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Info)
            .count();
        (critical, errors, warnings, infos)
    }

    /// Calculates the overall progress (0.0 to 1.0).
    fn calculate_progress(&self) -> f64 {
        if self.agents.is_empty() {
            return 0.0;
        }
        let completed = self
            .agents
            .iter()
            .filter(|a| a.status.is_complete())
            .count();
        completed as f64 / self.agents.len() as f64
    }

    /// Renders the Forge view.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Clear the area
        frame.render_widget(Clear, area);

        // Create outer block
        let block = Block::default()
            .title(" Forge Validation ")
            .title_style(Style::default().fg(Color::Magenta).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Layout: progress bar at top, then agents table, then findings panel, then help
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),      // Progress bar
                Constraint::Percentage(40), // Agents table
                Constraint::Min(5),         // Findings panel
                Constraint::Length(2),      // Help text
            ])
            .split(inner);

        self.render_progress_bar(frame, chunks[0]);
        self.render_agents_table(frame, chunks[1]);
        self.render_findings_panel(frame, chunks[2]);
        self.render_help_text(frame, chunks[3]);
    }

    /// Renders the progress bar.
    fn render_progress_bar(&self, frame: &mut Frame, area: Rect) {
        let progress = self.calculate_progress();
        let (critical, errors, warnings, infos) = self.count_by_severity();

        let status_text = if self.is_running {
            format!(
                "Running... ({:.0}%) | {} agents",
                progress * 100.0,
                self.agents.len()
            )
        } else if let Some(status) = self.overall_status {
            let status_str = match status {
                AgentRunStatus::Passed => "✓ Passed",
                AgentRunStatus::Failed => "✗ Failed",
                AgentRunStatus::Warning => "⚠ Warnings",
                _ => "Complete",
            };
            format!(
                "{} | Critical: {} | Errors: {} | Warnings: {} | Info: {}",
                status_str, critical, errors, warnings, infos
            )
        } else {
            "Ready".to_string()
        };

        let gauge_color = if self.is_running {
            Color::Yellow
        } else {
            match self.overall_status {
                Some(AgentRunStatus::Passed) => Color::Green,
                Some(AgentRunStatus::Failed) => Color::Red,
                Some(AgentRunStatus::Warning) => Color::Yellow,
                _ => Color::Gray,
            }
        };

        let gauge = Gauge::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(" Progress "),
            )
            .gauge_style(Style::default().fg(gauge_color))
            .ratio(progress)
            .label(status_text);

        frame.render_widget(gauge, area);
    }

    /// Renders the agents table.
    fn render_agents_table(&self, frame: &mut Frame, area: Rect) {
        let is_active = self.active_panel == ForgePanel::Agents;
        let border_color = if is_active {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .title(" Agents ")
            .title_style(Style::default().fg(Color::Cyan).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.agents.is_empty() {
            let empty_text = Paragraph::new("No agents configured.")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(empty_text, inner);
            return;
        }

        // Create table header
        let header = Row::new(vec![
            Cell::from("Agent").style(Style::default().fg(Color::Yellow).bold()),
            Cell::from("Status").style(Style::default().fg(Color::Yellow).bold()),
            Cell::from("Findings").style(Style::default().fg(Color::Yellow).bold()),
            Cell::from("Duration").style(Style::default().fg(Color::Yellow).bold()),
        ])
        .height(1)
        .bottom_margin(1);

        // Create table rows
        let rows: Vec<Row> = self
            .agents
            .iter()
            .enumerate()
            .map(|(idx, agent)| {
                let is_selected = idx == self.selected_agent && is_active;
                let style = if is_selected {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };

                let duration_str = agent
                    .duration_ms
                    .map(format_duration_ms)
                    .unwrap_or_else(|| "-".to_string());

                Row::new(vec![
                    Cell::from(agent.name.clone()).style(style),
                    Cell::from(status_badge(agent.status)),
                    Cell::from(format!("{}", agent.findings_count)).style(style),
                    Cell::from(duration_str).style(style),
                ])
                .style(style)
            })
            .collect();

        // Define column widths
        let widths = [
            Constraint::Percentage(40),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(12),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

        frame.render_widget(table, inner);
    }

    /// Renders the findings panel.
    fn render_findings_panel(&mut self, frame: &mut Frame, area: Rect) {
        let is_active = self.active_panel == ForgePanel::Findings;
        let border_color = if is_active {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .title(" Findings ")
            .title_style(Style::default().fg(Color::Cyan).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.findings.is_empty() {
            let empty_text = Paragraph::new("No findings yet.")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(empty_text, inner);
            return;
        }

        // Calculate visible rows
        let visible_rows = inner.height as usize;

        // Adjust scroll offset to keep selection visible and persist it
        self.scroll_offset = if self.selected_finding < self.scroll_offset {
            self.selected_finding
        } else if visible_rows > 0 && self.selected_finding >= self.scroll_offset + visible_rows {
            self.selected_finding
                .saturating_sub(visible_rows.saturating_sub(1))
        } else {
            self.scroll_offset
        };

        // Build findings text with highlighting
        let mut lines: Vec<Line> = Vec::new();

        for (idx, finding) in self
            .findings
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(visible_rows)
        {
            let is_selected = idx == self.selected_finding && is_active;
            let bg_style = if is_selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            let severity_span = severity_badge(finding.severity);
            let agent_span = Span::styled(
                format!("[{}] ", finding.agent_id),
                Style::default().fg(Color::DarkGray),
            );
            let message_span = Span::styled(&finding.message, bg_style);

            let mut spans = vec![severity_span, Span::raw(" "), agent_span, message_span];

            if let Some(ref loc) = finding.location {
                spans.push(Span::styled(
                    format!(" @ {}", loc),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            lines.push(Line::from(spans));
        }

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, inner);
    }

    /// Renders the help text.
    fn render_help_text(&self, frame: &mut Frame, area: Rect) {
        let help_text =
            Paragraph::new("↑↓ Navigate  Tab: Switch panel  Enter: Details  Esc: Close")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
        frame.render_widget(help_text, area);
    }
}

/// Formats a status as a styled badge.
fn status_badge(status: AgentRunStatus) -> Span<'static> {
    match status {
        AgentRunStatus::Pending => Span::styled("○ Pending", Style::default().fg(Color::Gray)),
        AgentRunStatus::Running => Span::styled("● Running", Style::default().fg(Color::Yellow)),
        AgentRunStatus::Passed => Span::styled("✓ Passed", Style::default().fg(Color::Green)),
        AgentRunStatus::Failed => Span::styled("✗ Failed", Style::default().fg(Color::Red)),
        AgentRunStatus::Warning => Span::styled("⚠ Warning", Style::default().fg(Color::Yellow)),
        AgentRunStatus::Skipped => Span::styled("○ Skipped", Style::default().fg(Color::DarkGray)),
    }
}

/// Formats a severity as a styled badge.
fn severity_badge(severity: Severity) -> Span<'static> {
    match severity {
        Severity::Critical => Span::styled("🚨 CRIT", Style::default().fg(Color::Red).bold()),
        Severity::Error => Span::styled("❌ ERR ", Style::default().fg(Color::Red)),
        Severity::Warning => Span::styled("⚠️  WARN", Style::default().fg(Color::Yellow)),
        Severity::Info => Span::styled("ℹ️  INFO", Style::default().fg(Color::Cyan)),
    }
}

/// Formats duration in milliseconds to a human-readable string.
fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let secs = ms / 1000;
        let mins = secs / 60;
        let remaining_secs = secs % 60;
        format!("{}m {}s", mins, remaining_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_forge_view_empty() {
        let view = ForgeView::new();
        assert_eq!(view.agent_count(), 0);
        assert_eq!(view.finding_count(), 0);
        assert!(!view.is_running());
        assert!(view.overall_status().is_none());
    }

    #[test]
    fn test_forge_view_set_agents() {
        let mut view = ForgeView::new();
        let agents = vec![
            AgentDisplayInfo::new("security", "Security Scanner"),
            AgentDisplayInfo::new("quality", "Quality Checker"),
        ];
        view.set_agents(agents);

        assert_eq!(view.agent_count(), 2);
        assert_eq!(
            view.selected_agent().map(|a| a.id.as_str()),
            Some("security")
        );
    }

    #[test]
    fn test_forge_view_navigation() {
        let mut view = ForgeView::new();
        let agents = vec![
            AgentDisplayInfo::new("agent-1", "Agent 1"),
            AgentDisplayInfo::new("agent-2", "Agent 2"),
            AgentDisplayInfo::new("agent-3", "Agent 3"),
        ];
        view.set_agents(agents);

        assert_eq!(view.selected_agent, 0);

        view.select_next_agent();
        assert_eq!(view.selected_agent, 1);

        view.select_next_agent();
        assert_eq!(view.selected_agent, 2);

        // Should not go beyond last
        view.select_next_agent();
        assert_eq!(view.selected_agent, 2);

        view.select_previous_agent();
        assert_eq!(view.selected_agent, 1);

        view.select_previous_agent();
        assert_eq!(view.selected_agent, 0);

        // Should not go below 0
        view.select_previous_agent();
        assert_eq!(view.selected_agent, 0);
    }

    #[test]
    fn test_agent_run_status() {
        assert!(!AgentRunStatus::Pending.is_complete());
        assert!(!AgentRunStatus::Running.is_complete());
        assert!(AgentRunStatus::Passed.is_complete());
        assert!(AgentRunStatus::Failed.is_complete());
        assert!(AgentRunStatus::Warning.is_complete());
        assert!(AgentRunStatus::Skipped.is_complete());
    }

    #[test]
    fn test_forge_view_add_finding() {
        let mut view = ForgeView::new();
        view.add_finding(FindingDisplay::new(
            "security",
            Severity::Error,
            "Security issue",
        ));
        view.add_finding(FindingDisplay::new(
            "quality",
            Severity::Warning,
            "Code smell",
        ));

        assert_eq!(view.finding_count(), 2);
        let (critical, errors, warnings, infos) = view.count_by_severity();
        assert_eq!(critical, 0);
        assert_eq!(errors, 1);
        assert_eq!(warnings, 1);
        assert_eq!(infos, 0);
    }

    #[test]
    fn test_forge_view_set_agent_status() {
        let mut view = ForgeView::new();
        let agents = vec![AgentDisplayInfo::new("test", "Test Agent")];
        view.set_agents(agents);

        view.set_agent_status("test", AgentRunStatus::Running, None);
        assert_eq!(
            view.selected_agent().map(|a| a.status),
            Some(AgentRunStatus::Running)
        );

        view.set_agent_status("test", AgentRunStatus::Passed, Some(1500));
        assert_eq!(
            view.selected_agent().map(|a| a.status),
            Some(AgentRunStatus::Passed)
        );
        assert_eq!(
            view.selected_agent().and_then(|a| a.duration_ms),
            Some(1500)
        );
    }

    #[test]
    fn test_forge_view_overall_status() {
        let mut view = ForgeView::new();
        view.set_running(true);
        assert!(view.is_running());
        assert!(view.overall_status().is_none());

        view.set_overall_status(AgentRunStatus::Passed);
        assert!(!view.is_running());
        assert_eq!(view.overall_status(), Some(AgentRunStatus::Passed));
    }

    #[test]
    fn test_forge_view_panel_switching() {
        let mut view = ForgeView::new();
        assert_eq!(view.active_panel(), ForgePanel::Agents);

        view.switch_panel();
        assert_eq!(view.active_panel(), ForgePanel::Findings);

        view.switch_panel();
        assert_eq!(view.active_panel(), ForgePanel::Agents);
    }

    #[test]
    fn test_format_duration_ms() {
        assert_eq!(format_duration_ms(500), "500ms");
        assert_eq!(format_duration_ms(1500), "1.5s");
        assert_eq!(format_duration_ms(60000), "1m 0s");
        assert_eq!(format_duration_ms(90000), "1m 30s");
    }

    #[test]
    fn test_finding_display_with_location() {
        let finding = FindingDisplay::new("test", Severity::Error, "Test message")
            .with_location("src/lib.rs:42");

        assert_eq!(finding.agent_id, "test");
        assert_eq!(finding.severity, Severity::Error);
        assert_eq!(finding.message, "Test message");
        assert_eq!(finding.location, Some("src/lib.rs:42".to_string()));
    }
}
