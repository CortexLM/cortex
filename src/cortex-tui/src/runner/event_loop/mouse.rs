//! Mouse handling: click, drag, scroll events.

use anyhow::Result;
use ratatui::layout::Rect;

use crate::app::FocusTarget;
use crate::input::{ClickZoneId, MouseAction, MouseButton};
use crate::runner::terminal::CortexTerminal;
use crate::session::CortexSession;
use crate::views::{QuestionClickZones, QuestionHit};

use super::core::EventLoop;

impl EventLoop {
    /// Handles a mouse action.
    pub(super) async fn handle_mouse_action(
        &mut self,
        action: MouseAction,
        terminal: &mut CortexTerminal,
    ) -> Result<()> {
        match action {
            MouseAction::Click { x, y, button } => {
                // Handle interactive mode clicks
                if self.app_state.is_interactive_mode()
                    && let Some((action, item_id, item_ids)) = self.handle_interactive_click(x, y)
                {
                    let keep_open = self
                        .handle_interactive_selection(action, item_id, item_ids)
                        .await;
                    if !keep_open {
                        self.app_state.exit_interactive_mode();
                    }
                    self.render(terminal)?;
                    return Ok(());
                }

                // Handle question view clicks
                if self.app_state.view == crate::app::AppView::Questions
                    && self.handle_question_click(x, y)
                {
                    self.render(terminal)?;
                    return Ok(());
                }

                // Find clicked zone
                if let Some(zone_id) = self.click_zones.find(x, y) {
                    let should_copy = self.handle_click(zone_id, button)?;
                    if should_copy {
                        self.copy_selection_to_clipboard(terminal)?;
                        self.app_state.text_selection.clear();
                    }
                }

                self.render(terminal)?;
            }

            MouseAction::DoubleClick { x, y } => {
                if let Some(zone_id) = self.click_zones.find(x, y) {
                    self.handle_double_click(zone_id)?;
                }
                self.render(terminal)?;
            }

            MouseAction::TripleClick { x, y } => {
                if let Some(zone_id) = self.click_zones.find(x, y) {
                    self.handle_triple_click(zone_id)?;
                }
                self.render(terminal)?;
            }

            MouseAction::Drag {
                start,
                current,
                button: _,
            } => {
                // Start text selection from start position if not already started
                let (sx, sy) = start;
                let (cx, cy) = current;
                if !self.app_state.text_selection.is_selecting() {
                    self.app_state.text_selection.start_selection(sx, sy);
                }
                // Update text selection end position to current position
                self.app_state.text_selection.update_selection(cx, cy);
                self.render(terminal)?;
            }

            MouseAction::Release { x, y, button: _ } => {
                // End text selection on release
                self.app_state.text_selection.update_selection(x, y);
                self.app_state.text_selection.finish_selection();
                self.render(terminal)?;
            }

            MouseAction::Scroll { x, y, delta } => {
                if let Some(zone_id) = self.click_zones.find(x, y) {
                    self.handle_scroll(zone_id, delta);
                }
                self.render(terminal)?;
            }

            MouseAction::Move { x, y } => {
                // Handle hover effects for interactive mode
                if self.app_state.is_interactive_mode()
                    && let Some(state) = self.app_state.get_interactive_state_mut()
                {
                    if let Some(idx) = state.hit_test(x, y) {
                        if state.hovered != Some(idx) {
                            state.hovered = Some(idx);
                        }
                    } else if state.hovered.is_some() {
                        state.hovered = None;
                    }
                }

                // Handle question view hover
                if self.app_state.view == crate::app::AppView::Questions {
                    self.handle_question_hover(x, y);
                }

                self.render(terminal)?;
            }
        }

        Ok(())
    }

    /// Handle question view hover
    fn handle_question_hover(&mut self, x: u16, y: u16) {
        let (width, height) = self.app_state.terminal_size;
        let area = Rect::new(0, 0, width, height);

        let Some(q_state) = self.app_state.get_question_state() else {
            return;
        };

        let zones = QuestionClickZones::calculate(q_state, area);
        let hit = zones.hit_test(x, y);

        match hit {
            QuestionHit::Tab(idx) => {
                self.app_state.set_question_hovered_tab(Some(idx));
                self.app_state.set_question_hovered_option(None);
            }
            QuestionHit::Option(idx) => {
                self.app_state.set_question_hovered_option(Some(idx));
                self.app_state.set_question_hovered_tab(None);
            }
            _ => {
                self.app_state.set_question_hovered_option(None);
                self.app_state.set_question_hovered_tab(None);
            }
        }
    }

    /// Handles mouse click for question prompts
    fn handle_question_click(&mut self, x: u16, y: u16) -> bool {
        let (width, height) = self.app_state.terminal_size;
        let area = Rect::new(0, 0, width, height);

        let Some(q_state) = self.app_state.get_question_state() else {
            return false;
        };

        let zones = QuestionClickZones::calculate(q_state, area);
        let hit = zones.hit_test(x, y);

        match hit {
            QuestionHit::Tab(idx) => {
                if let Some(q_state) = self.app_state.get_question_state_mut() {
                    q_state.select_tab(idx);
                }
                true
            }
            QuestionHit::Option(idx) => {
                if let Some(q_state) = self.app_state.get_question_state_mut() {
                    q_state.move_to(idx);
                    q_state.toggle_current();
                }
                true
            }
            QuestionHit::Confirm => true,
            QuestionHit::None => false,
        }
    }

    /// Handles mouse click for interactive mode
    fn handle_interactive_click(
        &mut self,
        x: u16,
        y: u16,
    ) -> Option<(crate::interactive::InteractiveAction, String, Vec<String>)> {
        let state = self.app_state.get_interactive_state_mut()?;

        // Check if click is on a tab first
        if let Some(tab_idx) = state.hit_test_tab(x, y) {
            if tab_idx != state.active_tab {
                return Some((
                    crate::interactive::InteractiveAction::Custom(format!(
                        "switch_tab:{}",
                        tab_idx
                    )),
                    String::new(),
                    Vec::new(),
                ));
            }
            return None;
        }

        // Hit test using stored click zones
        let idx = state.hit_test(x, y)?;

        // Move selection to clicked item
        state.selected = idx;
        state.hovered = Some(idx);

        // Get the selected item to check if it's selectable
        let _item = match state.selected_item() {
            Some(item) if !item.disabled && !item.is_separator => item,
            _ => return None,
        };

        let is_multi = state.multi_select;

        // Handle multi-select differently
        if is_multi {
            state.toggle_check();
            return None;
        }

        // Simulate Enter key press
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let enter_key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = crate::interactive::handle_interactive_key(state, enter_key);

        match result {
            crate::interactive::InteractiveResult::Selected {
                action,
                item_id,
                item_ids,
            } => Some((action, item_id, item_ids)),
            _ => None,
        }
    }

    /// Handles a single click on a click zone.
    fn handle_click(&mut self, zone_id: ClickZoneId, button: MouseButton) -> Result<bool> {
        match (zone_id, button) {
            (ClickZoneId::SessionItem(idx), MouseButton::Left) => {
                self.load_session_at_index(idx)?;
            }

            (ClickZoneId::NewSessionButton, MouseButton::Left) => {
                self.app_state.new_session();
            }

            (ClickZoneId::InputField, MouseButton::Left) => {
                // Already focused on input, nothing to do
            }

            (ClickZoneId::ChatArea | ClickZoneId::MessageItem(_), MouseButton::Left) => {
                // Chat area is read-only, keep focus on input
            }

            (ClickZoneId::Sidebar, MouseButton::Left) => {
                // Sidebar not used in minimalist mode
            }

            (ClickZoneId::ModelSelector, MouseButton::Left) => {
                self.open_models_modal();
                tracing::debug!("Model selector clicked - opening modal");
            }

            (ClickZoneId::ApproveButton, MouseButton::Left) => {
                if self.app_state.pending_approval.is_some() {
                    self.app_state.approve();
                }
            }

            (ClickZoneId::RejectButton, MouseButton::Left) => {
                if self.app_state.pending_approval.is_some() {
                    self.app_state.reject();
                }
            }

            (zone_id, MouseButton::Right) => {
                if self.app_state.text_selection.has_selection() {
                    return Ok(true);
                } else {
                    tracing::debug!("Right-click on {:?}", zone_id);
                }
            }

            _ => {}
        }

        Ok(false)
    }

    /// Handles a double-click on a click zone.
    fn handle_double_click(&mut self, zone_id: ClickZoneId) -> Result<()> {
        match zone_id {
            ClickZoneId::SessionItem(idx) => {
                self.load_session_at_index(idx)?;
                self.app_state.set_focus(FocusTarget::Input);
            }

            ClickZoneId::MessageItem(_) | ClickZoneId::ChatArea => {
                tracing::debug!("Double-click in chat area - word selection requested");
            }

            ClickZoneId::InputField => {
                self.app_state.set_focus(FocusTarget::Input);
                self.app_state.input.select_word_at_cursor();
            }

            _ => {}
        }

        Ok(())
    }

    /// Handles a triple-click on a click zone (line selection).
    fn handle_triple_click(&mut self, zone_id: ClickZoneId) -> Result<()> {
        match zone_id {
            ClickZoneId::MessageItem(_) | ClickZoneId::ChatArea => {
                tracing::debug!("Triple-click in chat area - line selection requested");
            }

            ClickZoneId::InputField => {
                self.app_state.set_focus(FocusTarget::Input);
                self.app_state.input.select_current_line();
            }

            _ => {}
        }

        Ok(())
    }

    /// Handles scroll events on a click zone.
    pub(super) fn handle_scroll(&mut self, zone_id: ClickZoneId, delta: i16) {
        match zone_id {
            ClickZoneId::ChatArea | ClickZoneId::MessageItem(_) => {
                self.app_state.scroll_chat(-delta as i32);
            }

            ClickZoneId::Sidebar | ClickZoneId::SessionItem(_) => {
                self.app_state.scroll_sidebar(delta as i32);
            }

            ClickZoneId::DiffView => {
                self.app_state.scroll_diff(delta as i32);
                tracing::debug!("Scroll diff view by {}", delta);
            }

            _ => {}
        }
    }

    /// Loads a session by its index in the session history.
    fn load_session_at_index(&mut self, idx: usize) -> Result<()> {
        if let Some(session) = self.app_state.session_history.get(idx) {
            let session_id = session.id;
            self.app_state.load_session(session_id);

            let session_id_str = session_id.to_string();
            if let Ok(loaded_session) = CortexSession::load(&session_id_str) {
                for msg in loaded_session.messages() {
                    let message = if msg.role == "user" {
                        cortex_core::widgets::Message::user(&msg.content)
                    } else {
                        cortex_core::widgets::Message::assistant(&msg.content)
                    };
                    self.app_state.add_message(message);
                }
                self.cortex_session = Some(loaded_session);
                tracing::info!(
                    "Loaded session with {} messages",
                    self.app_state.messages.len()
                );
            }
        }
        Ok(())
    }

    /// Handles keyboard input for question prompts.
    pub(super) async fn handle_question_key(
        &mut self,
        key_event: crossterm::event::KeyEvent,
    ) -> Result<bool> {
        use crossterm::event::KeyCode;

        let Some(q_state) = self.app_state.get_question_state_mut() else {
            return Ok(false);
        };

        // If editing custom input, handle text input
        if q_state.editing_custom {
            match key_event.code {
                KeyCode::Esc => {
                    q_state.cancel_custom_input();
                    return Ok(true);
                }
                KeyCode::Enter => {
                    q_state.confirm_custom_input();
                    if q_state.is_single_question() && !q_state.answers[0].is_empty() {
                        self.submit_question_answers().await?;
                    }
                    return Ok(true);
                }
                KeyCode::Backspace => {
                    q_state.current_custom_text.pop();
                    return Ok(true);
                }
                KeyCode::Char(c) => {
                    q_state.current_custom_text.push(c);
                    return Ok(true);
                }
                _ => return Ok(true),
            }
        }

        // On confirm tab
        if q_state.on_confirm_tab {
            match key_event.code {
                KeyCode::Esc => {
                    self.cancel_question_prompt().await?;
                    return Ok(true);
                }
                KeyCode::Enter => {
                    self.submit_question_answers().await?;
                    return Ok(true);
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    q_state.prev_tab();
                    return Ok(true);
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    q_state.next_tab();
                    return Ok(true);
                }
                _ => return Ok(true),
            }
        }

        // Normal question navigation
        match key_event.code {
            KeyCode::Esc => {
                self.cancel_question_prompt().await?;
                Ok(true)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                q_state.move_up();
                Ok(true)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                q_state.move_down();
                Ok(true)
            }
            KeyCode::Left | KeyCode::Char('h') => {
                q_state.prev_tab();
                Ok(true)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                q_state.next_tab();
                Ok(true)
            }
            KeyCode::Enter => {
                q_state.toggle_current();
                if q_state.is_single_question()
                    && !q_state.answers[0].is_empty()
                    && !q_state.editing_custom
                {
                    self.submit_question_answers().await?;
                }
                Ok(true)
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let digit = c.to_digit(10).unwrap_or(0) as usize;
                if digit >= 1 && digit <= q_state.option_count() {
                    q_state.move_to(digit - 1);
                    q_state.toggle_current();
                    if q_state.is_single_question()
                        && !q_state.answers[0].is_empty()
                        && !q_state.editing_custom
                    {
                        self.submit_question_answers().await?;
                    }
                }
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    /// Submit question answers and complete the tool call
    async fn submit_question_answers(&mut self) -> Result<()> {
        let Some(q_state) = self.app_state.get_question_state() else {
            return Ok(());
        };

        let tool_call_id = q_state.request.id.clone();
        let answers = q_state.get_formatted_answers();

        let formatted_answers = serde_json::to_string_pretty(&answers).unwrap_or_default();
        let output = format!("User answered the questions:\n{}", formatted_answers);

        self.app_state.update_tool_result(
            &tool_call_id,
            output.clone(),
            true,
            "Questions answered".to_string(),
        );

        self.app_state.complete_question_prompt();

        self.app_state
            .add_pending_tool_result(tool_call_id, "Questions".to_string(), output, true);

        if !self.app_state.streaming.is_streaming && self.app_state.has_pending_tool_results() {
            let _ = self.continue_with_tool_results().await;
        }

        Ok(())
    }

    /// Cancel question prompt and return an error result
    async fn cancel_question_prompt(&mut self) -> Result<()> {
        let Some(tool_call_id) = self.app_state.cancel_question_prompt() else {
            return Ok(());
        };

        let output = "User dismissed the questions without answering.".to_string();

        self.app_state.update_tool_result(
            &tool_call_id,
            output.clone(),
            false,
            "Questions dismissed".to_string(),
        );

        self.app_state.add_pending_tool_result(
            tool_call_id,
            "Questions".to_string(),
            output,
            false,
        );

        if !self.app_state.streaming.is_streaming && self.app_state.has_pending_tool_results() {
            let _ = self.continue_with_tool_results().await;
        }

        Ok(())
    }
}
