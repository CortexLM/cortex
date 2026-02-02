//! Rendering functions for the sessions modal.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use cortex_core::style::{
    BORDER, CYAN_PRIMARY, SURFACE_0, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED, VOID, YELLOW,
};

use super::session_action::SessionAction;
use super::session_info::SessionInfo;

/// Render the search bar at the top.
pub fn render_search_bar(search_query: &str, area: Rect, buf: &mut Buffer) {
    // Background
    for x in area.x..area.right() {
        buf[(x, area.y)].set_bg(SURFACE_1);
    }

    let x = area.x + 2;

    // Search icon
    buf.set_string(x, area.y, ">", Style::default().fg(TEXT_DIM));

    // Search input area
    let bracket_x = x + 3;
    buf.set_string(
        bracket_x,
        area.y,
        "[",
        Style::default().fg(BORDER).bg(SURFACE_1),
    );

    let input_x = bracket_x + 1;
    let max_input_width = (area.width as usize).saturating_sub(8);

    if search_query.is_empty() {
        let placeholder = "Search sessions...";
        let display = if placeholder.len() > max_input_width {
            &placeholder[..max_input_width]
        } else {
            placeholder
        };
        buf.set_string(
            input_x,
            area.y,
            display,
            Style::default().fg(TEXT_MUTED).bg(SURFACE_1),
        );
    } else {
        let display = if search_query.len() > max_input_width {
            &search_query[search_query.len() - max_input_width..]
        } else {
            search_query
        };
        buf.set_string(
            input_x,
            area.y,
            display,
            Style::default().fg(TEXT).bg(SURFACE_1),
        );
    }

    // Cursor
    let cursor_x = input_x + search_query.len().min(max_input_width) as u16;
    if cursor_x < area.right().saturating_sub(2) {
        buf[(cursor_x, area.y)].set_bg(CYAN_PRIMARY);
        buf[(cursor_x, area.y)].set_fg(VOID);
    }

    // Closing bracket
    let close_x = area.right().saturating_sub(2);
    buf.set_string(
        close_x,
        area.y,
        "]",
        Style::default().fg(BORDER).bg(SURFACE_1),
    );
}

/// Render a single session row.
pub fn render_session_row(session: &SessionInfo, is_selected: bool, area: Rect, buf: &mut Buffer) {
    let (bg, fg, prefix_fg) = if is_selected {
        (CYAN_PRIMARY, VOID, VOID)
    } else {
        (SURFACE_0, TEXT, CYAN_PRIMARY)
    };

    // Clear the row
    for x in area.x..area.right() {
        buf[(x, area.y)].set_bg(bg);
    }

    let mut col = area.x;

    // Selection indicator
    let prefix = if is_selected { ">" } else { " " };
    buf.set_string(col, area.y, prefix, Style::default().fg(prefix_fg).bg(bg));
    col += 2;

    // Session name (left-aligned)
    let name_style = Style::default().fg(fg).bg(bg);

    // Build metadata: "2h ago   15 msgs   claude-opus"
    let time_ago = session.relative_time();
    let msg_count = format!("{} msgs", session.message_count);
    let model = session.short_model();
    let meta = format!("{}   {}   {}", time_ago, msg_count, model);
    let meta_len = meta.len();

    // Calculate max name length
    let available_width = (area.width as usize).saturating_sub(4); // prefix + padding
    let max_name_len = available_width.saturating_sub(meta_len + 3); // 3 for spacing

    let truncated_name = if session.name.len() > max_name_len && max_name_len > 3 {
        format!("{}...", &session.name[..max_name_len.saturating_sub(3)])
    } else {
        session.name.clone()
    };

    buf.set_string(col, area.y, &truncated_name, name_style);

    // Render metadata right-aligned
    let meta_style = if is_selected {
        Style::default().fg(VOID).bg(bg)
    } else {
        Style::default().fg(TEXT_DIM).bg(bg)
    };
    let meta_x = area.right().saturating_sub(meta_len as u16 + 2);
    if meta_x > col + truncated_name.len() as u16 + 1 {
        buf.set_string(meta_x, area.y, &meta, meta_style);
    }
}

/// Render the "New Session" row.
pub fn render_new_session_row(is_selected: bool, area: Rect, buf: &mut Buffer) {
    let (bg, fg) = if is_selected {
        (CYAN_PRIMARY, VOID)
    } else {
        (SURFACE_0, TEXT)
    };

    // Clear the row
    for x in area.x..area.right() {
        buf[(x, area.y)].set_bg(bg);
    }

    let mut col = area.x;

    // Selection indicator
    let prefix = if is_selected { ">" } else { " " };
    let prefix_fg = if is_selected { VOID } else { CYAN_PRIMARY };
    buf.set_string(col, area.y, prefix, Style::default().fg(prefix_fg).bg(bg));
    col += 2;

    // "+" icon
    let plus_style = if is_selected {
        Style::default()
            .fg(VOID)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(CYAN_PRIMARY)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    };
    buf.set_string(col, area.y, "+", plus_style);
    col += 2;

    // "New Session" text
    let text_style = Style::default().fg(fg).bg(bg);
    buf.set_string(col, area.y, "New Session", text_style);
}

/// Render the separator line after "New Session".
pub fn render_separator(area: Rect, buf: &mut Buffer) {
    let sep = "─".repeat((area.width as usize).saturating_sub(4));
    buf.set_string(
        area.x + 2,
        area.y,
        &sep,
        Style::default().fg(BORDER).bg(SURFACE_0),
    );
}

/// Renders the confirmation dialog.
pub fn render_confirmation(
    session_name: &str,
    action: &SessionAction,
    area: Rect,
    buf: &mut Buffer,
) {
    let (title, message, warning) = match action {
        SessionAction::Delete => (
            "Delete Session",
            format!("Delete session \"{}\"?", session_name),
            Some("This action cannot be undone."),
        ),
        _ => ("Confirm", "Proceed?".to_string(), None),
    };

    // Background
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            buf[(x, y)].set_bg(SURFACE_0);
        }
    }

    // Title
    let title_style = Style::default()
        .fg(CYAN_PRIMARY)
        .add_modifier(Modifier::BOLD);
    let title_x = area.x + (area.width.saturating_sub(title.len() as u16)) / 2;
    buf.set_string(title_x, area.y + 1, title, title_style);

    // Message
    let msg_style = Style::default().fg(TEXT);
    let msg_x = area.x + (area.width.saturating_sub(message.len() as u16)) / 2;
    buf.set_string(msg_x, area.y + 3, &message, msg_style);

    // Warning (if any)
    if let Some(warn) = warning {
        let warn_style = Style::default().fg(YELLOW).add_modifier(Modifier::ITALIC);
        let warn_x = area.x + (area.width.saturating_sub(warn.len() as u16)) / 2;
        buf.set_string(warn_x, area.y + 4, warn, warn_style);
    }
}
