//! Simple picker modals for selecting from a list of options.

use super::{Modal, ModalAction, ModalResult};
use crate::widgets::selection_list::{SelectionItem, SelectionList, SelectionResult};
use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

// ============================================================================
// APPROVAL PICKER
// ============================================================================

/// Modal for selecting approval mode (ask, session, always, never)
pub struct ApprovalPickerModal {
    list: SelectionList,
    _current: Option<String>,
}

impl ApprovalPickerModal {
    pub fn new(current: Option<String>) -> Self {
        let items = vec![
            SelectionItem::new("ask")
                .with_description("Ask for each tool call")
                .with_shortcut('a')
                .with_current(current.as_deref() == Some("ask")),
            SelectionItem::new("session")
                .with_description("Remember choice for this session")
                .with_shortcut('s')
                .with_current(current.as_deref() == Some("session")),
            SelectionItem::new("always")
                .with_description("Always approve automatically")
                .with_shortcut('y')
                .with_current(current.as_deref() == Some("always")),
            SelectionItem::new("never")
                .with_description("Never approve (reject all)")
                .with_shortcut('n')
                .with_current(current.as_deref() == Some("never")),
        ];

        Self {
            list: SelectionList::new(items),
            _current: current,
        }
    }
}

impl Modal for ApprovalPickerModal {
    fn title(&self) -> &str {
        "Approval Mode"
    }

    fn desired_height(&self, _max_height: u16, _width: u16) -> u16 {
        6 // 4 items + 2 for borders
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        (&self.list).render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match self.list.handle_key(key) {
            SelectionResult::Selected(idx) => {
                let modes = ["ask", "session", "always", "never"];
                if let Some(mode) = modes.get(idx) {
                    ModalResult::Action(ModalAction::SetApprovalMode(mode.to_string()))
                } else {
                    ModalResult::Close
                }
            }
            SelectionResult::Cancelled => ModalResult::Close,
            SelectionResult::None => ModalResult::Continue,
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("↑↓", "navigate"),
            ("Enter", "select"),
            ("a/s/y/n", "quick select"),
            ("Esc", "cancel"),
        ]
    }
}

// ============================================================================
// LOG LEVEL PICKER
// ============================================================================

/// Modal for selecting log level (trace, debug, info, warn, error)
pub struct LogLevelPickerModal {
    list: SelectionList,
    _current: Option<String>,
}

impl LogLevelPickerModal {
    pub fn new(current: Option<String>) -> Self {
        let items = vec![
            SelectionItem::new("trace")
                .with_description("Most verbose logging")
                .with_shortcut('t')
                .with_current(current.as_deref() == Some("trace")),
            SelectionItem::new("debug")
                .with_description("Debug information")
                .with_shortcut('d')
                .with_current(current.as_deref() == Some("debug")),
            SelectionItem::new("info")
                .with_description("General information")
                .with_shortcut('i')
                .with_current(current.as_deref() == Some("info")),
            SelectionItem::new("warn")
                .with_description("Warnings only")
                .with_shortcut('w')
                .with_current(current.as_deref() == Some("warn")),
            SelectionItem::new("error")
                .with_description("Errors only")
                .with_shortcut('e')
                .with_current(current.as_deref() == Some("error")),
        ];

        Self {
            list: SelectionList::new(items),
            _current: current,
        }
    }
}

impl Modal for LogLevelPickerModal {
    fn title(&self) -> &str {
        "Log Level"
    }

    fn desired_height(&self, _max_height: u16, _width: u16) -> u16 {
        7 // 5 items + 2 for borders
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        (&self.list).render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match self.list.handle_key(key) {
            SelectionResult::Selected(idx) => {
                let levels = ["trace", "debug", "info", "warn", "error"];
                if let Some(level) = levels.get(idx) {
                    ModalResult::Action(ModalAction::SetLogLevel(level.to_string()))
                } else {
                    ModalResult::Close
                }
            }
            SelectionResult::Cancelled => ModalResult::Close,
            SelectionResult::None => ModalResult::Continue,
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("↑↓", "navigate"),
            ("Enter", "select"),
            ("t/d/i/w/e", "quick select"),
            ("Esc", "cancel"),
        ]
    }
}
