//! Tool approval modal view.
//!
//! This view is displayed when a tool requires user approval before execution.
//! It shows the tool name, arguments, and optional diff preview, allowing the
//! user to approve, reject, or configure automatic approval behavior.

use crate::app::{AppState, ApprovalState};
use cortex_core::style::{
    BLUE, BORDER, GREEN, ORANGE, PINK, RED, SURFACE_1, TEXT, TEXT_DIM, YELLOW,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

// ============================================================
// CONSTANTS
// ============================================================

/// Maximum width of the modal in characters
const MAX_MODAL_WIDTH: u16 = 100;

/// Maximum height as percentage of terminal
const MAX_HEIGHT_PERCENT: u16 = 80;

/// Width as percentage of terminal
const WIDTH_PERCENT: u16 = 80;

/// Padding inside the modal
const INNER_PADDING: u16 = 2;

/// Maximum lines to show in arguments section
const MAX_ARGS_LINES: u16 = 10;

/// Maximum lines to show in diff preview
const MAX_DIFF_LINES: u16 = 15;

// ============================================================
// APPROVAL VIEW
// ============================================================

/// Tool approval modal view.
///
/// This modal is shown when a tool requires explicit user approval before
/// execution. It displays:
/// - Tool name
/// - Arguments (JSON pretty-printed with syntax highlighting)
/// - Optional diff preview for file modifications
/// - Action hints for user interaction
///
/// # Example
/// ```rust,ignore
/// use cortex_tui::views::ApprovalView;
/// use cortex_tui::app::AppState;
///
/// let state = AppState::default();
/// let approval = ApprovalView::new(&state);
/// // Render using ratatui's Frame::render_widget
/// ```
pub struct ApprovalView<'a> {
    state: &'a AppState,
}

impl<'a> ApprovalView<'a> {
    /// Creates a new ApprovalView with the given application state.
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    /// Renders the title section.
    fn render_title(&self, area: Rect, buf: &mut Buffer) {
        let title = Paragraph::new("Tool Approval Required")
            .style(Style::default().fg(ORANGE).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center);
        title.render(area, buf);
    }

    /// Renders the tool name section.
    fn render_tool_name(&self, area: Rect, buf: &mut Buffer, tool_name: &str) {
        let label_style = Style::default().fg(TEXT_DIM);
        let tool_style = Style::default().fg(PINK).add_modifier(Modifier::BOLD);

        let line = Line::from(vec![
            Span::styled("Tool: ", label_style),
            Span::styled(tool_name, tool_style),
        ]);

        Paragraph::new(line).render(area, buf);
    }

    /// Renders the arguments section with syntax-highlighted JSON.
    fn render_arguments(&self, area: Rect, buf: &mut Buffer, args: &str) {
        // Render label
        let label = Paragraph::new("Arguments:").style(Style::default().fg(TEXT_DIM));
        let label_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        label.render(label_area, buf);

        // Arguments box area
        if area.height <= 2 {
            return;
        }
        let args_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(1),
        };

        // Render arguments box border
        let args_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(SURFACE_1));
        let inner_area = args_block.inner(args_area);
        args_block.render(args_area, buf);

        // Pretty-print and syntax highlight JSON
        let formatted = if let Ok(value) = serde_json::from_str::<serde_json::Value>(args) {
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| args.to_string())
        } else {
            args.to_string()
        };
        let highlighted_lines = syntax_highlight_json(&formatted);

        let text: Vec<Line> = highlighted_lines
            .into_iter()
            .take(MAX_ARGS_LINES as usize)
            .collect();

        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        paragraph.render(inner_area, buf);
    }

    /// Renders the diff preview section.
    fn render_diff_preview(&self, area: Rect, buf: &mut Buffer, diff: &str) {
        // Render label
        let label = Paragraph::new("Preview:").style(Style::default().fg(TEXT_DIM));
        let label_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        label.render(label_area, buf);

        // Diff box area
        if area.height <= 2 {
            return;
        }
        let diff_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(1),
        };

        // Render diff box border
        let diff_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(SURFACE_1));
        let inner_area = diff_block.inner(diff_area);
        diff_block.render(diff_area, buf);

        // Render diff with syntax highlighting
        let diff_lines = render_diff_lines(diff, MAX_DIFF_LINES as usize);
        let paragraph = Paragraph::new(diff_lines).wrap(Wrap { trim: false });
        paragraph.render(inner_area, buf);
    }

    /// Renders the action hints at the bottom.
    fn render_actions(&self, area: Rect, buf: &mut Buffer) {
        let approve_style = Style::default().fg(GREEN);
        let reject_style = Style::default().fg(RED);
        let option_style = Style::default().fg(BLUE);
        let key_style = Style::default().fg(YELLOW).add_modifier(Modifier::BOLD);
        let dim_style = Style::default().fg(TEXT_DIM);

        let line = Line::from(vec![
            Span::styled("[", dim_style),
            Span::styled("y", key_style),
            Span::styled("] ", dim_style),
            Span::styled("Approve", approve_style),
            Span::styled("  ", dim_style),
            Span::styled("[", dim_style),
            Span::styled("n", key_style),
            Span::styled("] ", dim_style),
            Span::styled("Reject", reject_style),
            Span::styled("  ", dim_style),
            Span::styled("[", dim_style),
            Span::styled("a", key_style),
            Span::styled("] ", dim_style),
            Span::styled("Always", option_style),
            Span::styled("  ", dim_style),
            Span::styled("[", dim_style),
            Span::styled("s", key_style),
            Span::styled("] ", dim_style),
            Span::styled("Session", option_style),
            Span::styled("  ", dim_style),
            Span::styled("[", dim_style),
            Span::styled("d", key_style),
            Span::styled("] ", dim_style),
            Span::styled("Diff", option_style),
        ]);

        let paragraph = Paragraph::new(line).alignment(Alignment::Center);
        paragraph.render(area, buf);
    }
}

impl Widget for ApprovalView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Only render if there's a pending approval
        let Some(approval) = &self.state.pending_approval else {
            return;
        };

        // Calculate modal dimensions
        let modal_width =
            ((area.width as u32 * WIDTH_PERCENT as u32) / 100).min(MAX_MODAL_WIDTH as u32) as u16;
        let content_height = calculate_content_height(approval);
        let modal_height = content_height.min((area.height * MAX_HEIGHT_PERCENT) / 100);

        // Center the modal
        let modal_area = center_rect(modal_width, modal_height, area);

        // Clear the modal area (draw background overlay)
        Clear.render(modal_area, buf);

        // Fill modal background
        for y in modal_area.y..modal_area.y + modal_area.height {
            for x in modal_area.x..modal_area.x + modal_area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_bg(SURFACE_1);
                }
            }
        }

        // Render modal border
        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(PINK))
            .style(Style::default().bg(SURFACE_1));
        let inner_area = modal_block.inner(modal_area);
        modal_block.render(modal_area, buf);

        // Calculate content area with padding
        let content_area = Rect {
            x: inner_area.x + INNER_PADDING,
            y: inner_area.y + 1,
            width: inner_area.width.saturating_sub(INNER_PADDING * 2),
            height: inner_area.height.saturating_sub(2),
        };

        if content_area.width < 10 || content_area.height < 5 {
            return;
        }

        // Layout content sections
        let has_diff = approval.diff_preview.is_some();
        let args_height = calculate_args_height(&approval.tool_args).min(MAX_ARGS_LINES + 2);
        let diff_height = if has_diff { MAX_DIFF_LINES + 2 } else { 0 };

        let constraints = if has_diff {
            vec![
                Constraint::Length(1),           // Title
                Constraint::Length(1),           // Gap
                Constraint::Length(1),           // Tool name
                Constraint::Length(1),           // Gap
                Constraint::Length(args_height), // Arguments
                Constraint::Length(1),           // Gap
                Constraint::Length(diff_height), // Diff preview
                Constraint::Length(1),           // Gap
                Constraint::Length(1),           // Actions
            ]
        } else {
            vec![
                Constraint::Length(1),           // Title
                Constraint::Length(1),           // Gap
                Constraint::Length(1),           // Tool name
                Constraint::Length(1),           // Gap
                Constraint::Length(args_height), // Arguments
                Constraint::Length(1),           // Gap
                Constraint::Length(1),           // Actions
            ]
        };

        let chunks = Layout::vertical(constraints).split(content_area);

        // Render sections
        self.render_title(chunks[0], buf);
        self.render_tool_name(chunks[2], buf, &approval.tool_name);
        self.render_arguments(chunks[4], buf, &approval.tool_args);

        if has_diff {
            if let Some(diff) = &approval.diff_preview {
                self.render_diff_preview(chunks[6], buf, diff);
            }
            self.render_actions(chunks[8], buf);
        } else {
            self.render_actions(chunks[6], buf);
        }
    }
}

// ============================================================
// HELPER FUNCTIONS
// ============================================================

/// Centers a rectangle of given dimensions within an area.
fn center_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

/// Calculates the total height needed for the modal content.
fn calculate_content_height(approval: &ApprovalState) -> u16 {
    let mut height: u16 = 0;

    // Border (top + bottom)
    height += 2;

    // Inner padding
    height += 2;

    // Title + gap
    height += 2;

    // Tool name + gap
    height += 2;

    // Arguments section (label + box with border)
    let args_height = calculate_args_height(&approval.tool_args).min(MAX_ARGS_LINES + 2);
    height += args_height + 1; // +1 for gap

    // Diff preview section (if present)
    if approval.diff_preview.is_some() {
        height += MAX_DIFF_LINES + 2 + 1; // box + border + gap
    }

    // Actions
    height += 1;

    height
}

/// Calculates the height needed for arguments display.
fn calculate_args_height(args: &str) -> u16 {
    let formatted = if let Ok(value) = serde_json::from_str::<serde_json::Value>(args) {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| args.to_string())
    } else {
        args.to_string()
    };
    let line_count = formatted.lines().count() as u16;

    // Add 2 for border (top + bottom) and 1 for label
    (line_count + 3).min(MAX_ARGS_LINES + 3)
}

/// Syntax highlights a JSON string and returns styled lines.
fn syntax_highlight_json(json: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for line in json.lines() {
        let mut spans = Vec::new();
        let chars = line.chars().peekable();
        let mut current_str = String::new();
        let mut in_string = false;
        let mut after_colon = false;

        for ch in chars {
            match ch {
                '"' => {
                    if !current_str.is_empty() {
                        let style = if in_string {
                            if after_colon {
                                Style::default().fg(GREEN) // String value
                            } else {
                                Style::default().fg(BLUE) // Key
                            }
                        } else {
                            Style::default().fg(TEXT)
                        };
                        spans.push(Span::styled(current_str.clone(), style));
                        current_str.clear();
                    }
                    current_str.push(ch);
                    if in_string {
                        // End of string
                        let style = if after_colon {
                            Style::default().fg(GREEN)
                        } else {
                            Style::default().fg(BLUE)
                        };
                        spans.push(Span::styled(current_str.clone(), style));
                        current_str.clear();
                        in_string = false;
                    } else {
                        in_string = true;
                    }
                }
                ':' if !in_string => {
                    if !current_str.is_empty() {
                        spans.push(Span::styled(current_str.clone(), Style::default().fg(TEXT)));
                        current_str.clear();
                    }
                    spans.push(Span::styled(":", Style::default().fg(TEXT_DIM)));
                    after_colon = true;
                }
                ',' | '{' | '}' | '[' | ']' if !in_string => {
                    if !current_str.is_empty() {
                        let style = if after_colon {
                            determine_value_style(&current_str)
                        } else {
                            Style::default().fg(TEXT)
                        };
                        spans.push(Span::styled(current_str.clone(), style));
                        current_str.clear();
                    }
                    spans.push(Span::styled(ch.to_string(), Style::default().fg(TEXT_DIM)));
                    if ch == ',' {
                        after_colon = false;
                    }
                }
                _ => {
                    current_str.push(ch);
                }
            }
        }

        // Handle remaining content
        if !current_str.is_empty() {
            let style = if in_string {
                if after_colon {
                    Style::default().fg(GREEN)
                } else {
                    Style::default().fg(BLUE)
                }
            } else if after_colon {
                determine_value_style(&current_str)
            } else {
                Style::default().fg(TEXT)
            };
            spans.push(Span::styled(current_str, style));
        }

        lines.push(Line::from(spans));
    }

    lines
}

/// Determines the style for a JSON value based on its type.
fn determine_value_style(value: &str) -> Style {
    let trimmed = value.trim();
    if trimmed == "true" || trimmed == "false" {
        Style::default().fg(ORANGE) // Boolean
    } else if trimmed == "null" {
        Style::default().fg(RED) // Null
    } else if trimmed.parse::<f64>().is_ok() {
        Style::default().fg(YELLOW) // Number
    } else {
        Style::default().fg(GREEN) // String or other
    }
}

/// Renders diff content with syntax highlighting.
///
/// Returns styled lines with:
/// - RED for deletions (lines starting with -)
/// - GREEN for additions (lines starting with +)
/// - BLUE for hunk headers (lines starting with @@)
/// - TEXT for context lines
fn render_diff_lines(diff: &str, max_lines: usize) -> Vec<Line<'static>> {
    diff.lines()
        .take(max_lines)
        .map(|line| {
            let (style, prefix_style) = if line.starts_with("@@") {
                // Hunk header
                (Style::default().fg(BLUE), Style::default().fg(BLUE))
            } else if line.starts_with('+') && !line.starts_with("+++") {
                // Addition
                (
                    Style::default().fg(GREEN),
                    Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
                )
            } else if line.starts_with('-') && !line.starts_with("---") {
                // Deletion
                (
                    Style::default().fg(RED),
                    Style::default().fg(RED).add_modifier(Modifier::BOLD),
                )
            } else if line.starts_with("+++") || line.starts_with("---") {
                // File headers
                (Style::default().fg(TEXT_DIM), Style::default().fg(TEXT_DIM))
            } else {
                // Context line
                (Style::default().fg(TEXT), Style::default().fg(TEXT_DIM))
            };

            // Split the prefix character from the rest of the line for different styling
            if !line.is_empty()
                && (line.starts_with('+') || line.starts_with('-') || line.starts_with(' '))
            {
                let prefix = &line[..1];
                let rest = &line[1..];
                Line::from(vec![
                    Span::styled(prefix.to_string(), prefix_style),
                    Span::styled(rest.to_string(), style),
                ])
            } else {
                Line::from(Span::styled(line.to_string(), style))
            }
        })
        .collect()
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppState, ApprovalState};

    fn create_test_state() -> AppState {
        let mut state = AppState::new();
        state.pending_approval = Some(ApprovalState::new(
            "write_file".to_string(),
            serde_json::json!({
                "path": "src/main.rs",
                "content": "fn main() { println!(\"Hello, world!\"); }"
            }),
        ));
        state
    }

    #[test]
    fn test_approval_view_creation() {
        let state = create_test_state();
        let _view = ApprovalView::new(&state);
    }

    #[test]
    fn test_center_rect() {
        let area = Rect::new(0, 0, 100, 50);
        let centered = center_rect(50, 20, area);
        assert_eq!(centered.x, 25);
        assert_eq!(centered.y, 15);
        assert_eq!(centered.width, 50);
        assert_eq!(centered.height, 20);
    }

    #[test]
    fn test_center_rect_larger_than_area() {
        let area = Rect::new(0, 0, 40, 20);
        let centered = center_rect(100, 50, area);
        assert_eq!(centered.x, 0);
        assert_eq!(centered.y, 0);
        assert_eq!(centered.width, 40);
        assert_eq!(centered.height, 20);
    }

    #[test]
    fn test_calculate_args_height() {
        let simple_args = serde_json::json!({"path": "test.txt"});
        let height = calculate_args_height(&simple_args.to_string());
        assert!(height >= 3); // At least label + border + 1 line

        let complex_args = serde_json::json!({
            "path": "test.txt",
            "content": "line1\nline2\nline3",
            "mode": "write"
        });
        let height = calculate_args_height(&complex_args.to_string());
        assert!(height >= 5);
    }

    #[test]
    fn test_calculate_content_height() {
        let approval = ApprovalState::new(
            "write_file".to_string(),
            serde_json::json!({"path": "test.txt"}),
        );
        let height = calculate_content_height(&approval);
        assert!(height > 10);

        let approval_with_diff = ApprovalState::new(
            "write_file".to_string(),
            serde_json::json!({"path": "test.txt"}),
        )
        .with_diff("-old\n+new".to_string());
        let height_with_diff = calculate_content_height(&approval_with_diff);
        assert!(height_with_diff > height);
    }

    #[test]
    fn test_syntax_highlight_json() {
        let json = r#"{"key": "value", "number": 42}"#;
        let lines = syntax_highlight_json(json);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_syntax_highlight_json_multiline() {
        let json = r#"{
  "path": "src/main.rs",
  "content": "fn main() {}"
}"#;
        let lines = syntax_highlight_json(json);
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn test_determine_value_style() {
        // Boolean
        let style = determine_value_style("true");
        assert_eq!(style.fg, Some(ORANGE));

        let style = determine_value_style("false");
        assert_eq!(style.fg, Some(ORANGE));

        // Null
        let style = determine_value_style("null");
        assert_eq!(style.fg, Some(RED));

        // Number
        let style = determine_value_style("42");
        assert_eq!(style.fg, Some(YELLOW));

        let style = determine_value_style("3.14");
        assert_eq!(style.fg, Some(YELLOW));
    }

    #[test]
    fn test_approval_view_render_no_panic() {
        let state = create_test_state();
        let view = ApprovalView::new(&state);
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_approval_view_render_small_area() {
        let state = create_test_state();
        let view = ApprovalView::new(&state);
        let area = Rect::new(0, 0, 40, 15);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_approval_view_with_diff() {
        let mut state = AppState::new();
        state.pending_approval = Some(
            ApprovalState::new(
                "edit_file".to_string(),
                serde_json::json!({"path": "test.rs"}),
            )
            .with_diff("@@ -1,3 +1,4 @@\n fn main() {\n-    old();\n+    new();\n }".to_string()),
        );
        let view = ApprovalView::new(&state);
        let area = Rect::new(0, 0, 100, 50);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_approval_view_no_pending() {
        let state = AppState::new(); // No pending approval
        let view = ApprovalView::new(&state);
        let area = Rect::new(0, 0, 80, 30);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        // Should not panic and not render anything
    }

    #[test]
    fn test_render_diff_lines() {
        let diff = "@@ -1,3 +1,4 @@\n fn main() {\n-    old();\n+    new();\n }";
        let lines = render_diff_lines(diff, 10);
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn test_render_diff_lines_max_lines() {
        let diff = "+line1\n+line2\n+line3\n+line4\n+line5";
        let lines = render_diff_lines(diff, 3);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_render_diff_lines_empty() {
        let diff = "";
        let lines = render_diff_lines(diff, 10);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_render_diff_lines_file_headers() {
        let diff = "--- a/file.rs\n+++ b/file.rs\n@@ -1,1 +1,1 @@\n-old\n+new";
        let lines = render_diff_lines(diff, 10);
        assert_eq!(lines.len(), 5);
    }
}
