//! Rendering logic: drawing the UI to the terminal.

use anyhow::Result;
use ratatui::prelude::*;
use ratatui::widgets::Clear;

use crate::app::AppView;
use crate::input::ClickZoneId;
use crate::runner::terminal::CortexTerminal;
use crate::views::{ApprovalView, QuestionPromptView};

use super::core::EventLoop;

impl EventLoop {
    /// Renders the current view to the terminal.
    pub(super) fn render(&mut self, terminal: &mut CortexTerminal) -> Result<()> {
        // Check if scrollback clear was requested
        if self.app_state.take_pending_scrollback_clear()
            && let Err(e) = terminal.clear_scrollback()
        {
            tracing::warn!("Failed to clear terminal scrollback: {}", e);
        }

        // Sync streaming content from stream controller to app state
        if self.app_state.streaming.is_streaming {
            let display_text = self.stream_controller.display_text();
            if let Some(ref mut typewriter) = self.app_state.typewriter
                && typewriter.visible_text() != display_text
            {
                typewriter.set_text(display_text.to_string());
                typewriter.skip_to_end();
            }
        }

        terminal.draw(|frame| {
            let area = frame.area();

            match &self.app_state.view {
                AppView::Session => {
                    let view = crate::views::MinimalSessionView::new(&self.app_state);
                    frame.render_widget(view, area);
                }

                AppView::Approval => {
                    let session_view = crate::views::MinimalSessionView::new(&self.app_state);
                    frame.render_widget(session_view, area);
                    let approval_view = ApprovalView::new(&self.app_state);
                    frame.render_widget(approval_view, area);
                }

                AppView::Questions => {
                    let session_view = crate::views::MinimalSessionView::new(&self.app_state);
                    frame.render_widget(session_view, area);
                    if let Some(q_state) = self.app_state.get_question_state() {
                        let question_view = QuestionPromptView::new(q_state)
                            .with_hovered_option(self.app_state.question_hovered_option)
                            .with_hovered_tab(self.app_state.question_hovered_tab)
                            .with_colors(self.app_state.adaptive_colors());
                        frame.render_widget(question_view, area);
                    }
                }

                AppView::Settings | AppView::Help => {
                    let view = crate::views::MinimalSessionView::new(&self.app_state);
                    frame.render_widget(view, area);
                }

                AppView::SubagentConversation(_session_id) => {
                    // Render the subagent conversation view (same as session for now)
                    let view = crate::views::MinimalSessionView::new(&self.app_state);
                    frame.render_widget(view, area);
                }
            }

            // Render modal overlays (legacy)
            if let Some(modal) = self.app_state.active_modal.as_ref() {
                use crate::app::ActiveModal;
                use crate::widgets::ModelPicker;

                match modal {
                    ActiveModal::ModelPicker => {
                        let picker = ModelPicker::new(&self.app_state.model_picker);
                        frame.render_widget(picker, area);
                    }
                    ActiveModal::Form(form_state) => {
                        let widget = crate::widgets::FormModal::new(form_state);
                        frame.render_widget(widget, area);
                    }
                    _ => {}
                }
            }

            // Render modal stack (new unified modal system)
            if self.modal_stack.is_active() {
                let modal_width = (area.width as f32 * 0.6).max(40.0).min(area.width as f32) as u16;
                if let Some(modal) = self.modal_stack.current() {
                    let modal_height = modal.desired_height(area.height - 4, modal_width);
                    let modal_x = (area.width.saturating_sub(modal_width)) / 2;
                    let modal_y = (area.height.saturating_sub(modal_height)) / 2;
                    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

                    Clear.render(modal_area, frame.buffer_mut());
                    modal.render(modal_area, frame.buffer_mut());
                }
            }

            // Render new card system overlays
            if self.card_handler.is_active() {
                let modal_width = (area.width as f32 * 0.6).max(40.0).min(area.width as f32) as u16;
                let modal_height =
                    (area.height as f32 * 0.8).max(10.0).min(area.height as f32) as u16;
                let modal_x = (area.width.saturating_sub(modal_width)) / 2;
                let modal_y = (area.height.saturating_sub(modal_height)) / 2;
                let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

                self.card_handler.render(modal_area, frame.buffer_mut());
            }

            // Apply selection highlight
            self.apply_selection_highlight(frame.buffer_mut());

            // Render toast notifications
            if !self.app_state.toasts.is_empty() {
                let toast_widget = crate::widgets::ToastWidget::new(&self.app_state.toasts)
                    .terminal_size(area.width, area.height);
                toast_widget.render(area, frame.buffer_mut());
            }
        })?;

        // Capture frame for TUI debugging
        if self.tui_capture.is_enabled() {
            let autocomplete_visible = self.app_state.autocomplete.visible;
            let autocomplete_query = self.app_state.autocomplete.query.clone();
            self.tui_capture
                .record_autocomplete(autocomplete_visible, &autocomplete_query);

            let modal_open = self.app_state.has_modal() || self.modal_stack.is_active();
            let modal_name = if self.modal_stack.is_active() {
                "modal_stack"
            } else if self.app_state.has_modal() {
                "legacy_modal"
            } else {
                ""
            };
            self.tui_capture.record_modal(modal_open, modal_name);

            let current_view = format!("{:?}", self.app_state.view);
            if current_view != self.tui_capture.current_view {
                let old_view =
                    std::mem::replace(&mut self.tui_capture.current_view, current_view.clone());
                self.tui_capture
                    .record_view_change(&old_view, &current_view);
            }

            let buffer = terminal.terminal.current_buffer_mut();
            self.tui_capture.capture_auto(buffer);
        }

        // Register click zones
        self.register_click_zones();

        Ok(())
    }

    /// Registers click zones for mouse interaction.
    pub(super) fn register_click_zones(&mut self) {
        self.click_zones.clear();

        let (width, height) = self.app_state.terminal_size;
        let area = Rect::new(0, 0, width, height);

        match &self.app_state.view {
            AppView::Session
            | AppView::Approval
            | AppView::Questions
            | AppView::Settings
            | AppView::Help
            | AppView::SubagentConversation(_) => {
                let input_height: u16 = 1;
                let hints_height: u16 = 1;
                let status_height: u16 = if self.app_state.streaming.is_streaming {
                    1
                } else {
                    0
                };

                let total_bottom = status_height + input_height + hints_height;
                let chat_height = area.height.saturating_sub(total_bottom);

                let chat_area = Rect::new(area.x, area.y, area.width, chat_height);
                self.click_zones.register(ClickZoneId::ChatArea, chat_area);

                let input_y = area.y + chat_height + status_height;
                let input_area = Rect::new(area.x, input_y, area.width, input_height);
                self.click_zones
                    .register(ClickZoneId::InputField, input_area);

                // Calculate click zones for interactive mode
                if self.app_state.is_interactive_mode()
                    && let Some(state) = self.app_state.get_interactive_state_mut()
                {
                    let items_count = state.filtered_indices.len().min(state.max_visible);
                    let required_height = (items_count as u16) + 4;
                    let max_height = (area.height * 85 / 100).max(12);
                    let widget_height = required_height.min(max_height);
                    let extra_height = widget_height.saturating_sub(input_height);
                    let interactive_area = Rect::new(
                        input_area.x,
                        input_area.y.saturating_sub(extra_height),
                        input_area.width,
                        widget_height,
                    );
                    crate::interactive::InteractiveWidget::calculate_click_zones(
                        state,
                        interactive_area,
                    );
                }
            }
        }
    }

    /// Applies selection highlight to the entire screen buffer.
    pub(super) fn apply_selection_highlight(&self, buf: &mut ratatui::buffer::Buffer) {
        use ratatui::style::Color;

        let Some((start, end)) = self.app_state.text_selection.get_bounds() else {
            return;
        };

        let selection_bg = Color::Rgb(60, 100, 140);
        let buf_area = buf.area;

        let start_row = start.1;
        let end_row = end.1;

        for row in start_row..=end_row {
            if row >= buf_area.height {
                continue;
            }

            let col_start = if row == start_row { start.0 } else { 0 };
            let col_end = if row == end_row {
                end.0
            } else {
                buf_area.width.saturating_sub(1)
            };

            for col in col_start..=col_end {
                if col < buf_area.width
                    && let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(col, row))
                {
                    cell.set_bg(selection_bg);
                }
            }
        }
    }

    /// Returns the chat area rectangle if available.
    pub(super) fn _get_chat_area(&self) -> Option<Rect> {
        self.click_zones.get_zone_rect(ClickZoneId::ChatArea)
    }

    /// Copies the current text selection to the clipboard.
    pub(super) fn copy_selection_to_clipboard(
        &mut self,
        terminal: &mut CortexTerminal,
    ) -> Result<()> {
        let selection_bounds = self.app_state.text_selection.get_bounds();

        if let Some(((start_col, start_row), (end_col, end_row))) = selection_bounds {
            use std::cell::RefCell;
            let selected_lines: RefCell<Vec<String>> = RefCell::new(Vec::new());

            let active_modal = self.app_state.active_modal.clone();
            let card_active = self.card_handler.is_active();

            terminal.terminal.draw(|frame| {
                let area = frame.area();
                let screen_width = area.width;
                let screen_height = area.height;

                // Full render
                match &self.app_state.view {
                    AppView::Session => {
                        let widget = crate::views::MinimalSessionView::new(&self.app_state);
                        frame.render_widget(widget, area);
                    }
                    AppView::Approval => {
                        let session_view = crate::views::MinimalSessionView::new(&self.app_state);
                        frame.render_widget(session_view, area);
                        let approval_view = ApprovalView::new(&self.app_state);
                        frame.render_widget(approval_view, area);
                    }
                    AppView::Questions => {
                        let session_view = crate::views::MinimalSessionView::new(&self.app_state);
                        frame.render_widget(session_view, area);
                        if let Some(q_state) = self.app_state.get_question_state() {
                            let question_view = QuestionPromptView::new(q_state)
                                .with_hovered_option(self.app_state.question_hovered_option)
                                .with_hovered_tab(self.app_state.question_hovered_tab)
                                .with_colors(self.app_state.adaptive_colors());
                            frame.render_widget(question_view, area);
                        }
                    }
                    AppView::Settings | AppView::Help => {
                        let widget = crate::views::MinimalSessionView::new(&self.app_state);
                        frame.render_widget(widget, area);
                    }
                    AppView::SubagentConversation(_session_id) => {
                        let widget = crate::views::MinimalSessionView::new(&self.app_state);
                        frame.render_widget(widget, area);
                    }
                }

                // Render modals
                if let Some(modal) = active_modal {
                    use crate::app::ActiveModal;
                    use crate::widgets::ModelPicker;
                    if let ActiveModal::ModelPicker = modal {
                        let picker = ModelPicker::new(&self.app_state.model_picker);
                        frame.render_widget(picker, area);
                    }
                }

                // Render cards
                if card_active {
                    let modal_width =
                        (area.width as f32 * 0.6).max(40.0).min(area.width as f32) as u16;
                    let modal_height =
                        (area.height as f32 * 0.8).max(10.0).min(area.height as f32) as u16;
                    let modal_x = (area.width.saturating_sub(modal_width)) / 2;
                    let modal_y = (area.height.saturating_sub(modal_height)) / 2;
                    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);
                    self.card_handler.render(modal_area, frame.buffer_mut());
                }

                // Extract text from buffer
                let buf = frame.buffer_mut();
                let mut lines = selected_lines.borrow_mut();

                for row in start_row..=end_row {
                    if row >= screen_height {
                        break;
                    }

                    let col_start = if row == start_row { start_col } else { 0 };
                    let col_end = if row == end_row {
                        end_col
                    } else {
                        screen_width.saturating_sub(1)
                    };

                    let mut line = String::new();
                    for col in col_start..=col_end {
                        if col >= screen_width {
                            break;
                        }

                        if let Some(cell) = buf.cell((col, row)) {
                            line.push_str(cell.symbol());
                        }
                    }

                    lines.push(line.trim_end().to_string());
                }
            })?;

            let selected_text = selected_lines.into_inner().join("\n");

            if !selected_text.is_empty() {
                tracing::debug!("Copied {} chars to clipboard", selected_text.len());
                self.app_state.toasts.success("Copied!");

                #[cfg(target_os = "linux")]
                {
                    std::thread::spawn(move || match arboard::Clipboard::new() {
                        Ok(mut clipboard) => {
                            use arboard::SetExtLinux;
                            if let Err(e) = clipboard.set().wait().text(selected_text) {
                                tracing::warn!("Failed to copy to clipboard: {}", e);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to access clipboard: {}", e);
                        }
                    });
                }

                #[cfg(not(target_os = "linux"))]
                {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Err(e) = clipboard.set_text(&selected_text) {
                            tracing::warn!("Failed to copy to clipboard: {}", e);
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
