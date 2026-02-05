//! Input handling: keyboard, engine events, autocomplete.

use std::time::Duration;

use anyhow::Result;

use crate::actions::{ActionContext, KeyAction};
use crate::app::{AppView, AutocompleteItem, AutocompleteTrigger};
use crate::bridge::adapt_event;
use crate::events::AppEvent;
use crate::modal::ModalResult;
use crate::runner::terminal::CortexTerminal;

use super::core::EventLoop;
use cortex_core::EngineEvent;

impl EventLoop {
    /// Handle events from the frame engine.
    ///
    /// This processes low-level events from the terminal: frame ticks (for
    /// animations), keyboard events (mapped to actions), mouse events,
    /// resize events, and quit signals.
    pub(super) async fn handle_engine_event(
        &mut self,
        event: EngineEvent,
        terminal: &mut CortexTerminal,
    ) -> Result<()> {
        match event {
            EngineEvent::Tick(_frame) => {
                self.handle_tick(terminal).await?;
            }

            EngineEvent::Key(key_event) => {
                self.handle_key_event(key_event, terminal).await?;
            }

            EngineEvent::Mouse(mouse_event) => {
                if let Some(action) = self.mouse_handler.handle(mouse_event) {
                    self.handle_mouse_action(action, terminal).await?;
                }
            }

            EngineEvent::Resize(width, height) => {
                self.handle_resize(width, height, terminal)?;
            }

            EngineEvent::Quit => {
                self.app_state.set_quit();
            }

            EngineEvent::Error(msg) => {
                tracing::error!("Engine error: {}", msg);
            }

            EngineEvent::Paste(text) => {
                self.handle_paste(&text, terminal)?;
            }

            EngineEvent::Suspend => {
                self.handle_suspend();
            }

            EngineEvent::Resume => {
                self.handle_resume(terminal)?;
            }
        }

        Ok(())
    }

    /// Handle tick event for animations and rendering
    async fn handle_tick(&mut self, terminal: &mut CortexTerminal) -> Result<()> {
        // Track if we have visual changes that need rendering
        let mut needs_render = false;

        // Update animations
        self.app_state.tick();
        self.stream_controller.tick();

        // Update tool call spinners (for animated ◐◑◒◓ indicator)
        self.app_state.tick_tool_spinners();

        // Update subagent spinners (for animated ◐◑◒◓ indicator)
        self.app_state.tick_subagent_spinners();

        // Update scrollbar visibility (for auto-hide with fade)
        self.app_state.tick_scrollbar();

        // Update toast notifications (for auto-dismiss with fade)
        self.app_state.toasts.tick();

        // Check for crashed subagent tasks (panics, cancellations)
        // This ensures the main agent always receives a response even if a subagent crashes
        self.check_crashed_tasks().await;

        // Check if streaming content changed (#2792 - optimization for slow connections)
        if self.app_state.streaming.is_streaming {
            // During streaming, we need to render more frequently
            // but we can skip frames when there's no new content
            let display_text = self.stream_controller.display_text();
            if let Some(ref typewriter) = self.app_state.typewriter {
                if typewriter.visible_text() != display_text {
                    needs_render = true;
                }
            } else {
                needs_render = true;
            }

            // Also render if animations are active (spinners, etc.)
            if self.app_state.streaming.thinking {
                needs_render = true;
            }
        } else {
            // Not streaming - check for other animation states
            // Include brain animation on welcome screen (messages empty)
            let brain_animating =
                self.app_state.messages.is_empty() && self.app_state.view == AppView::Session;
            needs_render = self.app_state.toasts.has_visible()
                || self.app_state.has_active_tool_calls()
                || self.app_state.has_active_subagents()
                || brain_animating;
        }

        // Render frame (respecting frame time to avoid over-rendering)
        // During idle states, we can skip renders entirely
        if needs_render && self.last_render.elapsed() >= self.min_frame_time {
            self.render(terminal)?;
            self.last_render = std::time::Instant::now();
        } else if self.last_render.elapsed() >= Duration::from_millis(250) {
            // Still render at least 4 times per second for cursor blink, etc.
            self.render(terminal)?;
            self.last_render = std::time::Instant::now();
        }

        Ok(())
    }

    /// Handle keyboard events
    async fn handle_key_event(
        &mut self,
        key_event: crossterm::event::KeyEvent,
        terminal: &mut CortexTerminal,
    ) -> Result<()> {
        use crossterm::event::KeyCode;

        // Check modal stack first (new unified modal system)
        if self.modal_stack.is_active() {
            let result = self.modal_stack.handle_key(key_event);
            if let ModalResult::Action(action) = result {
                self.process_modal_action(action).await;
            }
            self.render(terminal)?;
            return Ok(());
        }

        // Check if in interactive mode and handle its input first
        if self.app_state.is_interactive_mode() {
            if let Some(state) = self.app_state.get_interactive_state_mut() {
                let result = crate::interactive::handle_interactive_key(state, key_event);
                match result {
                    crate::interactive::InteractiveResult::Selected {
                        action,
                        item_id,
                        item_ids,
                    } => {
                        let keep_open = self
                            .handle_interactive_selection(action, item_id, item_ids)
                            .await;
                        if !keep_open {
                            self.app_state.exit_interactive_mode();
                        }
                    }
                    crate::interactive::InteractiveResult::FormSubmitted { action_id, values } => {
                        // Handle inline form submission
                        // Returns true if we should stay in interactive mode
                        let stay_open = self.handle_inline_form_submission(&action_id, values);
                        if !stay_open {
                            self.app_state.exit_interactive_mode();
                        }
                    }
                    crate::interactive::InteractiveResult::Cancelled => {
                        self.app_state.exit_interactive_mode();
                    }
                    crate::interactive::InteractiveResult::Continue => {
                        // Just re-render
                    }
                    crate::interactive::InteractiveResult::SwitchTab { direction } => {
                        // Rebuild settings with new tab
                        if let Some(state) = self.app_state.get_interactive_state()
                            && !state.tabs.is_empty()
                        {
                            let current_tab = state.active_tab;
                            let num_tabs = state.tabs.len();
                            let new_tab = if direction < 0 {
                                if current_tab == 0 {
                                    num_tabs - 1
                                } else {
                                    current_tab - 1
                                }
                            } else {
                                (current_tab + 1) % num_tabs
                            };
                            // Rebuild settings with new tab using current snapshot
                            let snapshot = crate::interactive::builders::SettingsSnapshot {
                                compact_mode: self.app_state.compact_mode,
                                sandbox_mode: self.app_state.sandbox_mode,
                                streaming_enabled: self.app_state.streaming_enabled,
                                sound: self.app_state.sound_enabled,
                                thinking_enabled: self.app_state.thinking_budget.is_some(),
                                debug_mode: self.app_state.debug_mode,
                                ..Default::default()
                            };
                            let new_state =
                                crate::interactive::builders::build_settings_selector_with_tab(
                                    snapshot, None, new_tab,
                                );
                            self.app_state.enter_interactive_mode(new_state);
                        }
                    }
                }
            }
            self.render(terminal)?;
            return Ok(());
        }

        // Handle inline approval UI when pending approval exists
        if self.app_state.pending_approval.is_some() {
            if self.handle_inline_approval_key(key_event, terminal).await? {
                return Ok(());
            }
        }

        // Check if a card is active and handle its input first
        if self.card_handler.is_active() && self.card_handler.handle_key(key_event) {
            // Process any pending card actions
            self.process_card_actions();
            self.render(terminal)?;
            return Ok(());
        }

        // Check if a modal is open and handle its input first
        if self.app_state.has_modal() && self.handle_modal_key(key_event).await? {
            self.render(terminal)?;
            return Ok(());
        }

        // Handle Questions view input
        if self.app_state.view == AppView::Questions && self.handle_question_key(key_event).await? {
            self.render(terminal)?;
            return Ok(());
        }

        // Handle Ctrl+C with contextual behavior
        if key_event.code == KeyCode::Char('c')
            && key_event
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            return self.handle_ctrl_c(terminal);
        }

        // Handle ESC with double-tap to quit when idle
        if key_event.code == KeyCode::Esc {
            return self.handle_esc(terminal);
        }

        // Reset Ctrl+C and ESC timers on other key presses
        if key_event.code != KeyCode::Esc {
            self.app_state.reset_esc();
        }
        self.app_state.reset_ctrl_c();

        let context = self.get_action_context();
        let action = self.action_mapper.get_action(key_event, context);

        // Check if autocomplete is visible and handle its navigation
        if self.app_state.autocomplete.visible
            && self.handle_autocomplete_key(key_event, terminal).await?
        {
            return Ok(());
        }

        // Handle Copy action specially since it needs terminal access
        // This handles both Ctrl+C (when text selected) and Ctrl+Shift+C
        if action == KeyAction::Copy {
            if self.app_state.text_selection.has_selection() {
                self.copy_selection_to_clipboard(terminal)?;
                self.app_state.text_selection.clear();
            }
            self.render(terminal)?;
            return Ok(());
        }

        // Handle '?' key specially: only show help when input is empty
        // Otherwise, type '?' into the input
        if action == KeyAction::Help
            && key_event.code == KeyCode::Char('?')
            && context == ActionContext::Input
            && !self.app_state.input.is_empty()
        {
            // Type '?' into the input instead of opening help
            self.app_state.text_selection.clear();
            self.app_state.input.handle_key(key_event);
            self.update_autocomplete();
            self.render(terminal)?;
            return Ok(());
        }

        // Forward key events to input widget when focused and no action mapped
        // This allows character input, backspace, delete, etc. to work
        if context == ActionContext::Input && action == KeyAction::None {
            // Clear selection when typing
            self.app_state.text_selection.clear();

            self.app_state.input.handle_key(key_event);
            // Update autocomplete based on new input
            self.update_autocomplete();
        }

        self.handle_action(action).await?;

        // Always render after key input for responsiveness
        self.render(terminal)?;

        Ok(())
    }

    /// Handle Ctrl+C with contextual behavior
    fn handle_ctrl_c(&mut self, terminal: &mut CortexTerminal) -> Result<()> {
        // Priority 1: If in interactive mode, exit it
        if self.app_state.is_interactive_mode() {
            self.app_state.exit_interactive_mode();
            self.app_state.reset_ctrl_c();
            self.app_state.reset_esc();
            self.render(terminal)?;
            return Ok(());
        }

        // Priority 2: If text is selected, copy it
        if self.app_state.text_selection.has_selection() {
            self.copy_selection_to_clipboard(terminal)?;
            self.app_state.text_selection.clear();
            self.app_state.reset_ctrl_c();
            self.app_state.reset_esc();
            self.render(terminal)?;
            return Ok(());
        }

        // Priority 3: If input has text, clear it
        if !self.app_state.input.is_empty() {
            self.app_state.input.clear();
            self.app_state.autocomplete.hide();
            self.app_state.reset_ctrl_c();
            self.app_state.reset_esc();
            self.render(terminal)?;
            return Ok(());
        }

        // Priority 4: Double Ctrl+C to quit
        if self.app_state.handle_ctrl_c() {
            self.app_state.set_quit();
        } else {
            self.app_state.toasts.info("Press Ctrl+C again to quit");
            self.render(terminal)?;
        }
        Ok(())
    }

    /// Handle ESC key with double-tap to quit
    fn handle_esc(&mut self, terminal: &mut CortexTerminal) -> Result<()> {
        // Priority 1: If viewing a subagent conversation, return to main conversation
        if self.app_state.is_viewing_subagent() {
            self.app_state.return_to_main_conversation();
            self.render(terminal)?;
            return Ok(());
        }

        // Priority 2: If streaming is active, cancel it
        if self.app_state.streaming.is_streaming {
            self.cancel_streaming();
            self.render(terminal)?;
            return Ok(());
        }

        // Priority 3: If there are queued messages, cancel them
        if self.app_state.has_queued_messages() {
            let count = self.app_state.queued_count();
            self.app_state.clear_message_queue();
            self.add_system_message(&format!("Cancelled {} queued message(s)", count));
            self.render(terminal)?;
            return Ok(());
        }

        // Priority 4: If there's pending approval, reject it
        if self.app_state.pending_approval.is_some() {
            self.app_state.reject();
            self.render(terminal)?;
            return Ok(());
        }

        // Priority 5: If autocomplete is visible, hide it (already handled in handle_autocomplete_key)
        if self.app_state.autocomplete.visible {
            self.app_state.autocomplete.hide();
            self.render(terminal)?;
            return Ok(());
        }

        // Priority 6: Double-tap ESC to quit when idle
        if self.app_state.handle_esc() {
            self.app_state.set_quit();
            Ok(())
        } else {
            self.app_state.toasts.info("Press ESC again to quit");
            self.render(terminal)?;
            Ok(())
        }
    }

    /// Handle autocomplete navigation keys
    async fn handle_autocomplete_key(
        &mut self,
        key_event: crossterm::event::KeyEvent,
        terminal: &mut CortexTerminal,
    ) -> Result<bool> {
        use crossterm::event::KeyCode;

        match key_event.code {
            KeyCode::Up => {
                self.app_state.autocomplete.select_prev();
                self.render(terminal)?;
                Ok(true)
            }
            KeyCode::Down => {
                self.app_state.autocomplete.select_next();
                self.render(terminal)?;
                Ok(true)
            }
            KeyCode::Tab => {
                // Accept completion with Tab only
                if let Some(completion_text) = self.app_state.autocomplete.completion_text() {
                    let pos = self.app_state.autocomplete.trigger_position;
                    let current = self.app_state.input.text();

                    // Find the end of the current query (next whitespace or end of string)
                    let query_end = current[pos..]
                        .find(char::is_whitespace)
                        .map(|i| pos + i)
                        .unwrap_or(current.len());

                    // Get trigger char (/ or @)
                    let trigger = if pos < current.len() {
                        &current[pos..pos + 1]
                    } else {
                        ""
                    };

                    // Build new text: before trigger + trigger + completion + rest of input (or space)
                    let rest = if query_end < current.len() {
                        &current[query_end..]
                    } else {
                        " "
                    };
                    let new_text =
                        format!("{}{}{}{}", &current[..pos], trigger, completion_text, rest);
                    self.app_state.input.set_text(&new_text);
                }
                self.app_state.autocomplete.hide();
                self.render(terminal)?;
                Ok(true)
            }
            KeyCode::Enter => {
                // Enter completes and immediately executes the command
                if let Some(completion_text) = self.app_state.autocomplete.completion_text() {
                    let current = self.app_state.input.text().to_string();
                    let pos = self.app_state.autocomplete.trigger_position;
                    if pos < current.len() {
                        let query_end = current[pos..]
                            .find(char::is_whitespace)
                            .map(|i| pos + i)
                            .unwrap_or(current.len());
                        let trigger = if pos < current.len() {
                            &current[pos..pos + 1]
                        } else {
                            ""
                        };
                        let rest = if query_end < current.len() {
                            &current[query_end..]
                        } else {
                            ""
                        };
                        let new_text =
                            format!("{}{}{}{}", &current[..pos], trigger, completion_text, rest);
                        self.app_state.input.set_text(&new_text);
                    }
                }
                self.app_state.autocomplete.hide();
                // Don't return - fall through to execute the command
                Ok(false)
            }
            KeyCode::Esc => {
                self.app_state.autocomplete.hide();
                self.render(terminal)?;
                Ok(true)
            }
            _ => {
                // Continue to normal handling, which will update autocomplete
                Ok(false)
            }
        }
    }

    /// Handle inline approval UI key events.
    /// Returns true if the key was handled (approval action taken), false otherwise.
    async fn handle_inline_approval_key(
        &mut self,
        key_event: crossterm::event::KeyEvent,
        terminal: &mut CortexTerminal,
    ) -> Result<bool> {
        use crate::app::{InlineApprovalSelection, RiskLevelSelection};
        use crossterm::event::KeyCode;

        // Check if risk level submenu is visible
        let show_submenu = self
            .app_state
            .pending_approval
            .as_ref()
            .map(|a| a.show_risk_submenu)
            .unwrap_or(false);

        if show_submenu {
            // Handle risk level submenu keys
            match key_event.code {
                KeyCode::Char('1') => {
                    // Select Low risk level and approve
                    self.handle_approve_with_risk_level(RiskLevelSelection::Low)
                        .await?;
                    self.render(terminal)?;
                    return Ok(true);
                }
                KeyCode::Char('2') => {
                    // Select Medium risk level and approve
                    self.handle_approve_with_risk_level(RiskLevelSelection::Medium)
                        .await?;
                    self.render(terminal)?;
                    return Ok(true);
                }
                KeyCode::Char('3') => {
                    // Select High risk level and approve
                    self.handle_approve_with_risk_level(RiskLevelSelection::High)
                        .await?;
                    self.render(terminal)?;
                    return Ok(true);
                }
                KeyCode::Esc => {
                    // Close submenu, back to main approval UI
                    if let Some(ref mut approval) = self.app_state.pending_approval {
                        approval.show_risk_submenu = false;
                    }
                    self.render(terminal)?;
                    return Ok(true);
                }
                KeyCode::Left => {
                    // Navigate risk level selection left
                    if let Some(ref mut approval) = self.app_state.pending_approval {
                        approval.selected_risk_level = approval.selected_risk_level.prev();
                    }
                    self.render(terminal)?;
                    return Ok(true);
                }
                KeyCode::Right => {
                    // Navigate risk level selection right
                    if let Some(ref mut approval) = self.app_state.pending_approval {
                        approval.selected_risk_level = approval.selected_risk_level.next();
                    }
                    self.render(terminal)?;
                    return Ok(true);
                }
                KeyCode::Enter => {
                    // Confirm selected risk level
                    let risk_level = self
                        .app_state
                        .pending_approval
                        .as_ref()
                        .map(|a| a.selected_risk_level)
                        .unwrap_or_default();
                    self.handle_approve_with_risk_level(risk_level).await?;
                    self.render(terminal)?;
                    return Ok(true);
                }
                _ => {
                    // Consume other keys when submenu is visible
                    return Ok(true);
                }
            }
        }

        // Handle main approval UI keys
        match key_event.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // Accept once
                self.handle_approve().await?;
                self.render(terminal)?;
                Ok(true)
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                // Reject
                self.handle_reject().await?;
                self.render(terminal)?;
                Ok(true)
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                // Show risk level submenu
                if let Some(ref mut approval) = self.app_state.pending_approval {
                    approval.show_risk_submenu = true;
                    approval.selected_risk_level = RiskLevelSelection::default();
                }
                self.render(terminal)?;
                Ok(true)
            }
            KeyCode::Left => {
                // Navigate selection left
                if let Some(ref mut approval) = self.app_state.pending_approval {
                    approval.selected_action = approval.selected_action.prev();
                }
                self.render(terminal)?;
                Ok(true)
            }
            KeyCode::Right => {
                // Navigate selection right
                if let Some(ref mut approval) = self.app_state.pending_approval {
                    approval.selected_action = approval.selected_action.next();
                }
                self.render(terminal)?;
                Ok(true)
            }
            KeyCode::Enter => {
                // Confirm selected action
                let action = self
                    .app_state
                    .pending_approval
                    .as_ref()
                    .map(|a| a.selected_action)
                    .unwrap_or_default();
                match action {
                    InlineApprovalSelection::AcceptOnce => {
                        self.handle_approve().await?;
                    }
                    InlineApprovalSelection::Reject => {
                        self.handle_reject().await?;
                    }
                    InlineApprovalSelection::AcceptAndSet => {
                        // Show risk level submenu
                        if let Some(ref mut approval) = self.app_state.pending_approval {
                            approval.show_risk_submenu = true;
                            approval.selected_risk_level = RiskLevelSelection::default();
                        }
                    }
                }
                self.render(terminal)?;
                Ok(true)
            }
            _ => {
                // Don't consume other keys - allow them to pass through
                // This allows things like Ctrl+C to work
                Ok(false)
            }
        }
    }

    /// Handle approval with risk level - approves the tool and updates permission mode
    async fn handle_approve_with_risk_level(
        &mut self,
        risk_level: crate::app::RiskLevelSelection,
    ) -> Result<()> {
        use crate::app::RiskLevelSelection;
        use crate::permissions::PermissionMode;

        // Update permission mode based on selected risk level
        self.app_state.permission_mode = match risk_level {
            RiskLevelSelection::Low => PermissionMode::Low,
            RiskLevelSelection::Medium => PermissionMode::Medium,
            RiskLevelSelection::High => PermissionMode::High,
        };

        // Sync permission mode with the manager
        self.sync_permission_mode();

        // Show toast notification about the mode change
        let mode_name = self.app_state.permission_mode.display_name();
        self.app_state
            .toasts
            .info(&format!("Risk level set to: {}", mode_name));

        // Now approve the tool
        self.handle_approve().await?;

        Ok(())
    }

    /// Handle terminal resize event
    fn handle_resize(
        &mut self,
        width: u16,
        height: u16,
        terminal: &mut CortexTerminal,
    ) -> Result<()> {
        self.app_state.terminal_size = (width, height);
        // Update TUI capture dimensions for proper debugging output
        self.tui_capture.update_dimensions(width, height);

        // Issue #2327: Terminal resize during code block streaming
        // Clear terminal completely to prevent rendering corruption from previous layout.
        // This includes clearing any partially rendered code blocks that need re-wrapping.
        terminal.clear()?;

        // Reset any cached line wrap calculations by forcing content reflow
        // This ensures code blocks are properly re-wrapped for the new terminal width
        self.app_state.invalidate_content_layout();

        // Force a full re-render with new dimensions
        self.render(terminal)?;

        Ok(())
    }

    /// Handle paste event
    fn handle_paste(&mut self, text: &str, terminal: &mut CortexTerminal) -> Result<()> {
        // Check modal stack first (unified modal system)
        if self.modal_stack.is_active() && self.modal_stack.handle_paste(text) {
            // Paste was handled by modal
        } else if let Some(crate::app::ActiveModal::Form(ref mut form_state)) =
            self.app_state.active_modal
        {
            // Check if a form modal is active and route paste to it
            form_state.handle_paste(text);
        } else {
            // Otherwise insert pasted text into the main input widget
            self.app_state.input.insert_str(text);
        }
        self.render(terminal)?;
        Ok(())
    }

    /// Handle Ctrl+Z suspend (Unix only)
    fn handle_suspend(&mut self) {
        #[cfg(unix)]
        {
            use crossterm::{
                cursor,
                event::{DisableBracketedPaste, DisableMouseCapture},
                execute,
                terminal::{LeaveAlternateScreen, disable_raw_mode},
            };
            let mut stdout = std::io::stdout();
            let _ = execute!(stdout, cursor::Show);
            let _ = execute!(stdout, DisableBracketedPaste);
            let _ = execute!(stdout, DisableMouseCapture);
            let _ = execute!(stdout, LeaveAlternateScreen);
            let _ = disable_raw_mode();

            // Send SIGSTOP to ourselves to actually suspend
            unsafe {
                libc::raise(libc::SIGSTOP);
            }
        }
    }

    /// Handle resume after suspend (Unix only)
    fn handle_resume(&mut self, terminal: &mut CortexTerminal) -> Result<()> {
        #[cfg(unix)]
        {
            use crossterm::{
                cursor,
                event::{EnableBracketedPaste, EnableMouseCapture},
                execute,
                terminal::{EnterAlternateScreen, enable_raw_mode},
            };
            let mut stdout = std::io::stdout();
            let _ = enable_raw_mode();
            let _ = execute!(stdout, EnterAlternateScreen);
            let _ = execute!(stdout, EnableMouseCapture);
            let _ = execute!(stdout, EnableBracketedPaste);
            let _ = execute!(stdout, cursor::Hide);

            // Force a full redraw
            terminal.clear()?;
            self.render(terminal)?;
        }
        Ok(())
    }

    // ========================================================================
    // BACKEND EVENT HANDLING
    // ========================================================================

    /// Handles an event from the cortex-core backend.
    ///
    /// Converts the protocol event to an application event and dispatches it.
    pub(super) fn _handle_backend_event(&mut self, event: cortex_protocol::Event) -> Result<()> {
        // Convert to AppEvent
        if let Some(app_event) = adapt_event(event) {
            self._handle_app_event(app_event)?;
        }
        Ok(())
    }

    /// Handles an application-level event.
    ///
    /// This method processes streaming events, tool events, message events,
    /// and other high-level application events converted from backend events.
    fn _handle_app_event(&mut self, event: AppEvent) -> Result<()> {
        match event {
            AppEvent::StreamingStarted => {
                self.stream_controller.start_processing();
                // Don't reset timer here - this is triggered by backend TaskStarted event
                // which could be either a new prompt or a continuation
                self.app_state.start_streaming(None, false);
            }

            AppEvent::StreamingChunk(chunk) => {
                self.stream_controller.append_text(&chunk);
            }

            AppEvent::StreamingCompleted => {
                self.stream_controller.complete();
                self.app_state.stop_streaming();

                // Create message from accumulated content
                let content = self.stream_controller.committed_text().to_string();
                if !content.is_empty() {
                    let message = cortex_core::widgets::Message::assistant(content);
                    self.app_state.add_message(message);
                }
                self.stream_controller.reset();
            }

            AppEvent::StreamingError(err) => {
                self.stream_controller.set_error(err.clone());
                self.app_state.stop_streaming();
                tracing::error!("Streaming error: {}", err);
            }

            AppEvent::MessageReceived(msg) => {
                self.app_state.add_message(msg);
            }

            AppEvent::ToolStarted { name, args: _ } => {
                self.stream_controller.start_tool(name.clone());
                self.app_state.streaming.current_tool = Some(name);
            }

            AppEvent::ToolCompleted { name: _, result: _ } => {
                self.app_state.streaming.current_tool = None;
            }

            AppEvent::ToolApprovalRequired { name, args, diff } => {
                self.stream_controller.wait_approval(name.clone());
                let args_str = serde_json::to_string_pretty(&args).unwrap_or_default();
                self.app_state.request_approval(name, args_str, diff);
            }

            AppEvent::ToolError { name, error } => {
                tracing::error!("Tool '{}' error: {}", name, error);
                self.app_state.streaming.current_tool = None;
            }

            AppEvent::ToolProgress { name: _, status: _ } => {
                // Tool progress updates are handled by stream controller
            }

            AppEvent::ToolApproved(_) | AppEvent::ToolRejected(_) => {
                // These are outgoing events, not incoming
            }

            AppEvent::Error(err) => {
                tracing::error!("Backend error: {}", err);
                // Could show error notification in UI
            }

            AppEvent::Warning(warning) => {
                tracing::warn!("Backend warning: {}", warning);
            }

            AppEvent::Info(info) => {
                tracing::info!("Backend info: {}", info);
            }

            AppEvent::SessionCreated(id) => {
                self.app_state.session_id = Some(id);
            }

            AppEvent::SessionLoaded(id) => {
                self.app_state.session_id = Some(id);
                self.app_state.set_view(AppView::Session);
            }

            AppEvent::Quit => {
                self.app_state.set_quit();
            }

            // Handle other events as needed
            _ => {
                tracing::debug!("Unhandled app event: {:?}", event);
            }
        }

        Ok(())
    }

    // ========================================================================
    // AUTOCOMPLETE
    // ========================================================================

    /// Updates the autocomplete state based on current input.
    ///
    /// Detects `/` for commands and `@` for mentions and populates
    /// the autocomplete popup accordingly.
    pub(super) fn update_autocomplete(&mut self) {
        use crate::commands::{CommandRegistry, CompletionEngine};
        use crate::widgets::filter_mentions;

        let text = self.app_state.input.text();

        // Find trigger character position
        let mut trigger_pos: Option<(usize, AutocompleteTrigger)> = None;

        // Look for the last trigger character that starts a word
        for (i, ch) in text.char_indices().rev() {
            match ch {
                '/' => {
                    // Check if at start or after whitespace
                    if i == 0
                        || text
                            .chars()
                            .nth(i - 1)
                            .map(|c| c.is_whitespace())
                            .unwrap_or(true)
                    {
                        trigger_pos = Some((i, AutocompleteTrigger::Command));
                        break;
                    }
                }
                '@' => {
                    // Check if at start or after whitespace
                    if i == 0
                        || text
                            .chars()
                            .nth(i - 1)
                            .map(|c| c.is_whitespace())
                            .unwrap_or(true)
                    {
                        trigger_pos = Some((i, AutocompleteTrigger::Mention));
                        break;
                    }
                }
                ' ' | '\n' | '\t' => {
                    // Stop looking if we hit whitespace without finding a trigger
                    break;
                }
                _ => continue,
            }
        }

        match trigger_pos {
            Some((pos, AutocompleteTrigger::Command)) => {
                // Show command completions
                let query = &text[pos + 1..]; // Text after /
                self.app_state
                    .autocomplete
                    .show(AutocompleteTrigger::Command, pos);
                self.app_state.autocomplete.set_query(query);

                // Get command completions
                let registry = CommandRegistry::default();
                let engine = CompletionEngine::new(&registry);
                let completions = engine.complete(&format!("/{}", query));

                let items: Vec<AutocompleteItem> = completions
                    .into_iter()
                    .take(10)
                    .map(|c| {
                        AutocompleteItem::new(&c.command, &c.display, &c.description)
                            .with_category(format!("{:?}", c.category))
                    })
                    .collect();

                self.app_state.autocomplete.set_items(items);
            }
            Some((pos, AutocompleteTrigger::Mention)) => {
                // Show mention completions
                let query = &text[pos + 1..]; // Text after @
                self.app_state
                    .autocomplete
                    .show(AutocompleteTrigger::Mention, pos);
                self.app_state.autocomplete.set_query(query);

                // Get mention completions
                let items = filter_mentions(query);
                self.app_state.autocomplete.set_items(items);
            }
            None => {
                // No trigger found, hide autocomplete
                if self.app_state.autocomplete.visible {
                    self.app_state.autocomplete.hide();
                }
            }
        }
    }
}
