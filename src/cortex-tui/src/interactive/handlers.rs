//! Keyboard handlers for interactive mode.

use super::state::{InteractiveAction, InteractiveResult, InteractiveState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

/// Handle a key event in interactive mode.
///
/// Returns an `InteractiveResult` indicating what action to take.
pub fn handle_interactive_key(state: &mut InteractiveState, key: KeyEvent) -> InteractiveResult {
    // If inline form is active, handle form input
    if state.is_form_active() {
        return handle_form_key(state, key);
    }

    match key.code {
        // Navigation
        KeyCode::Up | KeyCode::Char('k')
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::NONE =>
        {
            state.select_prev();
            InteractiveResult::Continue
        }
        KeyCode::Down | KeyCode::Char('j')
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::NONE =>
        {
            state.select_next();
            InteractiveResult::Continue
        }

        // Ctrl+P / Ctrl+N for navigation (like emacs)
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
            state.select_prev();
            InteractiveResult::Continue
        }
        KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
            state.select_next();
            InteractiveResult::Continue
        }

        // Page navigation
        KeyCode::PageUp => {
            for _ in 0..state.max_visible {
                state.select_prev();
            }
            InteractiveResult::Continue
        }
        KeyCode::PageDown => {
            for _ in 0..state.max_visible {
                state.select_next();
            }
            InteractiveResult::Continue
        }

        // Home/End
        KeyCode::Home => {
            state.selected = 0;
            state.scroll_offset = 0;
            InteractiveResult::Continue
        }
        KeyCode::End => {
            if !state.filtered_indices.is_empty() {
                state.selected = state.filtered_indices.len() - 1;
                if state.selected >= state.max_visible {
                    state.scroll_offset = state.selected - state.max_visible + 1;
                }
            }
            InteractiveResult::Continue
        }

        // Tab navigation (Left/Right)
        KeyCode::Left if !state.tabs.is_empty() => InteractiveResult::SwitchTab { direction: -1 },
        KeyCode::Right if !state.tabs.is_empty() => InteractiveResult::SwitchTab { direction: 1 },

        // Selection
        KeyCode::Enter => {
            if let Some(item) = state.selected_item() {
                if item.disabled {
                    InteractiveResult::Continue
                } else if state.multi_select && !state.checked.is_empty() {
                    // Return all checked items
                    let item_ids: Vec<String> =
                        state.checked_items().iter().map(|i| i.id.clone()).collect();
                    InteractiveResult::Selected {
                        action: state.action.clone(),
                        item_id: item_ids.first().cloned().unwrap_or_default(),
                        item_ids,
                    }
                } else {
                    InteractiveResult::Selected {
                        action: state.action.clone(),
                        item_id: item.id.clone(),
                        item_ids: vec![item.id.clone()],
                    }
                }
            } else {
                InteractiveResult::Continue
            }
        }

        // Toggle (multi-select)
        KeyCode::Char(' ') if state.multi_select => {
            state.toggle_check();
            state.select_next(); // Move to next after toggle
            InteractiveResult::Continue
        }

        // Cancel (Esc, Ctrl+C, Ctrl+Q)
        KeyCode::Esc => InteractiveResult::Cancelled,
        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
            InteractiveResult::Cancelled
        }
        KeyCode::Char('q') if key.modifiers == KeyModifiers::CONTROL => {
            InteractiveResult::Cancelled
        }

        // Search input (when searchable)
        KeyCode::Char(c) if state.searchable && key.modifiers.is_empty() => {
            // Check for shortcuts first
            if !state.search_query.is_empty() || !is_shortcut(state, c) {
                state.push_search_char(c);
            } else if let Some(result) = try_shortcut(state, c) {
                return result;
            }
            InteractiveResult::Continue
        }

        // Shortcuts (when not searching)
        KeyCode::Char(c) if !state.searchable && key.modifiers.is_empty() => {
            if let Some(result) = try_shortcut(state, c) {
                return result;
            }
            InteractiveResult::Continue
        }

        // Backspace (search)
        KeyCode::Backspace if state.searchable && !state.search_query.is_empty() => {
            state.pop_search_char();
            InteractiveResult::Continue
        }

        // Clear search
        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL && state.searchable => {
            state.update_search("");
            InteractiveResult::Continue
        }

        _ => InteractiveResult::Continue,
    }
}

/// Handle key events when inline form is active.
fn handle_form_key(state: &mut InteractiveState, key: KeyEvent) -> InteractiveResult {
    match key.code {
        // Submit form with Enter
        KeyCode::Enter => {
            if let Some(ref form) = state.inline_form
                && form.is_valid()
            {
                let action_id = form.action_id.clone();
                let values: HashMap<String, String> = form
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.value.clone()))
                    .collect();
                state.close_form();
                return InteractiveResult::FormSubmitted { action_id, values };
            }
            InteractiveResult::Continue
        }

        // Cancel form with Esc
        KeyCode::Esc => {
            state.close_form();
            InteractiveResult::Continue
        }

        // Navigate fields with Tab
        KeyCode::Tab => {
            if let Some(ref mut form) = state.inline_form {
                form.focus_next();
            }
            InteractiveResult::Continue
        }

        // Navigate fields with Shift+Tab
        KeyCode::BackTab => {
            if let Some(ref mut form) = state.inline_form {
                form.focus_prev();
            }
            InteractiveResult::Continue
        }

        // Type characters into focused field
        KeyCode::Char(c) => {
            if let Some(ref mut form) = state.inline_form
                && let Some(ref mut field) = form.focused_mut()
            {
                field.value.push(c);
            }
            InteractiveResult::Continue
        }

        // Backspace
        KeyCode::Backspace => {
            if let Some(ref mut form) = state.inline_form
                && let Some(ref mut field) = form.focused_mut()
            {
                field.value.pop();
            }
            InteractiveResult::Continue
        }

        _ => InteractiveResult::Continue,
    }
}

/// Check if a character is a shortcut for any item.
fn is_shortcut(state: &InteractiveState, c: char) -> bool {
    state.items.iter().any(|item| item.shortcut == Some(c))
}

/// Try to select an item by its shortcut.
fn try_shortcut(state: &mut InteractiveState, c: char) -> Option<InteractiveResult> {
    // Special handling for Resume Picker: 'f' = Fork current selection
    if c == 'f'
        && matches!(state.action, InteractiveAction::ResumeSession)
        && let Some(item) = state.selected_item()
        && !item.disabled
        && !item.id.starts_with("__")
    {
        return Some(InteractiveResult::Selected {
            action: InteractiveAction::ForkSession,
            item_id: item.id.clone(),
            item_ids: vec![item.id.clone()],
        });
    }

    for (idx, item) in state.items.iter().enumerate() {
        if item.shortcut == Some(c) && !item.disabled {
            // Find the filtered index
            if let Some(filtered_idx) = state.filtered_indices.iter().position(|&i| i == idx) {
                state.selected = filtered_idx;
                return Some(InteractiveResult::Selected {
                    action: state.action.clone(),
                    item_id: item.id.clone(),
                    item_ids: vec![item.id.clone()],
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::state::{InteractiveAction, InteractiveItem};

    fn create_test_state() -> InteractiveState {
        let items = vec![
            InteractiveItem::new("1", "Apple").with_shortcut('a'),
            InteractiveItem::new("2", "Banana").with_shortcut('b'),
            InteractiveItem::new("3", "Cherry").with_shortcut('c'),
        ];
        InteractiveState::new("Test", items, InteractiveAction::Custom("test".into()))
    }

    #[test]
    fn test_navigation() {
        let mut state = create_test_state();

        assert_eq!(state.selected, 0);

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        handle_interactive_key(&mut state, key);
        assert_eq!(state.selected, 1);

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        handle_interactive_key(&mut state, key);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_selection() {
        let mut state = create_test_state();
        state.selected = 1;

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = handle_interactive_key(&mut state, key);

        match result {
            InteractiveResult::Selected { item_id, .. } => {
                assert_eq!(item_id, "2");
            }
            _ => panic!("Expected Selected result"),
        }
    }

    #[test]
    fn test_shortcut() {
        let mut state = create_test_state();

        let key = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE);
        let result = handle_interactive_key(&mut state, key);

        match result {
            InteractiveResult::Selected { item_id, .. } => {
                assert_eq!(item_id, "2");
            }
            _ => panic!("Expected Selected result"),
        }
    }

    #[test]
    fn test_cancel() {
        let mut state = create_test_state();

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = handle_interactive_key(&mut state, key);

        assert!(matches!(result, InteractiveResult::Cancelled));
    }
}
