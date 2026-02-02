//! Question Prompt View
//!
//! A TUI for asking users questions with:
//! - Tabs for multiple questions
//! - Mouse hover and click support
//! - Keyboard navigation (↑↓, 1-9, Enter, Esc)
//! - Checkboxes for multi-select
//! - Custom text input

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::question::{QuestionState, QuestionType};
use crate::ui::colors::AdaptiveColors;

// ============================================================
// QUESTION PROMPT VIEW
// ============================================================

/// Widget for rendering the question prompt
pub struct QuestionPromptView<'a> {
    state: &'a QuestionState,
    /// Hovered option index (from mouse)
    hovered_option: Option<usize>,
    /// Hovered tab index (from mouse)
    hovered_tab: Option<usize>,
    /// Color palette for rendering
    colors: AdaptiveColors,
}

impl<'a> QuestionPromptView<'a> {
    pub fn new(state: &'a QuestionState) -> Self {
        Self {
            state,
            hovered_option: None,
            hovered_tab: None,
            colors: AdaptiveColors::default(),
        }
    }

    pub fn with_hovered_option(mut self, index: Option<usize>) -> Self {
        self.hovered_option = index;
        self
    }

    pub fn with_hovered_tab(mut self, index: Option<usize>) -> Self {
        self.hovered_tab = index;
        self
    }

    /// Set the colors to use for rendering
    pub fn with_colors(mut self, colors: AdaptiveColors) -> Self {
        self.colors = colors;
        self
    }

    /// Calculate the areas for different parts of the UI
    fn calculate_layout(&self, area: Rect) -> QuestionLayout {
        // Modal dimensions - take up most of the screen but leave margins
        let modal_width = (area.width as f32 * 0.85).min(100.0) as u16;
        let modal_height = (area.height as f32 * 0.8).min(30.0) as u16;

        let modal_x = (area.width.saturating_sub(modal_width)) / 2;
        let modal_y = (area.height.saturating_sub(modal_height)) / 2;

        let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

        // Inner area (inside border)
        let inner = Rect::new(
            modal_area.x + 1,
            modal_area.y + 1,
            modal_area.width.saturating_sub(2),
            modal_area.height.saturating_sub(2),
        );

        // Layout: tabs, title, options, hints
        let chunks = Layout::vertical([
            Constraint::Length(if self.state.is_single_question() {
                0
            } else {
                2
            }), // Tabs
            Constraint::Length(2), // Title/Question
            Constraint::Min(5),    // Options
            Constraint::Length(2), // Hints
        ])
        .split(inner);

        QuestionLayout {
            modal_area,
            tabs_area: chunks[0],
            title_area: chunks[1],
            options_area: chunks[2],
            hints_area: chunks[3],
        }
    }
}

struct QuestionLayout {
    modal_area: Rect,
    tabs_area: Rect,
    title_area: Rect,
    options_area: Rect,
    hints_area: Rect,
}

impl Widget for QuestionPromptView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let colors = &self.colors;
        let layout = self.calculate_layout(area);

        // Clear the modal area
        Clear.render(layout.modal_area, buf);

        // Draw border
        let border_style = Style::default().fg(colors.accent);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(
                format!(" {} ", self.state.request.title),
                Style::default()
                    .fg(colors.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        block.render(layout.modal_area, buf);

        // Render tabs (if multiple questions)
        if !self.state.is_single_question() {
            self.render_tabs(&layout.tabs_area, buf, colors);
        }

        // Render question or confirm screen
        if self.state.on_confirm_tab {
            self.render_confirm(&layout.title_area, &layout.options_area, buf, colors);
        } else {
            self.render_question(&layout.title_area, &layout.options_area, buf, colors);
        }

        // Render hints
        self.render_hints(&layout.hints_area, buf, colors);
    }
}

impl QuestionPromptView<'_> {
    fn render_tabs(&self, area: &Rect, buf: &mut Buffer, colors: &AdaptiveColors) {
        let mut x = area.x + 1;

        for (i, _q) in self.state.request.questions.iter().enumerate() {
            let header = self.state.get_header(i);
            let is_active = !self.state.on_confirm_tab && self.state.current_tab == i;
            let is_hovered = self.hovered_tab == Some(i);
            let is_answered = !self.state.answers[i].is_empty();

            let (fg, bg) = if is_active {
                (colors.text, colors.accent)
            } else if is_hovered {
                (colors.text, colors.user_bg)
            } else {
                (
                    if is_answered {
                        colors.success
                    } else {
                        colors.text_muted
                    },
                    Color::Reset,
                )
            };

            let tab_width = header.len() as u16 + 2;
            if x + tab_width < area.x + area.width {
                // Render tab background
                for dx in 0..tab_width {
                    if let Some(cell) = buf.cell_mut((x + dx, area.y)) {
                        cell.set_bg(bg);
                    }
                }

                // Render tab text
                buf.set_string(x + 1, area.y, &header, Style::default().fg(fg).bg(bg));
                x += tab_width + 1;
            }
        }

        // Confirm tab
        if !self.state.is_single_question() {
            let is_active = self.state.on_confirm_tab;
            let is_hovered = self.hovered_tab == Some(self.state.request.questions.len());

            let (fg, bg) = if is_active {
                (colors.text, colors.accent)
            } else if is_hovered {
                (colors.text, colors.user_bg)
            } else {
                (colors.text_muted, Color::Reset)
            };

            let label = "Confirm";
            let tab_width = label.len() as u16 + 2;
            if x + tab_width < area.x + area.width {
                for dx in 0..tab_width {
                    if let Some(cell) = buf.cell_mut((x + dx, area.y)) {
                        cell.set_bg(bg);
                    }
                }
                buf.set_string(x + 1, area.y, label, Style::default().fg(fg).bg(bg));
            }
        }
    }

    fn render_question(
        &self,
        title_area: &Rect,
        options_area: &Rect,
        buf: &mut Buffer,
        colors: &AdaptiveColors,
    ) {
        let Some(question) = self.state.current_question() else {
            return;
        };

        // Render question text
        let multi_hint = if question.question_type == QuestionType::Multiple {
            " (select all that apply)"
        } else {
            ""
        };

        let question_text = format!("{}{}", question.question, multi_hint);
        let question_line = Line::from(Span::styled(
            question_text,
            Style::default().fg(colors.text),
        ));
        Paragraph::new(question_line).render(*title_area, buf);

        // Render options
        let current_tab = self.state.current_tab;
        let selected_idx = self.state.selected_index[current_tab];
        let is_multi = question.question_type == QuestionType::Multiple;

        let mut y = options_area.y;

        for (i, opt) in question.options.iter().enumerate() {
            if y >= options_area.y + options_area.height {
                break;
            }

            let is_selected = selected_idx == i;
            let is_hovered = self.hovered_option == Some(i);
            let is_picked = self.state.is_answer_selected(current_tab, &opt.label);

            // Option line
            let checkbox = if is_multi {
                if is_picked { "[✓]" } else { "[ ]" }
            } else {
                ""
            };

            let prefix = format!("{}. {}", i + 1, checkbox);
            let label = &opt.label;

            let (fg, bg) = if is_selected || is_hovered {
                (colors.accent, colors.user_bg)
            } else if is_picked {
                (colors.success, Color::Reset)
            } else {
                (colors.text, Color::Reset)
            };

            // Highlight background if selected/hovered
            if is_selected || is_hovered {
                for dx in 0..options_area.width {
                    if let Some(cell) = buf.cell_mut((options_area.x + dx, y)) {
                        cell.set_bg(bg);
                    }
                }
            }

            // Draw prefix and label
            let text = format!("{} {}", prefix, label);
            buf.set_string(options_area.x + 1, y, &text, Style::default().fg(fg).bg(bg));

            // Draw checkmark for single select
            if !is_multi && is_picked {
                let check_x = options_area.x + 1 + text.len() as u16 + 1;
                if check_x < options_area.x + options_area.width {
                    buf.set_string(check_x, y, "✓", Style::default().fg(colors.success).bg(bg));
                }
            }

            y += 1;

            // Description
            if let Some(desc) = &opt.description
                && y < options_area.y + options_area.height
            {
                buf.set_string(
                    options_area.x + 4,
                    y,
                    desc,
                    Style::default().fg(colors.text_muted),
                );
                y += 1;
            }
        }

        // Custom input option
        if question.allow_custom {
            let custom_idx = question.options.len();
            let is_selected = selected_idx == custom_idx;
            let is_hovered = self.hovered_option == Some(custom_idx);
            let custom_text = &self.state.custom_input[current_tab];
            let is_picked = !custom_text.is_empty() && self.state.is_custom_selected(current_tab);

            if y < options_area.y + options_area.height {
                let checkbox = if is_multi {
                    if is_picked { "[✓]" } else { "[ ]" }
                } else {
                    ""
                };

                let prefix = format!("{}. {}", custom_idx + 1, checkbox);
                let label = "Type your own answer";

                let (fg, bg) = if is_selected || is_hovered {
                    (colors.accent, colors.user_bg)
                } else if is_picked {
                    (colors.success, Color::Reset)
                } else {
                    (colors.text, Color::Reset)
                };

                if is_selected || is_hovered {
                    for dx in 0..options_area.width {
                        if let Some(cell) = buf.cell_mut((options_area.x + dx, y)) {
                            cell.set_bg(bg);
                        }
                    }
                }

                let text = format!("{} {}", prefix, label);
                buf.set_string(options_area.x + 1, y, &text, Style::default().fg(fg).bg(bg));

                if !is_multi && is_picked {
                    let check_x = options_area.x + 1 + text.len() as u16 + 1;
                    if check_x < options_area.x + options_area.width {
                        buf.set_string(check_x, y, "✓", Style::default().fg(colors.success).bg(bg));
                    }
                }

                y += 1;

                // Show editing input or current value
                if y < options_area.y + options_area.height {
                    if self.state.editing_custom {
                        let input_text = format!("› {}_", self.state.current_custom_text);
                        buf.set_string(
                            options_area.x + 4,
                            y,
                            &input_text,
                            Style::default().fg(colors.accent),
                        );
                    } else if !custom_text.is_empty() {
                        buf.set_string(
                            options_area.x + 4,
                            y,
                            custom_text,
                            Style::default().fg(colors.text_muted),
                        );
                    }
                }
            }
        }
    }

    fn render_confirm(
        &self,
        title_area: &Rect,
        options_area: &Rect,
        buf: &mut Buffer,
        colors: &AdaptiveColors,
    ) {
        // Title
        let title = Line::from(Span::styled(
            "Review your answers",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        ));
        Paragraph::new(title).render(*title_area, buf);

        // Answers summary
        let mut y = options_area.y;

        for (i, _q) in self.state.request.questions.iter().enumerate() {
            if y >= options_area.y + options_area.height {
                break;
            }

            let header = self.state.get_header(i);
            let answers = &self.state.answers[i];
            let answer_text = if answers.is_empty() {
                "(not answered)".to_string()
            } else {
                answers.join(", ")
            };

            let is_answered = !answers.is_empty();

            let line = Line::from(vec![
                Span::styled(
                    format!("{}: ", header),
                    Style::default().fg(colors.text_muted),
                ),
                Span::styled(
                    answer_text,
                    Style::default().fg(if is_answered {
                        colors.text
                    } else {
                        colors.error
                    }),
                ),
            ]);

            buf.set_line(
                options_area.x + 1,
                y,
                &line,
                options_area.width.saturating_sub(2),
            );
            y += 1;
        }
    }

    fn render_hints(&self, area: &Rect, buf: &mut Buffer, colors: &AdaptiveColors) {
        let hints = if self.state.editing_custom {
            vec![("Enter", "confirm"), ("Esc", "cancel")]
        } else if self.state.on_confirm_tab {
            vec![("←→", "tab"), ("Enter", "submit"), ("Esc", "dismiss")]
        } else {
            let is_multi = self
                .state
                .current_question()
                .map(|q| q.question_type == QuestionType::Multiple)
                .unwrap_or(false);

            let mut h = vec![("↑↓", "select")];

            if !self.state.is_single_question() {
                h.insert(0, ("←→", "tab"));
            }

            h.push(("Enter", if is_multi { "toggle" } else { "choose" }));
            h.push(("Esc", "dismiss"));

            h
        };

        let hint_spans: Vec<Span> = hints
            .iter()
            .flat_map(|(key, desc)| {
                vec![
                    Span::styled(*key, Style::default().fg(colors.text)),
                    Span::styled(" ", Style::default()),
                    Span::styled(*desc, Style::default().fg(colors.text_muted)),
                    Span::styled("  ", Style::default()),
                ]
            })
            .collect();

        let line = Line::from(hint_spans);
        buf.set_line(area.x + 1, area.y, &line, area.width.saturating_sub(2));
    }
}

// ============================================================
// CLICK ZONES FOR MOUSE SUPPORT
// ============================================================

/// Information about clickable areas in the question prompt
#[derive(Debug, Clone)]
pub struct QuestionClickZones {
    pub tabs: Vec<(Rect, usize)>,    // (area, tab_index)
    pub options: Vec<(Rect, usize)>, // (area, option_index)
    pub confirm_button: Option<Rect>,
}

impl QuestionClickZones {
    /// Calculate click zones based on the current state and area
    pub fn calculate(state: &QuestionState, area: Rect) -> Self {
        let view = QuestionPromptView::new(state);
        let layout = view.calculate_layout(area);

        let mut tabs = Vec::new();
        let mut options = Vec::new();
        let confirm_button = None;

        // Calculate tab zones
        if !state.is_single_question() {
            let mut x = layout.tabs_area.x + 1;

            for (i, _q) in state.request.questions.iter().enumerate() {
                let header = state.get_header(i);
                let tab_width = header.len() as u16 + 2;

                if x + tab_width < layout.tabs_area.x + layout.tabs_area.width {
                    tabs.push((Rect::new(x, layout.tabs_area.y, tab_width, 1), i));
                    x += tab_width + 1;
                }
            }

            // Confirm tab
            let label = "Confirm";
            let tab_width = label.len() as u16 + 2;
            if x + tab_width < layout.tabs_area.x + layout.tabs_area.width {
                tabs.push((
                    Rect::new(x, layout.tabs_area.y, tab_width, 1),
                    state.request.questions.len(),
                ));
            }
        }

        // Calculate option zones
        if !state.on_confirm_tab
            && let Some(question) = state.current_question()
        {
            let mut y = layout.options_area.y;

            for i in 0..question.options.len() {
                if y >= layout.options_area.y + layout.options_area.height {
                    break;
                }

                options.push((
                    Rect::new(layout.options_area.x, y, layout.options_area.width, 1),
                    i,
                ));

                // Skip description line
                if question.options[i].description.is_some() {
                    y += 2;
                } else {
                    y += 1;
                }
            }

            // Custom option
            if question.allow_custom && y < layout.options_area.y + layout.options_area.height {
                options.push((
                    Rect::new(layout.options_area.x, y, layout.options_area.width, 1),
                    question.options.len(),
                ));
            }
        }

        QuestionClickZones {
            tabs,
            options,
            confirm_button,
        }
    }

    /// Find which element is at the given position
    pub fn hit_test(&self, x: u16, y: u16) -> QuestionHit {
        // Check tabs
        for (rect, idx) in &self.tabs {
            if rect.contains((x, y).into()) {
                return QuestionHit::Tab(*idx);
            }
        }

        // Check options
        for (rect, idx) in &self.options {
            if rect.contains((x, y).into()) {
                return QuestionHit::Option(*idx);
            }
        }

        // Check confirm button
        if let Some(rect) = &self.confirm_button
            && rect.contains((x, y).into())
        {
            return QuestionHit::Confirm;
        }

        QuestionHit::None
    }
}

/// Result of a click zone hit test
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionHit {
    None,
    Tab(usize),
    Option(usize),
    Confirm,
}
