//! Rendering functions for minimal session view.
//!
//! Contains all render_* methods for messages, tool calls, subagents, and UI elements.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget,
};

use cortex_core::markdown::MarkdownTheme;
use cortex_core::widgets::{Brain, Message, MessageRole};
use cortex_tui_components::welcome_card::{InfoCard, InfoCardPair, ToLines, WelcomeCard};

use crate::app::{AppState, SubagentDisplayStatus, SubagentTaskDisplay};
use crate::ui::colors::AdaptiveColors;
use crate::views::tool_call::{ContentSegment, ToolCallDisplay, ToolStatus};

use super::VERSION;
use super::text_utils::wrap_text;

/// Renders the "← Back to main conversation" hint when viewing a subagent.
/// Displays in the top-left area of the screen.
pub fn render_back_to_main_hint(area: Rect, buf: &mut Buffer, colors: &AdaptiveColors) {
    let hint = "← Back to main (Esc)";
    let style = Style::default().fg(colors.text_dim);
    // Render at the start of the area with 1 character padding
    buf.set_string(area.x + 1, area.y, hint, style);
}

/// Renders a single message to lines with optional markdown theme.
pub fn render_message_with_theme(
    msg: &Message,
    width: u16,
    colors: &AdaptiveColors,
    markdown_theme: &MarkdownTheme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Colors for user messages
    let light_green = Color::Rgb(0x80, 0xFF, 0xD0); // Vert clair pour texte utilisateur

    match msg.role {
        MessageRole::User => {
            // "> message" - prefix vert accent, texte vert clair
            let prefix = Span::styled("> ", Style::default().fg(colors.accent));

            // Calculate available width for text (after "> " prefix)
            let text_width = (width as usize).saturating_sub(3); // "> " + margin

            // Wrap text and render each line
            let wrapped_lines = wrap_text(&msg.content, text_width);
            for (i, line_content) in wrapped_lines.iter().enumerate() {
                if i == 0 {
                    lines.push(Line::from(vec![
                        prefix.clone(),
                        Span::styled(line_content.clone(), Style::default().fg(light_green)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("  "), // Indent continuation (2 spaces = "> " length)
                        Span::styled(line_content.clone(), Style::default().fg(light_green)),
                    ]));
                }
            }
        }
        MessageRole::Assistant => {
            // Use full markdown renderer with theme
            use cortex_core::markdown::MarkdownRenderer;

            // Create renderer with width and theme
            let content_width = width.saturating_sub(4); // Leave margin
            let renderer =
                MarkdownRenderer::with_theme(markdown_theme.clone()).with_width(content_width);

            // Render markdown content
            let mut rendered_lines = renderer.render(&msg.content);

            // Add streaming cursor if still streaming
            if msg.is_streaming
                && let Some(last) = rendered_lines.last_mut()
            {
                last.spans
                    .push(Span::styled("▌", Style::default().fg(colors.accent)));
            }

            lines.extend(rendered_lines);
        }
        MessageRole::System => {
            // Detect error messages - no prefix, show in error color
            let is_error = msg.content.contains("Check your")
                || msg.content.contains("Access denied")
                || msg.content.contains("timed out")
                || msg.content.contains("failed")
                || msg.content.contains("Invalid")
                || msg.content.contains("limit")
                || msg.content.starts_with("Error:");

            let (prefix, text_color) = if is_error {
                // Error messages: no prefix, use error color
                (Span::raw(""), colors.error)
            } else {
                // Info messages: [i] prefix, use muted color
                (
                    Span::styled("[i] ", Style::default().fg(colors.text_muted)),
                    colors.text_muted,
                )
            };

            // Calculate available width for text
            let prefix_width = if is_error { 0 } else { 2 };
            let text_width = (width as usize).saturating_sub(prefix_width + 1);

            // Wrap text and render each line
            let wrapped_lines = wrap_text(&msg.content, text_width);
            for (i, line_content) in wrapped_lines.iter().enumerate() {
                if i == 0 {
                    let mut spans = Vec::new();
                    if !is_error {
                        spans.push(prefix.clone());
                    }
                    spans.push(Span::styled(
                        line_content.clone(),
                        Style::default().fg(text_color),
                    ));
                    lines.push(Line::from(spans));
                } else {
                    let indent = if is_error { "" } else { "  " };
                    lines.push(Line::from(vec![
                        Span::raw(indent.to_string()),
                        Span::styled(line_content.clone(), Style::default().fg(text_color)),
                    ]));
                }
            }
        }
        MessageRole::Tool => {
            // "[>] tool_name: result"
            let prefix = Span::styled("[>] ", Style::default().fg(colors.accent));
            let tool_name = msg.tool_name.as_deref().unwrap_or("tool");
            let name_span = Span::styled(
                format!("{}: ", tool_name),
                Style::default().fg(colors.text_dim),
            );

            // Truncate content for tool results
            let max_content = 100;
            let content = if msg.content.len() > max_content {
                format!("{}...", &msg.content[..max_content])
            } else {
                msg.content.clone()
            };

            lines.push(Line::from(vec![
                prefix,
                name_span,
                Span::styled(content, Style::default().fg(colors.text_muted)),
            ]));
        }
    }

    // Add blank line after each message for spacing
    lines.push(Line::from(""));

    lines
}

/// Renders a single message to lines (uses default theme).
/// For backwards compatibility - prefer render_message_with_theme when theme is available.
pub fn render_message(msg: &Message, width: u16, colors: &AdaptiveColors) -> Vec<Line<'static>> {
    render_message_with_theme(msg, width, colors, &MarkdownTheme::default())
}

/// Renders a single tool call with status indicator
pub fn render_tool_call(
    call: &ToolCallDisplay,
    width: u16,
    colors: &AdaptiveColors,
) -> Vec<Line<'static>> {
    use crate::ui::consts::TOOL_SPINNER_FRAMES;
    let mut lines = Vec::new();

    // Calculate available width for content (accounting for indentation)
    let content_width = (width as usize).saturating_sub(6); // 6 chars for prefix/indent
    let line_width = (width as usize).saturating_sub(8); // 8 chars for nested content

    // Status indicator with color - animated spinner for Running status
    let (dot, dot_color) = match call.status {
        ToolStatus::Pending => ("○".to_string(), colors.warning),
        ToolStatus::Running => {
            // Animated spinner using half-circle frames
            let frame = TOOL_SPINNER_FRAMES[call.spinner_frame % TOOL_SPINNER_FRAMES.len()];
            (frame.to_string(), colors.accent)
        }
        ToolStatus::Completed => ("●".to_string(), colors.success),
        ToolStatus::Failed => ("●".to_string(), colors.error),
    };

    // Line 1: ◐ ToolName summary_args (truncate summary to fit terminal width)
    let summary = crate::views::tool_call::format_tool_summary(&call.name, &call.arguments);
    let max_summary = content_width.saturating_sub(call.name.len() + 2);
    let summary_truncated = if summary.len() > max_summary {
        format!(
            "{}...",
            &summary
                .chars()
                .take(max_summary.saturating_sub(3))
                .collect::<String>()
        )
    } else {
        summary
    };
    lines.push(Line::from(vec![
        Span::styled(dot, Style::default().fg(dot_color)),
        Span::raw(" "),
        Span::styled(
            call.name.clone(),
            Style::default()
                .fg(colors.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(summary_truncated, Style::default().fg(colors.text_dim)),
    ]));

    // Live output lines (for Running status with output)
    if call.status == ToolStatus::Running && !call.live_output.is_empty() {
        for output_line in &call.live_output {
            // Truncate long lines to fit terminal width
            let truncated = if output_line.len() > line_width {
                format!(
                    "{}...",
                    &output_line
                        .chars()
                        .take(line_width.saturating_sub(3))
                        .collect::<String>()
                )
            } else {
                output_line.clone()
            };
            lines.push(Line::from(vec![
                Span::styled("  │ ", Style::default().fg(colors.text_muted)),
                Span::styled(truncated, Style::default().fg(colors.text_dim)),
            ]));
        }
    }

    // Result summary line (if completed/failed) - truncate to fit terminal width
    if let Some(ref result) = call.result {
        let result_color = if result.success {
            colors.text_dim
        } else {
            colors.error
        };
        let summary_truncated = if result.summary.len() > line_width {
            format!(
                "{}...",
                &result
                    .summary
                    .chars()
                    .take(line_width.saturating_sub(3))
                    .collect::<String>()
            )
        } else {
            result.summary.clone()
        };
        lines.push(Line::from(vec![
            Span::raw("  ⎿ "),
            Span::styled(summary_truncated, Style::default().fg(result_color)),
        ]));

        // If error and not collapsed, show full error with wrapping
        if !result.success && !call.collapsed {
            for err_line in result.output.lines().take(5) {
                // Wrap long error lines
                let wrapped = wrap_text(err_line, line_width.saturating_sub(4));
                for wrapped_line in wrapped.iter() {
                    lines.push(Line::from(vec![
                        Span::raw("    ".to_string()),
                        Span::styled(wrapped_line.clone(), Style::default().fg(colors.error)),
                    ]));
                }
            }
        }
    }

    // Expanded view (if not collapsed and has result)
    if !call.collapsed && call.result.is_some() {
        // Show arguments
        lines.push(Line::from(Span::styled(
            "  Arguments:",
            Style::default().fg(colors.text_dim),
        )));
        if let Ok(args_str) = serde_json::to_string_pretty(&call.arguments) {
            for arg_line in args_str.lines().take(10) {
                // Wrap long argument lines
                let wrapped = wrap_text(arg_line, line_width.saturating_sub(4));
                for wrapped_line in wrapped.iter() {
                    lines.push(Line::from(vec![
                        Span::raw("    ".to_string()),
                        Span::styled(wrapped_line.clone(), Style::default().fg(colors.text)),
                    ]));
                }
            }
        }
    }

    lines.push(Line::from("")); // Spacing
    lines
}

/// Renders a subagent task with todos in Factory-style format
///
/// Format:
/// ```text
/// ● Task {agent_type}
///   ⎿ [pending] task1
///     [in_progress] task2
///     [completed] task3
/// ```
pub fn render_subagent(
    task: &SubagentTaskDisplay,
    width: u16,
    colors: &AdaptiveColors,
) -> Vec<Line<'static>> {
    use crate::app::SubagentTodoStatus;
    let mut lines = Vec::new();

    // Calculate available width for content (accounting for indentation)
    let content_width = (width as usize).saturating_sub(6); // 6 chars for prefix/indent
    let line_width = (width as usize).saturating_sub(8); // 8 chars for nested content

    // Status indicator with color
    let (indicator, indicator_color) = match &task.status {
        SubagentDisplayStatus::Starting
        | SubagentDisplayStatus::Thinking
        | SubagentDisplayStatus::ExecutingTool(_) => ("●", colors.accent),
        SubagentDisplayStatus::Completed => ("●", colors.success),
        SubagentDisplayStatus::Failed => ("●", colors.error),
    };

    // Line 1: ● Task {agent_type}
    lines.push(Line::from(vec![
        Span::styled(indicator, Style::default().fg(indicator_color)),
        Span::raw(" "),
        Span::styled(
            format!("Task {}", task.agent_type),
            Style::default()
                .fg(colors.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Display error message if task failed
    if task.status == SubagentDisplayStatus::Failed {
        if let Some(ref error_msg) = task.error_message {
            lines.push(Line::from(vec![
                Span::styled("  ⎿ ", Style::default().fg(colors.text_muted)),
                Span::styled("Error: ", Style::default().fg(colors.error)),
            ]));
            // Display error message with wrapping
            for err_line in error_msg.lines().take(5) {
                let wrapped = wrap_text(err_line, line_width.saturating_sub(4));
                for wrapped_line in wrapped.iter() {
                    lines.push(Line::from(vec![
                        Span::styled("    ", Style::default().fg(colors.text_muted)),
                        Span::styled(wrapped_line.clone(), Style::default().fg(colors.error)),
                    ]));
                }
            }
        } else {
            // Fallback: no error message provided
            lines.push(Line::from(vec![
                Span::styled("  ⎿ ", Style::default().fg(colors.text_muted)),
                Span::styled("Task failed", Style::default().fg(colors.error)),
            ]));
        }
    } else if !task.todos.is_empty() {
        // Display todos if any - use ⎿ prefix for first, space for rest
        for (i, todo) in task.todos.iter().enumerate() {
            let (status_text, status_color) = match todo.status {
                SubagentTodoStatus::Completed => ("[completed]", colors.success),
                SubagentTodoStatus::InProgress => ("[in_progress]", colors.accent),
                SubagentTodoStatus::Pending => ("[pending]", colors.text_muted),
            };
            // Calculate max content width (accounting for status text)
            let max_content = content_width.saturating_sub(status_text.len() + 1);
            let content = if todo.content.len() > max_content {
                format!(
                    "{}...",
                    &todo
                        .content
                        .chars()
                        .take(max_content.saturating_sub(3))
                        .collect::<String>()
                )
            } else {
                todo.content.clone()
            };
            // First line uses ⎿, rest use indentation
            let prefix = if i == 0 { "  ⎿ " } else { "    " };
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(colors.text_muted)),
                Span::styled(status_text, Style::default().fg(status_color)),
                Span::styled(" ", Style::default()),
                Span::styled(content, Style::default().fg(colors.text_dim)),
            ]));
        }
    } else {
        // No todos yet - show current activity with ⎿ (wrap if too long)
        let activity = if task.current_activity.is_empty() {
            "Initializing...".to_string()
        } else if task.current_activity.len() > content_width {
            format!(
                "{}...",
                &task
                    .current_activity
                    .chars()
                    .take(content_width.saturating_sub(3))
                    .collect::<String>()
            )
        } else {
            task.current_activity.clone()
        };
        lines.push(Line::from(vec![
            Span::styled("  ⎿ ", Style::default().fg(colors.text_muted)),
            Span::styled(activity, Style::default().fg(colors.text_dim)),
        ]));
    }

    lines.push(Line::from("")); // Spacing
    lines
}

/// Generates welcome card as styled lines using TUI components.
pub fn generate_welcome_lines(
    width: u16,
    colors: &AdaptiveColors,
    app_state: &AppState,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Get user info
    let user_name = app_state.user_name.as_deref().unwrap_or("User");
    let org_name = app_state.org_name.as_deref().unwrap_or("Personal");
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "~/".to_string());

    // Create welcome card using component
    let welcome_card = WelcomeCard::new()
        .user_name(user_name)
        .subtitle("Your AI-powered coding assistant.")
        .version(VERSION)
        .tips(&[
            "Send /help for available commands.",
            "Use Tab for autocomplete. Press Esc to cancel.",
        ])
        .accent_color(colors.accent)
        .text_color(colors.text)
        .dim_color(colors.text_dim)
        .border_color(colors.accent);

    // Generate lines from welcome card
    lines.extend(welcome_card.to_lines(width));

    // Gap between cards
    lines.push(Line::from(""));

    // Create info cards using components
    let left_card = InfoCard::new()
        .add("Directory", &cwd)
        .add("Org", org_name)
        .dim_color(colors.text_dim)
        .text_color(colors.text)
        .border_color(colors.accent);

    let right_card = InfoCard::new()
        .add("Plan", "Pro")
        .add("", "")
        .dim_color(colors.text_dim)
        .text_color(colors.text)
        .border_color(colors.accent);

    let info_cards = InfoCardPair::new(left_card, right_card)
        .gap(2)
        .right_width(25);

    // Generate lines from info cards
    lines.extend(info_cards.to_lines(width));

    lines
}

/// Generates message lines for scrollable content.
pub fn generate_message_lines(
    width: u16,
    colors: &AdaptiveColors,
    app_state: &AppState,
) -> Vec<Line<'static>> {
    let mut all_lines: Vec<Line<'static>> = Vec::new();

    if app_state.messages.is_empty()
        && !app_state.streaming.is_streaming
        && app_state.content_segments.is_empty()
    {
        return all_lines;
    }

    // Determine what content we have for display
    let has_tool_calls = !app_state.tool_calls.is_empty();
    let has_content_segments = !app_state.content_segments.is_empty();
    let last_is_assistant = app_state
        .messages
        .last()
        .map(|m| m.role == cortex_core::widgets::MessageRole::Assistant)
        .unwrap_or(false);

    // If we have content segments, skip the last assistant message (it's in the segments)
    let messages_to_render = if has_content_segments && last_is_assistant {
        let len = app_state.messages.len();
        &app_state.messages[..len.saturating_sub(1)]
    } else {
        &app_state.messages[..]
    };

    // Get markdown theme from app state
    let markdown_theme = &app_state.markdown_theme;

    for msg in messages_to_render.iter() {
        all_lines.extend(render_message_with_theme(
            msg,
            width,
            colors,
            markdown_theme,
        ));
    }

    // Get streaming content if any
    let streaming_content = if app_state.streaming.is_streaming {
        app_state
            .typewriter
            .as_ref()
            .map(|tw| tw.visible_text().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    // Render content segments (interleaved text and tool calls)
    if has_content_segments {
        let mut sorted_segments: Vec<_> = app_state.content_segments.iter().collect();
        sorted_segments.sort_by_key(|s| s.sequence());

        for segment in sorted_segments {
            match segment {
                ContentSegment::Text { content, .. } => {
                    all_lines.extend(render_text_content_with_theme(
                        content,
                        width,
                        markdown_theme,
                    ));
                }
                ContentSegment::ToolCall { tool_call_id, .. } => {
                    if let Some(call) = app_state.tool_calls.iter().find(|c| &c.id == tool_call_id)
                    {
                        all_lines.extend(render_tool_call(call, width, colors));
                    }
                }
            }
        }

        if app_state.streaming.is_streaming {
            let pending_text = &app_state.pending_text_segment;
            if !pending_text.is_empty() {
                all_lines.extend(render_streaming_content_with_theme(
                    pending_text,
                    width,
                    colors,
                    markdown_theme,
                ));
            }
        }
    } else if has_tool_calls {
        let mut sorted_calls: Vec<_> = app_state.tool_calls.iter().collect();
        sorted_calls.sort_by_key(|c| c.sequence);

        if let Some(ref content) = streaming_content {
            all_lines.extend(render_streaming_content_with_theme(
                content,
                width,
                colors,
                markdown_theme,
            ));
        }

        for call in &sorted_calls {
            all_lines.extend(render_tool_call(call, width, colors));
        }
    } else if let Some(ref content) = streaming_content {
        all_lines.extend(render_streaming_content_with_theme(
            content,
            width,
            colors,
            markdown_theme,
        ));
    }

    // Render active subagents
    for task in &app_state.active_subagents {
        all_lines.extend(render_subagent(task, width, colors));
    }

    all_lines
}

/// Renders finalized text content with markdown theme (without streaming cursor).
/// Used for text segments that are already committed in content_segments.
pub fn render_text_content_with_theme(
    content: &str,
    width: u16,
    markdown_theme: &MarkdownTheme,
) -> Vec<Line<'static>> {
    use cortex_core::markdown::MarkdownRenderer;

    let content_width = width.saturating_sub(4);
    let renderer = MarkdownRenderer::with_theme(markdown_theme.clone()).with_width(content_width);

    // No cursor for finalized content
    renderer.render(content)
}

/// Renders finalized text content (without streaming cursor).
/// Used for text segments that are already committed in content_segments.
/// For backwards compatibility - prefer render_text_content_with_theme when theme is available.
#[allow(dead_code)]
pub fn render_text_content(
    content: &str,
    width: u16,
    _colors: &AdaptiveColors,
) -> Vec<Line<'static>> {
    render_text_content_with_theme(content, width, &MarkdownTheme::default())
}

/// Renders streaming content with cursor and markdown theme.
/// Used only for actively streaming content (pending_text_segment).
pub fn render_streaming_content_with_theme(
    content: &str,
    width: u16,
    colors: &AdaptiveColors,
    markdown_theme: &MarkdownTheme,
) -> Vec<Line<'static>> {
    use cortex_core::markdown::MarkdownRenderer;

    let content_width = width.saturating_sub(4);
    let renderer = MarkdownRenderer::with_theme(markdown_theme.clone()).with_width(content_width);
    let mut rendered_lines = renderer.render(content);

    // Add streaming cursor to the last line
    if let Some(last) = rendered_lines.last_mut() {
        last.spans
            .push(Span::styled("▌", Style::default().fg(colors.accent)));
    }

    rendered_lines.push(Line::from(""));
    rendered_lines
}

/// Renders streaming content with cursor.
/// Used only for actively streaming content (pending_text_segment).
/// For backwards compatibility - prefer render_streaming_content_with_theme when theme is available.
#[allow(dead_code)]
pub fn render_streaming_content(
    content: &str,
    width: u16,
    colors: &AdaptiveColors,
) -> Vec<Line<'static>> {
    render_streaming_content_with_theme(content, width, colors, &MarkdownTheme::default())
}

/// Renders a thin scrollbar on the right side with fade effect.
pub fn render_scrollbar(
    area: Rect,
    buf: &mut Buffer,
    total_lines: usize,
    scroll_offset: usize,
    max_scroll: usize,
    visible_lines: usize,
    opacity: f32,
) {
    if opacity <= 0.0 {
        return;
    }

    // No scrollbar needed if content fits
    if total_lines <= visible_lines || max_scroll == 0 {
        return;
    }

    // Calculate thumb color with fade (base: gray #606060)
    let gray_value = (0x60 as f32 * opacity) as u8;
    let thumb_color = Color::Rgb(gray_value, gray_value, gray_value);

    // scroll_offset = 0 means at bottom, max_scroll means at top
    // Scrollbar position: 0 = top of content, total_lines - visible_lines = bottom
    // When scroll_offset = 0 (at bottom), position should be at max (bottom of scrollbar)
    // When scroll_offset = max_scroll (at top), position should be 0 (top of scrollbar)
    let scrollbar_position = max_scroll.saturating_sub(scroll_offset);

    // Create scrollbar state
    // content_length = max_scroll (the scrollable range)
    // position = where we are in that range
    let mut scrollbar_state = ScrollbarState::new(max_scroll).position(scrollbar_position);

    // Render thin scrollbar on right edge
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(None) // Invisible track for clean look
        .thumb_symbol("▐") // Thin character
        .thumb_style(Style::default().fg(thumb_color))
        .render(area, buf, &mut scrollbar_state);
}

/// Renders a hint to scroll to bottom when user has scrolled up.
pub fn render_scroll_to_bottom_hint(area: Rect, buf: &mut Buffer, colors: &AdaptiveColors) {
    let hint = " ↓ End ";
    let hint_width = hint.len() as u16;

    // Position: bottom-right of chat area
    let x = area.right().saturating_sub(hint_width + 2);
    let y = area.bottom().saturating_sub(1);

    if x >= area.x && y >= area.y {
        // Background pill style
        let style = Style::default()
            .fg(colors.text)
            .bg(Color::Rgb(0x30, 0x30, 0x30))
            .add_modifier(Modifier::BOLD);

        buf.set_string(x, y, hint, style);
    }
}

/// Renders the MOTD (Message of the Day) with cards layout.
///
/// Layout: Main card with mascot + welcome, then two info cards below.
pub fn _render_motd(area: Rect, buf: &mut Buffer, colors: &AdaptiveColors, app_state: &AppState) {
    let card_width = 79_u16.min(area.width.saturating_sub(2));
    let welcome_card_height = 11_u16;
    let info_cards_height = 4_u16; // 2 items + 2 borders
    let gap = 1_u16;
    let total_height = welcome_card_height + gap + info_cards_height;

    // Ensure we have enough space
    if area.width < 40 || area.height < total_height {
        _render_welcome_text_centered(area, buf, colors, app_state);
        return;
    }

    // Center horizontally, start 1 line from top
    let x_offset = area.width.saturating_sub(card_width) / 2;
    let y_start = 1_u16; // Start 1 line below the top

    // Welcome card area
    let welcome_area = Rect::new(
        area.x + x_offset,
        area.y + y_start,
        card_width,
        welcome_card_height,
    );

    // Get user info from app_state
    let user_name = app_state.user_name.as_deref().unwrap_or("User");

    // Render welcome card using the component with accent color for borders
    let welcome_card = WelcomeCard::new()
        .user_name(user_name)
        .subtitle("Your AI-powered coding assistant.")
        .version(VERSION)
        .tips(&[
            "Send /help for available commands.",
            "Use Tab for autocomplete. Press Esc to cancel.",
        ])
        .accent_color(colors.accent)
        .text_color(colors.text)
        .dim_color(colors.text_dim)
        .border_color(colors.accent);

    welcome_card.render(welcome_area, buf);

    // Info cards area (below welcome card)
    let info_area = Rect::new(
        area.x + x_offset,
        area.y + y_start + welcome_card_height + gap,
        card_width,
        info_cards_height,
    );

    // Get info from app_state
    let _user_email = app_state.user_email.as_deref().unwrap_or("user@cortex.ai");
    let org_name = app_state.org_name.as_deref().unwrap_or("Personal");
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "~/".to_string());

    // Left card: Directory, Org
    let left_card = InfoCard::new()
        .add("Directory", &cwd)
        .add("Org", org_name)
        .dim_color(colors.text_dim)
        .text_color(colors.text)
        .border_color(colors.accent);

    // Right card: Plan only (Model removed)
    let right_card = InfoCard::new()
        .add("Plan", "Pro")
        .dim_color(colors.text_dim)
        .text_color(colors.text)
        .border_color(colors.accent);

    // Render info cards side by side
    InfoCardPair::new(left_card, right_card)
        .gap(2)
        .right_width(25)
        .render(info_area, buf);
}

/// Renders the welcome text next to the brain (legacy).
#[allow(dead_code)]
pub fn render_welcome_text(area: Rect, buf: &mut Buffer, colors: &AdaptiveColors, model: &str) {
    let accent = colors.accent;
    let text_color = colors.text;
    let dim = colors.text_dim;

    let short_model = model.rsplit('/').next().unwrap_or(model);

    let lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            "Welcome to Cortex",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled("─────────────────", Style::default().fg(dim))),
        Line::from(""),
        Line::from(vec![
            Span::styled("Model: ", Style::default().fg(dim)),
            Span::styled(short_model.to_string(), Style::default().fg(text_color)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "• Type a message to start",
            Style::default().fg(dim),
        )),
        Line::from(Span::styled(
            "• Use / for commands",
            Style::default().fg(dim),
        )),
        Line::from(Span::styled("• Press ? for help", Style::default().fg(dim))),
        Line::from(Span::styled("• Ctrl+Q to quit", Style::default().fg(dim))),
    ];

    let paragraph = Paragraph::new(lines);
    paragraph.render(area, buf);
}

/// Renders welcome text centered (fallback for small terminals).
pub fn _render_welcome_text_centered(
    area: Rect,
    buf: &mut Buffer,
    colors: &AdaptiveColors,
    _app_state: &AppState,
) {
    let accent = colors.accent;
    let dim = colors.text_dim;

    let lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            "Welcome to Cortex",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Type a message to start • / for commands • ? for help",
            Style::default().fg(dim),
        )),
    ];

    // Center vertically
    let y_offset = area.height.saturating_sub(3) / 2;
    let text_area = Rect::new(area.x, area.y + y_offset, area.width, 3);

    let paragraph = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center);
    paragraph.render(text_area, buf);
}

/// Renders a compact MOTD at the top of the chat area (when messages exist).
/// Shows Brain animation on left and info on right, aligned to top.
#[allow(dead_code)]
pub fn render_motd_compact(
    area: Rect,
    buf: &mut Buffer,
    colors: &AdaptiveColors,
    app_state: &AppState,
) {
    let brain_width = Brain::width();
    let brain_height = Brain::height();
    let brain_text_gap = 4_u16;
    let text_width = 35_u16;

    let total_content_width = brain_width + brain_text_gap + text_width;

    // Check if we have enough space
    if area.width < total_content_width + 2 || area.height < 8 {
        // Not enough space, just show a minimal header
        let accent = colors.accent;
        let dim = colors.text_dim;
        let line = Line::from(vec![
            Span::styled(
                "Cortex",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" | ", Style::default().fg(dim)),
            Span::styled(&app_state.model, Style::default().fg(colors.text)),
        ]);
        Paragraph::new(line).render(area, buf);
        return;
    }

    // Center horizontally
    let x_offset = area.width.saturating_sub(total_content_width) / 2;

    // Brain area (left side, at top)
    let brain_area = Rect::new(
        area.x + x_offset,
        area.y,
        brain_width,
        brain_height.min(area.height),
    );

    // Render animated brain
    let brain = Brain::new()
        .with_frame(app_state.brain_frame)
        .with_intensity(1.0);
    brain.render(brain_area, buf);

    // Text area (right of brain)
    let text_x = area.x + x_offset + brain_width + brain_text_gap;
    let text_area = Rect::new(text_x, area.y, text_width, area.height);

    // Compact welcome text
    let accent = colors.accent;
    let text_color = colors.text;
    let dim = colors.text_dim;

    let short_model = app_state
        .model
        .rsplit('/')
        .next()
        .unwrap_or(&app_state.model);

    let lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            "Cortex",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled("─────────────────", Style::default().fg(dim))),
        Line::from(vec![
            Span::styled("Model: ", Style::default().fg(dim)),
            Span::styled(short_model.to_string(), Style::default().fg(text_color)),
        ]),
        Line::from(""),
        Line::from(Span::styled("/ commands  ? help", Style::default().fg(dim))),
    ];

    Paragraph::new(lines).render(text_area, buf);
}
