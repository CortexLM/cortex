//! Trust Screen TUI
//!
//! Security prompt shown before accessing a workspace for the first time.

use std::io::stdout;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use cortex_core::style::{CYAN_PRIMARY, TEXT, TEXT_DIM, TEXT_MUTED};
use cortex_tui_components::mascot::MASCOT;

// ============================================================================
// Trust Result
// ============================================================================

/// Result of the trust screen interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustResult {
    /// User trusts the workspace.
    Trusted,
    /// User rejected/exited.
    Rejected,
}

// ============================================================================
// Trust Screen
// ============================================================================

/// Security trust verification screen.
pub struct TrustScreen {
    workspace_path: PathBuf,
    selected: usize,
}

impl TrustScreen {
    /// Create a new trust screen for the given workspace.
    pub fn new(workspace_path: PathBuf) -> Self {
        Self {
            workspace_path,
            selected: 0,
        }
    }

    /// Run the trust screen and return the user's decision.
    pub async fn run(&mut self) -> Result<TrustResult> {
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = stdout();
        crossterm::execute!(
            stdout,
            crossterm::terminal::EnterAlternateScreen,
            crossterm::event::EnableMouseCapture,
        )?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let result = self.run_loop(&mut terminal).await;

        // Cleanup
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(
            terminal.backend_mut(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
        )?;
        terminal.show_cursor()?;

        result
    }

    async fn run_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<TrustResult> {
        loop {
            terminal.draw(|f| self.render(f))?;

            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && let Some(result) = self.handle_key(key)
            {
                return Ok(result);
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<TrustResult> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected < 1 {
                    self.selected += 1;
                }
                None
            }
            KeyCode::Char('1') => {
                self.selected = 0;
                Some(TrustResult::Trusted)
            }
            KeyCode::Char('2') => {
                self.selected = 1;
                Some(TrustResult::Rejected)
            }
            KeyCode::Enter => {
                if self.selected == 0 {
                    Some(TrustResult::Trusted)
                } else {
                    Some(TrustResult::Rejected)
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => Some(TrustResult::Rejected),
            _ => None,
        }
    }

    fn render(&self, f: &mut ratatui::Frame) {
        let area = f.area();
        f.render_widget(Clear, area);

        // Calculate content area with some padding
        let content_width = 80.min(area.width.saturating_sub(4));
        let content_x = (area.width.saturating_sub(content_width)) / 2;
        let content_area = Rect::new(content_x, 0, content_width, area.height);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Top separator
                Constraint::Length(1), // Spacing
                Constraint::Length(5), // Mascot
                Constraint::Length(1), // Spacing
                Constraint::Length(4), // Description text
                Constraint::Length(1), // Spacing
                Constraint::Length(2), // Permissions text
                Constraint::Length(1), // Spacing
                Constraint::Length(1), // Security link
                Constraint::Length(2), // Spacing
                Constraint::Length(2), // Options
                Constraint::Min(1),    // Flex
                Constraint::Length(1), // Hints
            ])
            .split(content_area);

        // Top separator line
        let separator =
            Paragraph::new("─".repeat(content_width as usize)).style(Style::default().fg(TEXT_DIM));
        f.render_widget(separator, chunks[0]);

        // Mascot with title
        let path_display = self.workspace_path.display().to_string();
        let mascot_lines: Vec<Line> = MASCOT
            .lines()
            .enumerate()
            .map(|(i, line)| {
                let suffix = match i {
                    1 => Span::styled("  Workspace Access", Style::default().fg(CYAN_PRIMARY)),
                    3 => Span::styled(
                        format!("  {}", truncate_path(&path_display, 50)),
                        Style::default().fg(TEXT),
                    ),
                    _ => Span::raw(""),
                };
                Line::from(vec![Span::styled(line, Style::default().fg(TEXT)), suffix])
            })
            .collect();
        let mascot_widget = Paragraph::new(mascot_lines);
        f.render_widget(mascot_widget, chunks[2]);

        // Description text
        let desc_lines = vec![
            Line::from(Span::styled(
                " Before continuing, please confirm you trust this workspace.",
                Style::default().fg(TEXT_MUTED),
            )),
            Line::from(Span::styled(
                " This should be your own project, a verified repository, or code from collaborators.",
                Style::default().fg(TEXT_MUTED),
            )),
            Line::from(""),
            Line::from(Span::styled(
                " For unfamiliar directories, review the contents before granting access.",
                Style::default().fg(TEXT_MUTED),
            )),
        ];
        f.render_widget(Paragraph::new(desc_lines), chunks[4]);

        // Permissions text
        let perm_lines = vec![Line::from(Span::styled(
            " Cortex requires permission to read, modify, and run commands in this directory.",
            Style::default().fg(TEXT_MUTED),
        ))];
        f.render_widget(Paragraph::new(perm_lines), chunks[6]);

        // Security link
        let link = Line::from(vec![
            Span::styled(" Learn more: ", Style::default().fg(TEXT_MUTED)),
            Span::styled(
                "https://cortex.dev/docs/security",
                Style::default()
                    .fg(CYAN_PRIMARY)
                    .add_modifier(Modifier::UNDERLINED),
            ),
        ]);
        f.render_widget(Paragraph::new(vec![link]), chunks[8]);

        // Options
        let opt1_style = if self.selected == 0 {
            Style::default().fg(CYAN_PRIMARY)
        } else {
            Style::default().fg(TEXT_DIM)
        };
        let opt2_style = if self.selected == 1 {
            Style::default().fg(CYAN_PRIMARY)
        } else {
            Style::default().fg(TEXT_DIM)
        };

        let opt1_prefix = if self.selected == 0 { " › " } else { "   " };
        let opt2_prefix = if self.selected == 1 { " › " } else { "   " };

        let options = vec![
            Line::from(Span::styled(
                format!("{}1. Yes, I trust this folder", opt1_prefix),
                opt1_style,
            )),
            Line::from(Span::styled(
                format!("{}2. No, exit", opt2_prefix),
                opt2_style,
            )),
        ];
        f.render_widget(Paragraph::new(options), chunks[10]);

        // Hints at bottom
        let hints = Line::from(Span::styled(
            "Enter to confirm · Esc to exit",
            Style::default().fg(TEXT_MUTED),
        ));
        f.render_widget(
            Paragraph::new(vec![hints]).alignment(Alignment::Right),
            chunks[12],
        );
    }
}

/// Truncate a path string to fit within max_len.
fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        path.to_string()
    } else if max_len > 5 {
        format!("...{}", &path[path.len() - (max_len - 3)..])
    } else {
        path.chars().take(max_len).collect()
    }
}
