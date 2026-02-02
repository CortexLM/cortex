//! Modal dialog component.
//!
//! Overlay dialogs that capture focus until dismissed.

use crate::borders::{ROUNDED_BORDER, RoundedBorder};
use crate::key_hints::KeyHintsBar;
use cortex_core::style::{CYAN_PRIMARY, SURFACE_0};
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

/// Result from handling a modal key event.
pub enum ModalResult<T = ()> {
    /// Continue displaying the modal
    Continue,
    /// Close the modal
    Close,
    /// Close and return an action
    Action(T),
    /// Push a new modal onto the stack
    Push(Box<dyn ModalTrait>),
}

/// Actions that can be triggered by modals.
#[derive(Debug, Clone)]
pub enum ModalAction {
    /// Execute a command
    ExecuteCommand(String),
    /// Select an item by ID
    Select(String),
    /// Custom action with data
    Custom { action: String, data: String },
}

/// Trait for modal implementations.
pub trait ModalTrait: Send {
    /// Modal title
    fn title(&self) -> &str;

    /// Calculate desired height
    fn desired_height(&self, max_height: u16, width: u16) -> u16;

    /// Render the modal content
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Handle a key event
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult<ModalAction>;

    /// Key hints for this modal
    fn key_hints(&self) -> Vec<(&'static str, &'static str)>;

    /// Handle pasted text
    fn handle_paste(&mut self, _text: &str) -> bool {
        false
    }
}

/// A stack of modals for nested dialogs.
pub struct ModalStack {
    stack: Vec<Box<dyn ModalTrait>>,
}

impl ModalStack {
    /// Create an empty modal stack.
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a modal onto the stack.
    pub fn push(&mut self, modal: Box<dyn ModalTrait>) {
        self.stack.push(modal);
    }

    /// Pop the top modal.
    pub fn pop(&mut self) -> Option<Box<dyn ModalTrait>> {
        self.stack.pop()
    }

    /// Check if any modal is active.
    pub fn is_active(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Check if the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Get the current (top) modal.
    pub fn current(&self) -> Option<&dyn ModalTrait> {
        self.stack.last().map(|m| m.as_ref())
    }

    /// Get mutable reference to the current modal.
    pub fn current_mut(&mut self) -> Option<&mut Box<dyn ModalTrait>> {
        self.stack.last_mut()
    }

    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> ModalResult<ModalAction> {
        if let Some(modal) = self.current_mut() {
            let result = modal.handle_key(key);
            match result {
                ModalResult::Close => {
                    self.pop();
                    ModalResult::Continue
                }
                ModalResult::Push(new_modal) => {
                    self.push(new_modal);
                    ModalResult::Continue
                }
                ModalResult::Action(action) => {
                    self.pop();
                    ModalResult::Action(action)
                }
                other => other,
            }
        } else {
            ModalResult::Continue
        }
    }

    /// Handle pasted text.
    pub fn handle_paste(&mut self, text: &str) -> bool {
        if let Some(modal) = self.current_mut() {
            modal.handle_paste(text)
        } else {
            false
        }
    }

    /// Clear all modals.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// Get the number of modals.
    pub fn len(&self) -> usize {
        self.stack.len()
    }
}

impl Default for ModalStack {
    fn default() -> Self {
        Self::new()
    }
}

/// A simple modal container widget.
pub struct Modal<'a> {
    title: &'a str,
    width_percent: u16,
    height: u16,
    key_hints: Vec<(&'static str, &'static str)>,
}

impl<'a> Modal<'a> {
    /// Create a new modal.
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            width_percent: 60,
            height: 10,
            key_hints: Vec::new(),
        }
    }

    /// Set the width as percentage of screen.
    pub fn width_percent(mut self, percent: u16) -> Self {
        self.width_percent = percent.clamp(20, 90);
        self
    }

    /// Set the height in lines.
    pub fn height(mut self, height: u16) -> Self {
        self.height = height;
        self
    }

    /// Set key hints.
    pub fn key_hints(mut self, hints: Vec<(&'static str, &'static str)>) -> Self {
        self.key_hints = hints;
        self
    }

    /// Calculate the modal area centered in the given area.
    pub fn centered_area(&self, area: Rect) -> Rect {
        let width = (area.width * self.width_percent / 100).max(20);
        let height = self.height.min(area.height.saturating_sub(2));

        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;

        Rect::new(x, y, width, height)
    }

    /// Calculate the inner content area.
    pub fn inner(&self, area: Rect) -> Rect {
        let modal_area = self.centered_area(area);
        let mut inner = RoundedBorder::new().inner(modal_area);

        // Reserve space for key hints
        if !self.key_hints.is_empty() && inner.height > 1 {
            inner.height = inner.height.saturating_sub(1);
        }

        inner
    }
}

impl Widget for Modal<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let modal_area = self.centered_area(area);

        if modal_area.height < 3 || modal_area.width < 10 {
            return;
        }

        // Clear background
        Clear.render(modal_area, buf);

        // Fill with background color
        for y in modal_area.y..modal_area.bottom() {
            for x in modal_area.x..modal_area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_bg(SURFACE_0);
                }
            }
        }

        // Border with title
        let block = Block::default()
            .title(format!(" {} ", self.title))
            .title_style(
                Style::default()
                    .fg(CYAN_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_set(ROUNDED_BORDER)
            .border_style(Style::default().fg(CYAN_PRIMARY));

        block.render(modal_area, buf);

        // Key hints
        if !self.key_hints.is_empty() {
            let inner = RoundedBorder::new().inner(modal_area);
            let hints_area = Rect::new(
                inner.x,
                inner.y + inner.height.saturating_sub(1),
                inner.width,
                1,
            );

            if hints_area.height > 0 {
                KeyHintsBar::from_tuples(&self.key_hints).render(hints_area, buf);
            }
        }
    }
}

/// Builder for creating modals.
pub struct ModalBuilder<'a> {
    modal: Modal<'a>,
}

impl<'a> ModalBuilder<'a> {
    /// Create a new modal builder.
    pub fn new(title: &'a str) -> Self {
        Self {
            modal: Modal::new(title),
        }
    }

    /// Set width percentage.
    pub fn width_percent(mut self, percent: u16) -> Self {
        self.modal = self.modal.width_percent(percent);
        self
    }

    /// Set height.
    pub fn height(mut self, height: u16) -> Self {
        self.modal = self.modal.height(height);
        self
    }

    /// Set key hints.
    pub fn key_hints(mut self, hints: Vec<(&'static str, &'static str)>) -> Self {
        self.modal = self.modal.key_hints(hints);
        self
    }

    /// Build the modal.
    pub fn build(self) -> Modal<'a> {
        self.modal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modal_centered_area() {
        let modal = Modal::new("Test").width_percent(50).height(10);
        let area = Rect::new(0, 0, 100, 40);

        let centered = modal.centered_area(area);
        assert_eq!(centered.width, 50);
        assert_eq!(centered.height, 10);
        assert_eq!(centered.x, 25);
        assert_eq!(centered.y, 15);
    }

    #[test]
    fn test_modal_stack() {
        // Note: Can't easily test with Box<dyn ModalTrait> without a concrete implementation
        let stack = ModalStack::new();
        assert!(stack.is_empty());
        assert!(!stack.is_active());
    }

    #[test]
    fn test_modal_builder() {
        let modal = ModalBuilder::new("Test")
            .width_percent(70)
            .height(15)
            .key_hints(vec![("Enter", "Select")])
            .build();

        assert_eq!(modal.title, "Test");
        assert_eq!(modal.width_percent, 70);
        assert_eq!(modal.height, 15);
    }
}
