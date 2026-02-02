//! Theme Selector Modal
//!
//! A modal for selecting the application theme (dark, light, ocean_dark, monokai).

use cortex_core::style::{CYAN_PRIMARY, SURFACE_0, TEXT, TEXT_DIM, VOID};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::widgets::ActionBar;

use super::{CancelBehavior, Modal, ModalAction, ModalResult};

// ============================================================================
// THEME DEFINITIONS
// ============================================================================

/// Theme definition with display info.
struct ThemeDef {
    id: &'static str,
    label: &'static str,
    description: &'static str,
}

/// Available themes in the application.
const THEMES: &[ThemeDef] = &[
    ThemeDef {
        id: "dark",
        label: "Dark",
        description: "Default dark theme with green accents",
    },
    ThemeDef {
        id: "light",
        label: "Light",
        description: "Light theme with dark text",
    },
    ThemeDef {
        id: "ocean_dark",
        label: "Ocean Dark",
        description: "Deep blue/cyan aesthetic",
    },
    ThemeDef {
        id: "monokai",
        label: "Monokai",
        description: "Classic code editor colors",
    },
];

// ============================================================================
// THEME SELECTOR MODAL
// ============================================================================

/// A modal for selecting the application theme.
pub struct ThemeSelectorModal {
    /// Index of the currently selected theme in the list.
    selected_index: usize,
    /// Current theme name for highlighting.
    current_theme: String,
}

impl ThemeSelectorModal {
    /// Create a new ThemeSelectorModal.
    ///
    /// The modal pre-selects the current theme so users can see which theme is active.
    pub fn new(current_theme: &str) -> Self {
        // Find the current theme's index to pre-select it
        let selected_index = THEMES
            .iter()
            .position(|t| t.id == current_theme)
            .unwrap_or(0);

        Self {
            selected_index,
            current_theme: current_theme.to_string(),
        }
    }

    /// Get the currently selected theme ID.
    fn selected_theme_id(&self) -> Option<&'static str> {
        THEMES.get(self.selected_index).map(|t| t.id)
    }

    /// Build the action bar.
    fn build_action_bar(&self) -> ActionBar {
        ActionBar::new().with_standard_hints()
    }

    /// Navigate up in the list.
    fn navigate_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Navigate down in the list.
    fn navigate_down(&mut self) {
        if self.selected_index < THEMES.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    /// Render a theme row.
    fn render_theme_row(
        &self,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        theme: &ThemeDef,
        is_selected: bool,
    ) {
        let is_current = theme.id == self.current_theme;

        let (bg, fg, marker_fg) = if is_selected {
            (CYAN_PRIMARY, VOID, VOID)
        } else {
            (
                SURFACE_0,
                TEXT,
                if is_current { CYAN_PRIMARY } else { TEXT_DIM },
            )
        };

        // Clear line with background
        for col in x..x.saturating_add(width) {
            buf[(col, y)].set_bg(bg);
        }

        let mut col = x + 1;

        // Current marker
        let marker = if is_current { "●" } else { " " };
        buf.set_string(col, y, marker, Style::default().fg(marker_fg).bg(bg));
        col += 2;

        // Theme label
        buf.set_string(col, y, theme.label, Style::default().fg(fg).bg(bg));
        col += theme.label.len() as u16 + 2;

        // Description
        let desc_style = if is_selected {
            Style::default().fg(VOID).bg(bg)
        } else {
            Style::default().fg(TEXT_DIM).bg(bg)
        };

        let max_desc_len = width.saturating_sub(col - x + 2) as usize;
        let desc = if theme.description.len() > max_desc_len && max_desc_len > 3 {
            format!(
                "{}...",
                &theme.description[..max_desc_len.saturating_sub(3)]
            )
        } else {
            theme.description.to_string()
        };
        buf.set_string(col, y, &desc, desc_style);
    }
}

// ============================================================================
// MODAL IMPLEMENTATION
// ============================================================================

impl Modal for ThemeSelectorModal {
    fn title(&self) -> &str {
        "Select Theme"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        let content_height = THEMES.len() as u16 + 3; // themes + title + action bar + padding
        content_height.clamp(6, 12).min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 10 {
            return;
        }

        // Layout: themes in middle, action bar at bottom
        let action_bar_height = 1_u16;
        let content_height = area.height.saturating_sub(action_bar_height);
        let content_area = Rect::new(area.x, area.y, area.width, content_height);
        let action_area = Rect::new(
            area.x,
            area.y + content_height,
            area.width,
            action_bar_height,
        );

        // Render themes
        let mut y = content_area.y;

        for (idx, theme) in THEMES.iter().enumerate() {
            if y >= content_area.bottom() {
                break;
            }
            let is_selected = self.selected_index == idx;
            self.render_theme_row(
                content_area.x,
                y,
                content_area.width,
                buf,
                theme,
                is_selected,
            );
            y += 1;
        }

        // Render action bar
        let action_bar = self.build_action_bar();
        (&action_bar).render(action_area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match key.code {
            KeyCode::Esc => ModalResult::Close,
            KeyCode::Enter => {
                if let Some(theme_id) = self.selected_theme_id() {
                    ModalResult::Action(ModalAction::Custom(format!("theme:{}", theme_id)))
                } else {
                    ModalResult::Close
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.navigate_up();
                ModalResult::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.navigate_down();
                ModalResult::Continue
            }
            _ => ModalResult::Continue,
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("↑↓", "navigate"), ("Enter", "select"), ("Esc", "cancel")]
    }

    fn on_cancel(&mut self) -> CancelBehavior {
        CancelBehavior::Close
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn test_theme_selector_new() {
        let modal = ThemeSelectorModal::new("dark");
        assert_eq!(modal.title(), "Select Theme");
        assert_eq!(modal.current_theme, "dark");
    }

    #[test]
    fn test_selected_theme_default() {
        let modal = ThemeSelectorModal::new("dark");
        assert_eq!(modal.selected_theme_id(), Some("dark"));
    }

    #[test]
    fn test_preselects_current_theme() {
        let modal = ThemeSelectorModal::new("monokai");
        assert_eq!(modal.selected_index, 3);
        assert_eq!(modal.selected_theme_id(), Some("monokai"));
    }

    #[test]
    fn test_unknown_theme_defaults_to_first() {
        let modal = ThemeSelectorModal::new("nonexistent");
        assert_eq!(modal.selected_index, 0);
        assert_eq!(modal.selected_theme_id(), Some("dark"));
    }

    #[test]
    fn test_key_hints() {
        let modal = ThemeSelectorModal::new("dark");
        let hints = modal.key_hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|(k, _)| *k == "Enter"));
        assert!(hints.iter().any(|(k, _)| *k == "Esc"));
    }

    #[test]
    fn test_enter_returns_action() {
        let mut modal = ThemeSelectorModal::new("dark");
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = modal.handle_key(key);

        if let ModalResult::Action(ModalAction::Custom(action)) = result {
            assert_eq!(action, "theme:dark");
        } else {
            panic!("Expected Custom action");
        }
    }

    #[test]
    fn test_escape_closes() {
        let mut modal = ThemeSelectorModal::new("dark");
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = modal.handle_key(key);
        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_navigate_down() {
        let mut modal = ThemeSelectorModal::new("dark");
        assert_eq!(modal.selected_index, 0);

        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        modal.handle_key(down);
        assert_eq!(modal.selected_index, 1);
        assert_eq!(modal.selected_theme_id(), Some("light"));
    }

    #[test]
    fn test_navigate_up() {
        let mut modal = ThemeSelectorModal::new("light");
        assert_eq!(modal.selected_index, 1);

        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        modal.handle_key(up);
        assert_eq!(modal.selected_index, 0);
        assert_eq!(modal.selected_theme_id(), Some("dark"));
    }

    #[test]
    fn test_navigate_up_at_top() {
        let mut modal = ThemeSelectorModal::new("dark");
        assert_eq!(modal.selected_index, 0);

        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        modal.handle_key(up);
        assert_eq!(modal.selected_index, 0); // Should stay at 0
    }

    #[test]
    fn test_navigate_down_at_bottom() {
        let mut modal = ThemeSelectorModal::new("monokai");
        assert_eq!(modal.selected_index, 3);

        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        modal.handle_key(down);
        assert_eq!(modal.selected_index, 3); // Should stay at 3
    }

    #[test]
    fn test_vim_navigation() {
        let mut modal = ThemeSelectorModal::new("dark");

        // j moves down
        let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        modal.handle_key(j);
        assert_eq!(modal.selected_index, 1);

        // k moves up
        let k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        modal.handle_key(k);
        assert_eq!(modal.selected_index, 0);
    }

    #[test]
    fn test_select_different_theme() {
        let mut modal = ThemeSelectorModal::new("dark");

        // Navigate to ocean_dark (index 2)
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        modal.handle_key(down);
        modal.handle_key(down);
        assert_eq!(modal.selected_theme_id(), Some("ocean_dark"));

        // Select it
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = modal.handle_key(enter);

        if let ModalResult::Action(ModalAction::Custom(action)) = result {
            assert_eq!(action, "theme:ocean_dark");
        } else {
            panic!("Expected Custom action with ocean_dark theme");
        }
    }

    #[test]
    fn test_on_cancel() {
        let mut modal = ThemeSelectorModal::new("dark");
        let behavior = modal.on_cancel();
        assert_eq!(behavior, CancelBehavior::Close);
    }

    #[test]
    fn test_desired_height() {
        let modal = ThemeSelectorModal::new("dark");

        // 4 themes + 3 (title + action bar + padding) = 7
        let height = modal.desired_height(20, 80);
        assert!(height >= 6);
        assert!(height <= 12);
    }
}
