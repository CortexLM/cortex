//! Task progress widget for displaying real-time progress in the TUI.
//!
//! This widget provides a comprehensive display of task progress including:
//! - Current task status with elapsed time
//! - Active tool calls with live updates
//! - Todo list with status indicators
//! - Progress bar for percentage-based operations
//!
//! ## Example
//!
//! ```ignore
//! use cortex_tui::widgets::TaskProgressWidget;
//! use cortex_core::progress::{ProgressCollector, ProgressSubscriber};
//!
//! let collector = ProgressCollector::new(subscriber);
//! let widget = TaskProgressWidget::from(&collector);
//! frame.render_widget(widget, area);
//! ```

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Widget};

use cortex_core::progress::{ProgressEvent, ProgressSubscriber, TodoItem, TodoStatus};

use crate::ui::colors::AdaptiveColors;
use crate::ui::consts::STREAMING_SPINNER_FRAMES;
use crate::ui::shimmer::elapsed_since_start;

// ============================================================
// TASK PROGRESS STATE
// ============================================================

/// State for a single active task.
#[derive(Clone, Debug)]
pub struct TaskProgress {
    /// Unique task identifier.
    pub id: String,
    /// Human-readable task description.
    pub description: String,
    /// Current status of the task.
    pub status: TaskStatus,
    /// When the task started.
    pub started_at: Instant,
    /// Elapsed time in seconds (cached for display).
    pub elapsed_secs: u64,
}

impl TaskProgress {
    /// Creates a new task progress entry.
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            status: TaskStatus::Running,
            started_at: Instant::now(),
            elapsed_secs: 0,
        }
    }

    /// Updates the cached elapsed time.
    pub fn update_elapsed(&mut self) {
        self.elapsed_secs = self.started_at.elapsed().as_secs();
    }

    /// Returns a formatted elapsed time string.
    pub fn elapsed_string(&self) -> String {
        format_duration(Duration::from_secs(self.elapsed_secs))
    }
}

/// Status of a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    /// Task is currently running.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed with an error.
    Failed,
}

// ============================================================
// CURRENT TOOL STATE
// ============================================================

/// State for the currently executing tool.
#[derive(Clone, Debug)]
pub struct CurrentTool {
    /// Name of the tool being executed.
    pub name: String,
    /// Preview of the arguments (truncated).
    pub arguments: String,
    /// When the tool call started.
    pub started_at: Instant,
}

impl CurrentTool {
    /// Creates a new current tool entry.
    pub fn new(name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: arguments.into(),
            started_at: Instant::now(),
        }
    }

    /// Returns the elapsed time since the tool started.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

// ============================================================
// TOOL CALL SUMMARY
// ============================================================

/// Summary of a completed tool call.
#[derive(Clone, Debug)]
pub struct ToolCallSummary {
    /// Name of the tool.
    pub name: String,
    /// Whether the tool call succeeded.
    pub success: bool,
    /// How long the tool took to execute.
    pub duration_ms: u64,
}

// ============================================================
// PROGRESS COLLECTOR
// ============================================================

/// Collects and manages progress state from a subscriber.
///
/// The collector maintains the current state of all active tasks, tool calls,
/// and todo items. It processes events from a [`ProgressSubscriber`] and
/// provides a snapshot of the current state for rendering.
pub struct ProgressCollector {
    subscriber: ProgressSubscriber,
    state: ProgressState,
}

/// Internal state tracked by the collector.
#[derive(Default)]
pub struct ProgressState {
    /// Currently active tasks.
    pub active_tasks: HashMap<String, TaskProgress>,
    /// Currently executing tool (if any).
    pub current_tool: Option<CurrentTool>,
    /// Current todo list.
    pub todos: Vec<TodoItem>,
    /// Recent tool call history (limited to last 10).
    pub tool_history: VecDeque<ToolCallSummary>,
    /// Current progress percentage (if applicable).
    pub progress_percent: Option<u8>,
    /// Progress message (if applicable).
    pub progress_message: Option<String>,
}

impl ProgressCollector {
    /// Creates a new progress collector from a subscriber.
    pub fn new(subscriber: ProgressSubscriber) -> Self {
        Self {
            subscriber,
            state: ProgressState::default(),
        }
    }

    /// Processes all pending events and updates the state.
    ///
    /// Call this method on each frame tick to keep the state up-to-date.
    pub fn poll(&mut self) {
        while let Some(event) = self.subscriber.try_recv() {
            self.handle_event(event);
        }

        // Update elapsed times for all active tasks
        for task in self.state.active_tasks.values_mut() {
            task.update_elapsed();
        }
    }

    /// Handles a single progress event.
    fn handle_event(&mut self, event: ProgressEvent) {
        match event {
            ProgressEvent::TaskStarted {
                task_id,
                description,
            } => {
                self.state
                    .active_tasks
                    .insert(task_id.clone(), TaskProgress::new(task_id, description));
            }

            ProgressEvent::ToolCallStarted {
                tool_name,
                arguments,
                ..
            } => {
                let args_preview: String = serde_json::to_string(&arguments)
                    .unwrap_or_default()
                    .chars()
                    .take(50)
                    .collect();
                self.state.current_tool = Some(CurrentTool::new(tool_name, args_preview));
            }

            ProgressEvent::ToolCallCompleted {
                tool_name,
                result,
                duration_ms,
                ..
            } => {
                self.state.current_tool = None;
                self.state.tool_history.push_back(ToolCallSummary {
                    name: tool_name,
                    success: result.success,
                    duration_ms,
                });

                // Keep only last 10 tool calls
                while self.state.tool_history.len() > 10 {
                    self.state.tool_history.pop_front();
                }
            }

            ProgressEvent::TodoUpdated { todos, .. } => {
                self.state.todos = todos;
            }

            ProgressEvent::TaskCompleted { task_id, .. } => {
                if let Some(task) = self.state.active_tasks.get_mut(&task_id) {
                    task.status = TaskStatus::Completed;
                }
                // Remove completed tasks after a short delay (handled elsewhere)
                self.state.active_tasks.remove(&task_id);
            }

            ProgressEvent::TaskError { task_id, .. } => {
                if let Some(task) = self.state.active_tasks.get_mut(&task_id) {
                    task.status = TaskStatus::Failed;
                }
            }

            ProgressEvent::ProgressUpdate {
                percent, message, ..
            } => {
                self.state.progress_percent = Some(percent);
                self.state.progress_message = message;
            }

            ProgressEvent::ThinkingStarted { .. } | ProgressEvent::TokenGenerated { .. } => {
                // These events are handled by the streaming indicator
            }
        }
    }

    /// Returns a reference to the current state.
    pub fn state(&self) -> &ProgressState {
        &self.state
    }

    /// Creates a widget from the current state.
    pub fn widget(&self) -> TaskProgressWidget<'_> {
        TaskProgressWidget::new(&self.state)
    }

    /// Returns whether there are any active tasks.
    pub fn has_active_tasks(&self) -> bool {
        !self.state.active_tasks.is_empty()
    }

    /// Returns whether there is an active tool call.
    pub fn has_active_tool(&self) -> bool {
        self.state.current_tool.is_some()
    }

    /// Returns the current todo list.
    pub fn todos(&self) -> &[TodoItem] {
        &self.state.todos
    }
}

// ============================================================
// TASK PROGRESS WIDGET
// ============================================================

/// Widget for displaying task progress in the TUI.
///
/// Renders a panel showing:
/// - Current task with elapsed time
/// - Active tool call with spinner
/// - Todo list with status icons
/// - Optional progress bar
pub struct TaskProgressWidget<'a> {
    state: &'a ProgressState,
    /// Whether to show the tool history.
    show_history: bool,
    /// Maximum number of todos to show.
    max_todos: usize,
}

impl<'a> TaskProgressWidget<'a> {
    /// Creates a new task progress widget.
    pub fn new(state: &'a ProgressState) -> Self {
        Self {
            state,
            show_history: false,
            max_todos: 10,
        }
    }

    /// Enables showing tool call history.
    pub fn with_history(mut self, show: bool) -> Self {
        self.show_history = show;
        self
    }

    /// Sets the maximum number of todos to display.
    pub fn with_max_todos(mut self, max: usize) -> Self {
        self.max_todos = max;
        self
    }

    /// Renders the current task section.
    fn render_current_task(&self, area: Rect, buf: &mut Buffer, colors: &AdaptiveColors) {
        if area.is_empty() {
            return;
        }

        let block = Block::default()
            .title(Span::styled(
                " Current Task ",
                Style::default().fg(colors.accent).bold(),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(colors.border));

        let inner = block.inner(area);
        block.render(area, buf);

        if let Some(task) = self.state.active_tasks.values().next() {
            let spinner = get_spinner_frame();
            let elapsed = task.elapsed_string();

            let lines = vec![
                Line::from(vec![
                    Span::styled(format!("{} ", spinner), Style::default().fg(colors.accent)),
                    Span::raw(&task.description),
                ]),
                Line::from(vec![Span::styled(
                    format!("  {} elapsed", elapsed),
                    Style::default().fg(colors.text_muted),
                )]),
            ];

            Paragraph::new(lines).render(inner, buf);
        } else {
            let line = Line::from(Span::styled(
                "No active tasks",
                Style::default().fg(colors.text_muted),
            ));
            Paragraph::new(line).render(inner, buf);
        }
    }

    /// Renders the current tool call section.
    fn render_current_tool(&self, area: Rect, buf: &mut Buffer, colors: &AdaptiveColors) {
        if area.is_empty() {
            return;
        }

        let block = Block::default()
            .title(Span::styled(
                " Tool Calls ",
                Style::default().fg(colors.success).bold(),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(colors.border));

        let inner = block.inner(area);
        block.render(area, buf);

        if let Some(tool) = &self.state.current_tool {
            let spinner = get_spinner_frame();
            let elapsed = format_duration(tool.elapsed());

            let lines = vec![
                Line::from(vec![
                    Span::styled(format!("{} ", spinner), Style::default().fg(colors.accent)),
                    Span::styled(
                        &tool.name,
                        Style::default()
                            .fg(colors.text)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![Span::styled(
                    format!("  {}", truncate(&tool.arguments, 50)),
                    Style::default().fg(colors.text_muted),
                )]),
                Line::from(vec![Span::styled(
                    format!("  Running for {}...", elapsed),
                    Style::default().fg(colors.warning),
                )]),
            ];

            Paragraph::new(lines).render(inner, buf);
        } else {
            let line = Line::from(Span::styled(
                "No active tool calls",
                Style::default().fg(colors.text_muted),
            ));
            Paragraph::new(line).render(inner, buf);
        }
    }

    /// Renders the todo list section.
    fn render_todos(&self, area: Rect, buf: &mut Buffer, colors: &AdaptiveColors) {
        if area.is_empty() {
            return;
        }

        let block = Block::default()
            .title(Span::styled(
                " Todo List ",
                Style::default().fg(colors.accent).bold(),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(colors.border));

        let inner = block.inner(area);
        block.render(area, buf);

        if self.state.todos.is_empty() {
            let line = Line::from(Span::styled(
                "No todos",
                Style::default().fg(colors.text_muted),
            ));
            Paragraph::new(line).render(inner, buf);
            return;
        }

        let items: Vec<ListItem> = self
            .state
            .todos
            .iter()
            .take(self.max_todos)
            .map(|todo| {
                let (icon, style) = match todo.status {
                    TodoStatus::Pending => ("○", Style::default().fg(colors.text_muted)),
                    TodoStatus::InProgress => ("◐", Style::default().fg(colors.warning)),
                    TodoStatus::Completed => ("●", Style::default().fg(colors.success)),
                };

                ListItem::new(Line::from(vec![
                    Span::styled(format!("{} ", icon), style),
                    Span::raw(&todo.text),
                ]))
            })
            .collect();

        let list = List::new(items);
        list.render(inner, buf);
    }

    /// Renders the progress bar if applicable.
    fn render_progress_bar(&self, area: Rect, buf: &mut Buffer, colors: &AdaptiveColors) {
        if area.is_empty() {
            return;
        }

        if let Some(percent) = self.state.progress_percent {
            let block = Block::default()
                .title(Span::styled(
                    " Progress ",
                    Style::default().fg(colors.accent).bold(),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(colors.border));

            let inner = block.inner(area);
            block.render(area, buf);

            // Calculate progress bar width
            let bar_width = inner.width.saturating_sub(10) as usize; // Leave room for percentage
            let filled = (bar_width * percent as usize) / 100;
            let empty = bar_width.saturating_sub(filled);

            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

            let line = Line::from(vec![
                Span::styled(bar, Style::default().fg(colors.accent)),
                Span::styled(format!(" {:3}%", percent), Style::default().fg(colors.text)),
            ]);

            if let Some(ref msg) = self.state.progress_message {
                let lines = vec![
                    line,
                    Line::from(Span::styled(
                        msg.as_str(),
                        Style::default().fg(colors.text_muted),
                    )),
                ];
                Paragraph::new(lines).render(inner, buf);
            } else {
                Paragraph::new(line).render(inner, buf);
            }
        }
    }
}

impl Widget for TaskProgressWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let colors = AdaptiveColors::default();

        // Calculate layout based on what we have to show
        let has_progress = self.state.progress_percent.is_some();
        let has_todos = !self.state.todos.is_empty();

        let constraints = if has_progress {
            vec![
                Constraint::Length(4), // Current task
                Constraint::Length(5), // Current tool
                Constraint::Length(3), // Progress bar
                Constraint::Min(5),    // Todo list
            ]
        } else if has_todos {
            vec![
                Constraint::Length(4), // Current task
                Constraint::Length(5), // Current tool
                Constraint::Min(5),    // Todo list
            ]
        } else {
            vec![
                Constraint::Length(4), // Current task
                Constraint::Min(5),    // Current tool
            ]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        // Render sections
        let mut idx = 0;

        // Current task
        self.render_current_task(chunks[idx], buf, &colors);
        idx += 1;

        // Current tool
        self.render_current_tool(chunks[idx], buf, &colors);
        idx += 1;

        // Progress bar (if applicable)
        if has_progress {
            self.render_progress_bar(chunks[idx], buf, &colors);
            idx += 1;
        }

        // Todo list (if applicable)
        if has_todos && idx < chunks.len() {
            self.render_todos(chunks[idx], buf, &colors);
        }
    }
}

// ============================================================
// COMPACT PROGRESS INDICATOR
// ============================================================

/// A compact, single-line progress indicator.
///
/// Shows a spinner with the current operation and elapsed time,
/// suitable for display in a status bar or header area.
pub struct CompactProgressIndicator<'a> {
    state: &'a ProgressState,
}

impl<'a> CompactProgressIndicator<'a> {
    /// Creates a new compact progress indicator.
    pub fn new(state: &'a ProgressState) -> Self {
        Self { state }
    }
}

impl Widget for CompactProgressIndicator<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() || area.height == 0 {
            return;
        }

        let colors = AdaptiveColors::default();
        let spinner = get_spinner_frame();

        let content = if let Some(tool) = &self.state.current_tool {
            let elapsed = format_duration(tool.elapsed());
            format!("{} Running {}... ({})", spinner, tool.name, elapsed)
        } else if let Some(task) = self.state.active_tasks.values().next() {
            let elapsed = task.elapsed_string();
            format!(
                "{} {} ({})",
                spinner,
                truncate(&task.description, 40),
                elapsed
            )
        } else if let Some(percent) = self.state.progress_percent {
            format!("{} Progress: {}%", spinner, percent)
        } else {
            return; // Nothing to show
        };

        let line = Line::from(vec![Span::styled(
            content,
            Style::default().fg(colors.text),
        )]);

        Paragraph::new(line).render(area, buf);
    }
}

// ============================================================
// PARALLEL TASK PROGRESS WIDGET
// ============================================================

/// Widget for displaying multiple parallel task progresses.
///
/// Handles the case where multiple tasks are running simultaneously,
/// preventing overflow and garbled output by:
/// - Limiting the number of visible tasks
/// - Using proper vertical spacing
/// - Coordinating spinner animations
pub struct ParallelTaskProgressWidget<'a> {
    state: &'a ProgressState,
    /// Maximum number of parallel tasks to show.
    max_visible: usize,
}

impl<'a> ParallelTaskProgressWidget<'a> {
    /// Creates a new parallel task progress widget.
    pub fn new(state: &'a ProgressState) -> Self {
        Self {
            state,
            max_visible: 5,
        }
    }

    /// Sets the maximum number of visible tasks.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self
    }
}

impl Widget for ParallelTaskProgressWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let colors = AdaptiveColors::default();

        // Calculate how many tasks to show
        let task_count = self.state.active_tasks.len();
        let visible_count = task_count.min(self.max_visible);

        if visible_count == 0 {
            return;
        }

        // Each task gets 2 lines (description + status)
        let task_height = 2_u16;
        let available_height = area.height;
        let tasks_that_fit = (available_height / task_height) as usize;
        let show_count = visible_count.min(tasks_that_fit);

        // Create layout for each task
        let constraints: Vec<Constraint> = (0..show_count)
            .map(|_| Constraint::Length(task_height))
            .collect();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        // Render each task
        let spinner = get_spinner_frame();
        for (i, task) in self
            .state
            .active_tasks
            .values()
            .take(show_count)
            .enumerate()
        {
            if i >= chunks.len() {
                break;
            }

            let chunk = chunks[i];
            let elapsed = task.elapsed_string();

            // First line: spinner + description
            let line1 = Line::from(vec![
                Span::styled(format!("{} ", spinner), Style::default().fg(colors.accent)),
                Span::raw(truncate(&task.description, chunk.width as usize - 4)),
            ]);
            buf.set_line(chunk.x, chunk.y, &line1, chunk.width);

            // Second line: elapsed time
            if chunk.height > 1 {
                let line2 = Line::from(vec![Span::styled(
                    format!("  {} elapsed", elapsed),
                    Style::default().fg(colors.text_muted),
                )]);
                buf.set_line(chunk.x, chunk.y + 1, &line2, chunk.width);
            }
        }

        // Show overflow indicator if there are more tasks
        if task_count > show_count {
            let overflow_msg = format!("  +{} more tasks...", task_count - show_count);
            let y = area.y + (show_count as u16 * task_height).min(area.height.saturating_sub(1));
            if y < area.y + area.height {
                let line = Line::from(Span::styled(
                    overflow_msg,
                    Style::default().fg(colors.text_muted),
                ));
                buf.set_line(area.x, y, &line, area.width);
            }
        }
    }
}

// ============================================================
// HELPER FUNCTIONS
// ============================================================

/// Formats a duration into a human-readable string.
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m {}s", secs / 3600, (secs % 3600) / 60, secs % 60)
    }
}

/// Truncates a string to the given length, adding "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        format!("{}...", truncated)
    }
}

/// Gets the current spinner frame based on elapsed time.
fn get_spinner_frame() -> char {
    let elapsed_ms = elapsed_since_start().as_millis() as u64;
    let frame_index = (elapsed_ms / 80) as usize % STREAMING_SPINNER_FRAMES.len();
    STREAMING_SPINNER_FRAMES[frame_index]
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(Duration::from_secs(60)), "1m 0s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3599)), "59m 59s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h 0m 0s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m 1s");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello...");
        assert_eq!(truncate("hi", 2), "hi");
    }

    #[test]
    fn test_task_progress_new() {
        let task = TaskProgress::new("task-1", "Test task");
        assert_eq!(task.id, "task-1");
        assert_eq!(task.description, "Test task");
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn test_current_tool_new() {
        let tool = CurrentTool::new("Read", "file: test.rs");
        assert_eq!(tool.name, "Read");
        assert_eq!(tool.arguments, "file: test.rs");
    }

    #[test]
    fn test_progress_state_default() {
        let state = ProgressState::default();
        assert!(state.active_tasks.is_empty());
        assert!(state.current_tool.is_none());
        assert!(state.todos.is_empty());
        assert!(state.tool_history.is_empty());
        assert!(state.progress_percent.is_none());
    }

    #[test]
    fn test_widget_render_empty() {
        let state = ProgressState::default();
        let widget = TaskProgressWidget::new(&state);

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        widget.render(Rect::new(0, 0, 80, 20), &mut buf);

        // Should not panic on empty state
    }

    #[test]
    fn test_compact_indicator_render() {
        let state = ProgressState::default();
        let widget = CompactProgressIndicator::new(&state);

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        widget.render(Rect::new(0, 0, 80, 1), &mut buf);

        // Should not panic on empty state
    }
}
