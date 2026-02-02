//! Main MinimalSessionView struct and Widget implementation.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use cortex_core::widgets::Message;

use crate::app::AppState;
use crate::ui::colors::AdaptiveColors;
use crate::ui::consts::{CURSOR_BLINK_INTERVAL_MS, border};
use crate::widgets::{HintContext, KeyHints, StatusIndicator};

use super::layout::LayoutManager;
use super::rendering::{
    _render_motd, generate_message_lines, generate_welcome_lines, render_message,
    render_scroll_to_bottom_hint, render_scrollbar, render_subagent, render_tool_call,
};

// Re-export for convenience
pub use cortex_core::widgets::Message as ChatMessage;

/// Minimalist session view for the chat interface.
///
/// Layout:
/// ```text
/// ┌─────────────────────────────────────────────────────────┐
/// │ > You: Hello, how are you?                              │
/// │                                                         │
/// │ Assistant: I'm doing well! How can I help you today?    │
/// │                                                         │
/// │ ⠹ Working · Analyzing code... (12s • Esc to interrupt)  │
/// ├─────────────────────────────────────────────────────────┤
/// │ > _                                                     │
/// ├─────────────────────────────────────────────────────────┤
/// │ Enter submit · Ctrl+K palette · Ctrl+M model · ? help   │
/// └─────────────────────────────────────────────────────────┘
/// ```
pub struct MinimalSessionView<'a> {
    /// Reference to the application state
    app_state: &'a AppState,
    /// Color palette
    colors: AdaptiveColors,
}

impl<'a> MinimalSessionView<'a> {
    /// Creates a new minimal session view.
    pub fn new(app_state: &'a AppState) -> Self {
        Self {
            app_state,
            colors: app_state.adaptive_colors(),
        }
    }

    /// Renders a single message to lines.
    fn render_message(&self, msg: &Message, width: u16) -> Vec<Line<'static>> {
        render_message(msg, width, &self.colors)
    }

    /// Renders a single tool call with status indicator
    fn _render_tool_call(
        &self,
        call: &crate::views::tool_call::ToolCallDisplay,
        width: u16,
    ) -> Vec<Line<'static>> {
        render_tool_call(call, width, &self.colors)
    }

    /// Renders a subagent task with todos in Factory-style format
    fn _render_subagent(
        &self,
        task: &crate::app::SubagentTaskDisplay,
        width: u16,
    ) -> Vec<Line<'static>> {
        render_subagent(task, width, &self.colors)
    }

    /// Renders the chat area with welcome cards as part of scrollable content.
    fn _render_chat_with_welcome(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        // Welcome card heights: 1 (top margin) + 11 (welcome card) + 1 (gap) + 5 (info cards) = 18
        let welcome_height = 18_u16;

        // Calculate total content height: welcome cards + messages
        let has_messages =
            !self.app_state.messages.is_empty() || self.app_state.streaming.is_streaming;

        if !has_messages {
            // Only welcome cards, render them at top with 1 line margin
            let welcome_area = Rect::new(
                area.x,
                area.y + 1,
                area.width,
                welcome_height.min(area.height.saturating_sub(1)),
            );
            _render_motd(welcome_area, buf, &self.colors, self.app_state);
            return;
        }

        // We have messages - render welcome cards first, then messages below
        let scroll_offset = self.app_state.chat_scroll;

        // If scrolled past welcome cards, only show messages
        if scroll_offset > 0 {
            // Render only messages (welcome cards scrolled out of view)
            self._render_messages_only(area, buf);
        } else {
            // Show welcome cards at top, messages below
            let welcome_area = Rect::new(
                area.x,
                area.y + 1,
                area.width,
                welcome_height.min(area.height.saturating_sub(1)),
            );
            _render_motd(welcome_area, buf, &self.colors, self.app_state);

            // Render messages below welcome cards
            let messages_y = area.y + 1 + welcome_height + 1; // 1 margin + welcome + 1 gap
            if messages_y < area.y + area.height {
                let messages_area = Rect::new(
                    area.x,
                    messages_y,
                    area.width,
                    area.height.saturating_sub(welcome_height + 2),
                );
                self._render_messages_only(messages_area, buf);
            }
        }
    }

    /// Renders all scrollable content (welcome cards + messages) as unified lines.
    /// Returns the actual content height rendered (for dynamic input positioning).
    fn render_scrollable_content(&self, area: Rect, buf: &mut Buffer, _welcome_height: u16) -> u16 {
        if area.is_empty() || area.height == 0 {
            return 0;
        }

        let mut all_lines: Vec<Line<'static>> = Vec::new();

        // 1. Generate welcome card lines (same visual style as render_motd)
        all_lines.extend(generate_welcome_lines(
            area.width,
            &self.colors,
            self.app_state,
        ));

        // 2. Gap after welcome
        all_lines.push(Line::from(""));
        all_lines.push(Line::from(""));

        // 3. Generate message lines
        all_lines.extend(generate_message_lines(
            area.width,
            &self.colors,
            self.app_state,
        ));

        let total_lines = all_lines.len();
        let visible_lines = area.height as usize;

        // Calculate scroll bounds
        let max_scroll = total_lines.saturating_sub(visible_lines);
        let scroll_offset = self.app_state.chat_scroll.min(max_scroll);

        // Calculate visible window
        let start = if total_lines > visible_lines {
            total_lines - visible_lines - scroll_offset
        } else {
            0
        };
        let end = (start + visible_lines).min(total_lines);

        // Render the visible portion
        let visible: Vec<Line<'static>> = all_lines[start..end].to_vec();
        let paragraph = Paragraph::new(visible);
        paragraph.render(area, buf);

        // Render scrollbar if needed
        if total_lines > visible_lines {
            let opacity = self.app_state.scrollbar_opacity();
            render_scrollbar(
                area,
                buf,
                total_lines,
                scroll_offset,
                max_scroll,
                visible_lines,
                opacity,
            );
        }

        // Render "go to bottom" indicator if not at bottom
        if !self.app_state.is_chat_at_bottom() && total_lines > visible_lines {
            render_scroll_to_bottom_hint(area, buf, &self.colors);
        }

        // Return actual content height (capped at area height)
        (total_lines as u16).min(area.height)
    }

    /// Generates welcome card as styled lines using TUI components.
    fn _generate_welcome_lines(&self, width: u16) -> Vec<Line<'static>> {
        generate_welcome_lines(width, &self.colors, self.app_state)
    }

    /// Generates message lines for scrollable content.
    fn _generate_message_lines(&self, width: u16) -> Vec<Line<'static>> {
        generate_message_lines(width, &self.colors, self.app_state)
    }

    /// Renders only the chat messages (no welcome cards) - legacy function for compatibility.
    fn _render_messages_only(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() || area.height == 0 {
            return;
        }

        let all_lines = self._generate_message_lines(area.width);
        let total_lines = all_lines.len();
        let visible_lines = area.height as usize;

        if total_lines == 0 {
            return;
        }

        let max_scroll = total_lines.saturating_sub(visible_lines);
        let scroll_offset = self.app_state.chat_scroll.min(max_scroll);

        let start = if total_lines > visible_lines {
            total_lines - visible_lines - scroll_offset
        } else {
            0
        };
        let end = (start + visible_lines).min(total_lines);

        // Render the visible portion
        let visible: Vec<Line<'static>> = all_lines[start..end].to_vec();
        let paragraph = Paragraph::new(visible);
        paragraph.render(area, buf);

        // Render scrollbar if visible (with fade effect)
        let opacity = self.app_state.scrollbar_opacity();
        render_scrollbar(
            area,
            buf,
            total_lines,
            scroll_offset,
            max_scroll,
            visible_lines,
            opacity,
        );

        // Render "go to bottom" indicator if not at bottom
        if !self.app_state.is_chat_at_bottom() && total_lines > visible_lines {
            render_scroll_to_bottom_hint(area, buf, &self.colors);
        }
    }

    /// Renders the input area.
    fn render_input(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() || area.height < 3 {
            return;
        }

        let dim_style = Style::default().fg(self.colors.text_dim);

        // Draw top border
        if let Some(cell) = buf.cell_mut((area.x, area.y)) {
            cell.set_char(border::TOP_LEFT).set_style(dim_style);
        }
        if let Some(cell) = buf.cell_mut((area.right() - 1, area.y)) {
            cell.set_char(border::TOP_RIGHT).set_style(dim_style);
        }
        for x in (area.x + 1)..(area.right() - 1) {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(border::HORIZONTAL).set_style(dim_style);
            }
        }

        // Show queue indicator if there are pending messages
        let queue_count = self.app_state.queued_count();
        if queue_count > 0 {
            let indicator = format!("[{} pending]", queue_count);
            let indicator_x = area.right().saturating_sub(indicator.len() as u16 + 2);
            if indicator_x > area.x + 1 {
                buf.set_string(
                    indicator_x,
                    area.y,
                    &indicator,
                    Style::default().fg(self.colors.warning),
                );
            }
        }

        // Draw bottom border
        if let Some(cell) = buf.cell_mut((area.x, area.bottom() - 1)) {
            cell.set_char(border::BOTTOM_LEFT).set_style(dim_style);
        }
        if let Some(cell) = buf.cell_mut((area.right() - 1, area.bottom() - 1)) {
            cell.set_char(border::BOTTOM_RIGHT).set_style(dim_style);
        }
        for x in (area.x + 1)..(area.right() - 1) {
            if let Some(cell) = buf.cell_mut((x, area.bottom() - 1)) {
                cell.set_char(border::HORIZONTAL).set_style(dim_style);
            }
        }

        // Draw side borders
        if let Some(cell) = buf.cell_mut((area.x, area.y + 1)) {
            cell.set_char(border::VERTICAL).set_style(dim_style);
        }
        if let Some(cell) = buf.cell_mut((area.right() - 1, area.y + 1)) {
            cell.set_char(border::VERTICAL).set_style(dim_style);
        }

        // Get input text from app_state
        let input_text = self.app_state.input.text();

        // Calculate cursor visibility
        let show_cursor = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            / CURSOR_BLINK_INTERVAL_MS)
            .is_multiple_of(2);

        // Content area is inside the box (1 char padding on left/right + 1 space padding)
        // Layout: "│ > content ▌ │"
        // Start x = area.x + 2 (border + space)

        let content_x = area.x + 2;
        let content_y = area.y + 1;
        let content_width = area.width.saturating_sub(4); // 2 borders + 2 spaces padding

        // Simple prompt: "> "
        let prompt_span = Span::styled("> ", Style::default().fg(self.colors.accent));
        let text_span = Span::styled(
            input_text.to_string(),
            Style::default().fg(self.colors.text),
        );

        let mut spans = vec![prompt_span, text_span];

        if show_cursor {
            spans.push(Span::styled("▌", Style::default().fg(self.colors.accent)));
        }

        let line = Line::from(spans);

        let text_area = Rect::new(content_x, content_y, content_width, 1);
        let paragraph = Paragraph::new(line);
        paragraph.render(text_area, buf);
    }

    /// Returns the cursor position for the input field.
    pub fn cursor_position(&self, input_area: Rect) -> Option<(u16, u16)> {
        // Cursor is after "> " prefix (2 chars) plus the input text
        // Input starts at input_area.x + 2 (border + space)
        // Text starts after prompt "> " (length 2)
        // So cursor is at input_area.x + 2 + 2 + cursor_pos

        let cursor_pos = self.app_state.input.cursor_pos();
        // x = area.x + border(1) + space(1) + prompt(2) + cursor_pos
        let x = input_area.x + 4 + cursor_pos as u16;
        let y = input_area.y + 1; // Middle line

        if x < input_area.right() - 2 {
            // Ensure inside right border
            Some((x, y))
        } else {
            None
        }
    }

    /// Returns whether a task is currently running.
    fn is_task_running(&self) -> bool {
        self.app_state.streaming.is_streaming
            || self.app_state.streaming.is_tool_executing()
            || self.app_state.streaming.is_delegating
            || self.app_state.has_active_subagents()
    }

    /// Returns the status header text based on current state.
    fn status_header(&self) -> String {
        // Check for delegation/subagent first (highest priority)
        if self.app_state.streaming.is_delegating || self.app_state.has_active_subagents() {
            "Delegation".to_string()
        } else if self.app_state.streaming.is_tool_executing() {
            let tool_name = self
                .app_state
                .streaming
                .executing_tool
                .as_deref()
                .unwrap_or("tool");
            format!("Executing {}", tool_name)
        } else if self.app_state.streaming.thinking && self.app_state.thinking_budget.is_some() {
            "Thinking".to_string()
        } else if self.app_state.streaming.is_streaming {
            "Working".to_string()
        } else {
            "Idle".to_string()
        }
    }

    /// Calculates the height needed to render all messages.
    #[allow(dead_code)]
    fn calculate_messages_height(&self, width: u16) -> u16 {
        let mut total_lines = 0_usize;

        for msg in &self.app_state.messages {
            let lines = self.render_message(msg, width);
            total_lines += lines.len();
        }

        // Add content segments
        for segment in &self.app_state.content_segments {
            if let crate::views::tool_call::ContentSegment::Text { content, .. } = segment {
                total_lines += (content.len() / 80) + 2;
            }
        }

        // Add pending streaming text
        if !self.app_state.pending_text_segment.is_empty() {
            total_lines += (self.app_state.pending_text_segment.len() / 80) + 2;
        }

        total_lines as u16
    }

    /// Renders autocomplete suggestions inline below the input.
    /// The top stays fixed, only the bottom varies with item count.
    fn render_autocomplete_inline(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let accent = self.colors.accent;
        let dim = self.colors.text_dim;
        let text = self.colors.text;
        let border_style = Style::default().fg(accent);

        // Calculate actual height based on items (top stays fixed, bottom varies)
        let visible_items = self.app_state.autocomplete.visible_items();
        let item_count = visible_items.len().min(8) as u16;
        let actual_height = item_count + 2; // items + top/bottom border

        // Draw top border with rounded corners
        if let Some(cell) = buf.cell_mut((area.x, area.y)) {
            cell.set_char('╭').set_style(border_style);
        }
        if let Some(cell) = buf.cell_mut((area.right() - 1, area.y)) {
            cell.set_char('╮').set_style(border_style);
        }
        for x in (area.x + 1)..(area.right() - 1) {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char('─').set_style(border_style);
            }
        }

        // Draw side borders (only for actual content height)
        for y in (area.y + 1)..(area.y + actual_height - 1) {
            if let Some(cell) = buf.cell_mut((area.x, y)) {
                cell.set_char('│').set_style(border_style);
            }
            if let Some(cell) = buf.cell_mut((area.right() - 1, y)) {
                cell.set_char('│').set_style(border_style);
            }
        }

        // Draw bottom border at actual content height (not at area.bottom)
        let bottom_y = area.y + actual_height - 1;
        if bottom_y > area.y {
            if let Some(cell) = buf.cell_mut((area.x, bottom_y)) {
                cell.set_char('╰').set_style(border_style);
            }
            if let Some(cell) = buf.cell_mut((area.right() - 1, bottom_y)) {
                cell.set_char('╯').set_style(border_style);
            }
            for x in (area.x + 1)..(area.right() - 1) {
                if let Some(cell) = buf.cell_mut((x, bottom_y)) {
                    cell.set_char('─').set_style(border_style);
                }
            }
        }

        // Render items (aligned to top)
        let inner_y = area.y + 1;
        let inner_x = area.x + 2;

        for (i, item) in visible_items.iter().enumerate() {
            if i >= 8 {
                break; // Max 8 visible items
            }
            let y = inner_y + i as u16;

            let is_selected = self.app_state.autocomplete.scroll_offset + i
                == self.app_state.autocomplete.selected;

            // Selection indicator
            let indicator = if is_selected { "> " } else { "  " };
            let indicator_style = if is_selected {
                Style::default().fg(accent)
            } else {
                Style::default().fg(dim)
            };
            buf.set_string(inner_x - 1, y, indicator, indicator_style);

            // Icon
            let mut x = inner_x;
            if item.icon != '\0' {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(item.icon)
                        .set_style(Style::default().fg(accent));
                }
                x += 2;
            }

            // Label
            let label_style = if is_selected {
                Style::default().fg(accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text)
            };
            for ch in item.label.chars() {
                if x >= area.right() - 2 {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch).set_style(label_style);
                }
                x += 1;
            }

            // Description (dimmed)
            if !item.description.is_empty() && x + 4 < area.right() - 2 {
                buf.set_string(x, y, " - ", Style::default().fg(dim));
                x += 3;
                let desc_style = Style::default().fg(dim);
                for ch in item.description.chars() {
                    if x >= area.right() - 2 {
                        break;
                    }
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(ch).set_style(desc_style);
                    }
                    x += 1;
                }
            }
        }
    }
}

impl<'a> Widget for MinimalSessionView<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let is_task_running = self.is_task_running();

        // Calculate fixed heights
        let autocomplete_visible =
            self.app_state.autocomplete.visible && self.app_state.autocomplete.has_items();
        let autocomplete_height: u16 = if autocomplete_visible { 10 } else { 0 };
        let status_height: u16 = if is_task_running { 1 } else { 0 };
        let input_height: u16 = 3;
        let hints_height: u16 = 1;

        // Calculate welcome card heights from render_motd constants
        let welcome_card_height = 11_u16;
        let info_cards_height = 4_u16;
        let welcome_total = welcome_card_height + 1 + info_cards_height; // +1 gap between cards

        // Use layout manager for automatic positioning
        let mut layout = LayoutManager::new(area);

        // 1. Top margin
        layout.gap(1);

        // Calculate available height for scrollable content (before input/hints)
        let bottom_reserved = status_height + input_height + autocomplete_height + hints_height + 2; // +2 for gaps
        let available_height = area.height.saturating_sub(1 + bottom_reserved); // 1 for top margin

        // Render scrollable content area (welcome cards + messages together)
        let content_area = layout.allocate(available_height);
        let actual_content_height =
            self.render_scrollable_content(content_area, buf, welcome_total);

        // Position elements after actual content (not after allocated area)
        let content_end_y = content_area.y + actual_content_height;
        let mut next_y = content_end_y + 1; // +1 gap after content

        // 5. Status indicator (if task running) - follows content
        if is_task_running {
            let status_area = Rect::new(area.x, next_y, area.width, status_height);
            let header = self.status_header();
            let elapsed = self.app_state.streaming.prompt_elapsed_seconds();
            let status = StatusIndicator::new(header)
                .with_elapsed_secs(elapsed)
                .with_interrupt_hint(true);
            status.render(status_area, buf);
            next_y += status_height;
        }

        // 6. Input area - follows status (or content if no status)
        let input_y = next_y;
        let input_area = Rect::new(area.x, input_y, area.width, input_height);

        if self.app_state.is_interactive_mode() {
            if let Some(state) = self.app_state.get_interactive_state() {
                let items_count = state.filtered_indices.len().min(state.max_visible);
                let required_height = (items_count as u16) + 4;
                let max_height = (area.height * 60 / 100).max(12);
                let widget_height = required_height.min(max_height);
                // Position at bottom of screen (above hints)
                let interactive_y = area.y + area.height - widget_height - hints_height;
                let interactive_area = Rect::new(area.x, interactive_y, area.width, widget_height);
                let widget = crate::interactive::InteractiveWidget::new(state);
                widget.render(interactive_area, buf);
            }
        } else {
            self.render_input(input_area, buf);
        }

        // 7. Autocomplete (below input if visible)
        let mut next_y = input_y + input_height;
        if autocomplete_visible {
            let autocomplete_area = Rect::new(area.x, next_y, area.width, autocomplete_height);
            self.render_autocomplete_inline(autocomplete_area, buf);
            next_y += autocomplete_height;
        }

        // 8. Key hints - only show when NOT in interactive mode
        if !self.app_state.is_interactive_mode() {
            let hints_area = Rect::new(area.x, next_y, area.width, hints_height);
            let context = if self.app_state.is_viewing_subagent() {
                HintContext::SubagentView
            } else if is_task_running {
                HintContext::TaskRunning
            } else {
                HintContext::Idle
            };
            let mut hints =
                KeyHints::new(context).with_permission_mode(self.app_state.permission_mode);
            hints = hints.with_model(&self.app_state.model);
            if let Some(ref budget) = self.app_state.thinking_budget {
                hints = hints.with_thinking_budget(budget);
            }
            hints.render(hints_area, buf);

            // Render "← Back to main (Esc)" hint when viewing a subagent
            if self.app_state.is_viewing_subagent() {
                crate::views::minimal_session::rendering::render_back_to_main_hint(
                    hints_area,
                    buf,
                    &self.colors,
                );
            }
        }
    }
}
