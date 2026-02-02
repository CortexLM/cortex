//! Approval Overlay Widget
//!
//! A modal overlay widget that requests user approval for agent actions.
//! Replaces the input area when approval is needed.
//!
//! ## Usage
//!
//! ```ignore
//! use cortex_tui::widgets::{ApprovalOverlay, ApprovalRequest, ApprovalDecision};
//! use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
//!
//! let request = ApprovalRequest::Exec {
//!     id: "cmd-1".to_string(),
//!     command: vec!["git".into(), "add".into(), "src/main.rs".into()],
//!     reason: Some("Adding modified files".to_string()),
//! };
//!
//! let mut overlay = ApprovalOverlay::new(request);
//!
//! // Handle key events
//! if let Some((id, decision)) = overlay.handle_key(key_event) {
//!     match decision {
//!         ApprovalDecision::Approved => { /* proceed with action */ }
//!         ApprovalDecision::ApprovedForSession => { /* proceed and remember */ }
//!         ApprovalDecision::Rejected => { /* cancel and let user respond */ }
//!     }
//! }
//! ```

use std::path::PathBuf;

use cortex_core::style::{CYAN_PRIMARY, SURFACE_0, TEXT, TEXT_DIM, TEXT_MUTED, VOID};
#[cfg(test)]
use crossterm::event::KeyModifiers;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

use super::{SelectionItem, SelectionList, SelectionResult};

// ============================================================
// APPROVAL REQUEST TYPES
// ============================================================

/// Type of file change in a patch request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeType {
    /// New file being added
    Add,
    /// Existing file being modified
    Modify,
    /// File being deleted
    Delete,
}

impl ChangeType {
    /// Get a display symbol for this change type.
    pub fn symbol(&self) -> &'static str {
        match self {
            ChangeType::Add => "+",
            ChangeType::Modify => "~",
            ChangeType::Delete => "-",
        }
    }
}

/// A single file change in a patch request.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path to the file being changed
    pub path: PathBuf,
    /// Type of change
    pub change_type: ChangeType,
    /// Number of lines added
    pub additions: usize,
    /// Number of lines deleted
    pub deletions: usize,
}

impl FileChange {
    /// Create a new file change.
    pub fn new(path: PathBuf, change_type: ChangeType, additions: usize, deletions: usize) -> Self {
        Self {
            path,
            change_type,
            additions,
            deletions,
        }
    }

    /// Format the change summary for display (e.g., "+15 -3").
    pub fn summary(&self) -> String {
        format!("+{} -{}", self.additions, self.deletions)
    }
}

/// Request coming from the agent that needs user approval.
#[derive(Debug, Clone)]
pub enum ApprovalRequest {
    /// Approve a command execution
    Exec {
        /// Unique identifier for this request
        id: String,
        /// Command and arguments to execute
        command: Vec<String>,
        /// Optional reason explaining why this command is needed
        reason: Option<String>,
    },
    /// Approve file changes (patch)
    ApplyPatch {
        /// Unique identifier for this request
        id: String,
        /// Optional reason explaining why these changes are needed
        reason: Option<String>,
        /// List of file changes to apply
        changes: Vec<FileChange>,
    },
}

impl ApprovalRequest {
    /// Get the unique identifier for this request.
    pub fn id(&self) -> &str {
        match self {
            ApprovalRequest::Exec { id, .. } => id,
            ApprovalRequest::ApplyPatch { id, .. } => id,
        }
    }
}

// ============================================================
// APPROVAL DECISION
// ============================================================

/// User's decision on an approval request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Approved for this single action
    Approved,
    /// Approved and don't ask again for similar actions this session
    ApprovedForSession,
    /// Rejected - user wants to provide different instructions
    Rejected,
}

// ============================================================
// APPROVAL OPTIONS
// ============================================================

/// Internal representation of an approval option.
#[derive(Debug, Clone)]
struct ApprovalOption {
    /// Display label
    label: String,
    /// Keyboard shortcut
    shortcut: char,
    /// Decision this option represents
    decision: ApprovalDecision,
}

/// Get options for exec approval.
fn exec_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Yes, proceed".to_string(),
            shortcut: 'y',
            decision: ApprovalDecision::Approved,
        },
        ApprovalOption {
            label: "No, tell Orion what to do differently".to_string(),
            shortcut: 'n',
            decision: ApprovalDecision::Rejected,
        },
    ]
}

/// Get options for patch approval.
fn patch_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Yes, proceed".to_string(),
            shortcut: 'y',
            decision: ApprovalDecision::Approved,
        },
        ApprovalOption {
            label: "Yes, and don't ask again for these files".to_string(),
            shortcut: 'a',
            decision: ApprovalDecision::ApprovedForSession,
        },
        ApprovalOption {
            label: "No, tell Orion what to do differently".to_string(),
            shortcut: 'n',
            decision: ApprovalDecision::Rejected,
        },
    ]
}

// ============================================================
// APPROVAL OVERLAY
// ============================================================

/// Modal overlay asking the user to approve or deny one or more requests.
#[derive(Debug, Clone)]
pub struct ApprovalOverlay {
    /// Current request being displayed
    current_request: Option<ApprovalRequest>,
    /// Queue of pending requests
    queue: Vec<ApprovalRequest>,
    /// Selection list for options
    list: SelectionList,
    /// Available options for the current request
    options: Vec<ApprovalOption>,
    /// Whether the overlay is complete (no more requests)
    is_complete: bool,
}

impl ApprovalOverlay {
    /// Create a new approval overlay with the given request.
    pub fn new(request: ApprovalRequest) -> Self {
        let options = Self::get_options(&request);
        let items = Self::build_selection_items(&options);
        let list = SelectionList::new(items);

        Self {
            current_request: Some(request),
            queue: Vec::new(),
            list,
            options,
            is_complete: false,
        }
    }

    /// Enqueue an additional request.
    pub fn enqueue(&mut self, request: ApprovalRequest) {
        self.queue.push(request);
    }

    /// Handle a key event and optionally return the decision.
    ///
    /// Returns `Some((id, decision))` if the user made a decision,
    /// `None` if the event was handled but no decision was made yet.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<(String, ApprovalDecision)> {
        if self.is_complete {
            return None;
        }

        // Check for Escape key (rejection)
        if key.code == KeyCode::Esc {
            return self.apply_selection_by_decision(ApprovalDecision::Rejected);
        }

        // Forward to selection list
        match self.list.handle_key(key) {
            SelectionResult::Selected(idx) => {
                if let Some(option) = self.options.get(idx) {
                    let decision = option.decision.clone();
                    return self.apply_decision(decision);
                }
            }
            SelectionResult::Cancelled => {
                return self.apply_selection_by_decision(ApprovalDecision::Rejected);
            }
            SelectionResult::None => {}
        }

        None
    }

    /// Check if the overlay is complete (no more requests).
    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    /// Get the current request being displayed.
    pub fn current_request(&self) -> Option<&ApprovalRequest> {
        self.current_request.as_ref()
    }

    /// Get the number of pending requests in the queue.
    pub fn pending_count(&self) -> usize {
        self.queue.len()
    }

    /// Render the overlay.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 20 {
            return;
        }

        let Some(request) = &self.current_request else {
            return;
        };

        // Clear background
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                buf[(x, y)].set_bg(SURFACE_0);
            }
        }

        let mut y = area.y;

        // Render header/title
        y = self.render_title(request, area.x, y, area.width, buf);
        y += 1; // Blank line

        // Render request-specific content
        y = match request {
            ApprovalRequest::Exec {
                command, reason, ..
            } => self.render_exec_content(command, reason.as_deref(), area.x, y, area.width, buf),
            ApprovalRequest::ApplyPatch {
                changes, reason, ..
            } => self.render_patch_content(changes, reason.as_deref(), area.x, y, area.width, buf),
        };

        y += 1; // Blank line

        // Render options list
        let options_height = self.options.len() as u16;
        let options_area = Rect::new(area.x, y, area.width, options_height.min(area.bottom() - y));
        self.render_options(options_area, buf);
    }

    /// Calculate the desired height for this overlay.
    pub fn desired_height(&self) -> u16 {
        let Some(request) = &self.current_request else {
            return 0;
        };

        let header_lines = 2; // Title + blank line
        let content_lines = match request {
            ApprovalRequest::Exec { reason, .. } => {
                let reason_lines = if reason.is_some() { 2 } else { 0 };
                1 + reason_lines // Command line + reason
            }
            ApprovalRequest::ApplyPatch {
                changes, reason, ..
            } => {
                let reason_lines = if reason.is_some() { 2 } else { 0 };
                changes.len() as u16 + reason_lines
            }
        };
        let options_lines = self.options.len() as u16;
        let spacing = 2; // Blank lines between sections

        header_lines + content_lines + options_lines + spacing
    }

    // --------------------------------------------------------
    // Private helpers
    // --------------------------------------------------------

    /// Get the appropriate options for a request type.
    fn get_options(request: &ApprovalRequest) -> Vec<ApprovalOption> {
        match request {
            ApprovalRequest::Exec { .. } => exec_options(),
            ApprovalRequest::ApplyPatch { .. } => patch_options(),
        }
    }

    /// Build selection items from options.
    fn build_selection_items(options: &[ApprovalOption]) -> Vec<SelectionItem> {
        options
            .iter()
            .map(|opt| SelectionItem::new(&opt.label).with_shortcut(opt.shortcut))
            .collect()
    }

    /// Apply a decision by finding the matching option.
    fn apply_selection_by_decision(
        &mut self,
        decision: ApprovalDecision,
    ) -> Option<(String, ApprovalDecision)> {
        self.apply_decision(decision)
    }

    /// Apply the given decision.
    fn apply_decision(&mut self, decision: ApprovalDecision) -> Option<(String, ApprovalDecision)> {
        let id = self.current_request.as_ref()?.id().to_string();
        self.advance_queue();
        Some((id, decision))
    }

    /// Advance to the next request in the queue.
    fn advance_queue(&mut self) {
        if let Some(next) = self.queue.pop() {
            self.options = Self::get_options(&next);
            let items = Self::build_selection_items(&self.options);
            self.list = SelectionList::new(items);
            self.current_request = Some(next);
        } else {
            self.current_request = None;
            self.is_complete = true;
        }
    }

    /// Render the title line.
    fn render_title(
        &self,
        request: &ApprovalRequest,
        x: u16,
        y: u16,
        _width: u16,
        buf: &mut Buffer,
    ) -> u16 {
        let title = match request {
            ApprovalRequest::Exec { .. } => "Would you like to run the following command?",
            ApprovalRequest::ApplyPatch { .. } => "Would you like to make the following edits?",
        };

        let style = Style::default()
            .fg(TEXT)
            .bg(SURFACE_0)
            .add_modifier(Modifier::BOLD);
        buf.set_string(x, y, title, style);

        y + 1
    }

    /// Render exec request content.
    fn render_exec_content(
        &self,
        command: &[String],
        reason: Option<&str>,
        x: u16,
        mut y: u16,
        width: u16,
        buf: &mut Buffer,
    ) -> u16 {
        // Render reason if present
        if let Some(reason_text) = reason {
            let reason_style = Style::default()
                .fg(TEXT_DIM)
                .bg(SURFACE_0)
                .add_modifier(Modifier::ITALIC);
            let reason_line = format!("Reason: {}", reason_text);
            let truncated = Self::truncate_str(&reason_line, width as usize);
            buf.set_string(x, y, truncated, reason_style);
            y += 2; // Extra blank line after reason
        }

        // Render command
        let cmd_str = Self::format_command(command);
        let cmd_display = format!("$ {}", cmd_str);
        let truncated = Self::truncate_str(&cmd_display, width as usize);

        // "$" prefix in cyan
        buf.set_string(x, y, "$", Style::default().fg(CYAN_PRIMARY).bg(SURFACE_0));
        // Command in bright text
        buf.set_string(
            x + 2,
            y,
            truncated[2..].trim_start(),
            Style::default().fg(TEXT).bg(SURFACE_0),
        );

        y
    }

    /// Render patch request content.
    fn render_patch_content(
        &self,
        changes: &[FileChange],
        reason: Option<&str>,
        x: u16,
        mut y: u16,
        width: u16,
        buf: &mut Buffer,
    ) -> u16 {
        // Render reason if present
        if let Some(reason_text) = reason {
            let reason_style = Style::default()
                .fg(TEXT_DIM)
                .bg(SURFACE_0)
                .add_modifier(Modifier::ITALIC);
            let reason_line = format!("Reason: {}", reason_text);
            let truncated = Self::truncate_str(&reason_line, width as usize);
            buf.set_string(x, y, truncated, reason_style);
            y += 2; // Extra blank line after reason
        }

        // Render each file change
        for change in changes {
            let path_str = change.path.display().to_string();
            let summary = change.summary();

            // Color code based on change type
            let (_type_color, summary_style) = match change.change_type {
                ChangeType::Add => (
                    cortex_core::style::SUCCESS,
                    Style::default()
                        .fg(cortex_core::style::SUCCESS)
                        .bg(SURFACE_0),
                ),
                ChangeType::Modify => (
                    cortex_core::style::WARNING,
                    Style::default().fg(TEXT_DIM).bg(SURFACE_0),
                ),
                ChangeType::Delete => (
                    cortex_core::style::ERROR,
                    Style::default().fg(cortex_core::style::ERROR).bg(SURFACE_0),
                ),
            };

            // Path
            let path_style = Style::default().fg(TEXT).bg(SURFACE_0);
            let max_path_len = (width as usize).saturating_sub(summary.len() + 3);
            let truncated_path = Self::truncate_str(&path_str, max_path_len);
            buf.set_string(x, y, &truncated_path, path_style);

            // Summary (e.g., "+15 -3")
            let summary_x = x + truncated_path.len() as u16 + 1;
            if summary_x + summary.len() as u16 <= x + width {
                buf.set_string(summary_x, y, format!("({})", summary), summary_style);
            }

            y += 1;
        }

        y.saturating_sub(1)
    }

    /// Render the options list.
    fn render_options(&self, area: Rect, buf: &mut Buffer) {
        for (idx, option) in self.options.iter().enumerate() {
            let y = area.y + idx as u16;
            if y >= area.bottom() {
                break;
            }

            let is_selected = self.list.selected_index() == Some(idx);

            // Clear line
            for col in area.x..area.right() {
                buf[(col, y)].set_bg(if is_selected { CYAN_PRIMARY } else { SURFACE_0 });
            }

            let mut col = area.x;

            // Selection indicator
            let prefix = if is_selected { ">" } else { " " };
            let prefix_style = if is_selected {
                Style::default().fg(VOID).bg(CYAN_PRIMARY)
            } else {
                Style::default().fg(CYAN_PRIMARY).bg(SURFACE_0)
            };
            buf.set_string(col, y, prefix, prefix_style);
            col += 2;

            // Option label
            let label_style = if is_selected {
                Style::default().fg(VOID).bg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT).bg(SURFACE_0)
            };
            let max_label_len = (area.width as usize).saturating_sub(10);
            let truncated_label = Self::truncate_str(&option.label, max_label_len);
            buf.set_string(col, y, &truncated_label, label_style);

            // Shortcut hint on the right
            let shortcut_str = format!("[{}]", option.shortcut);
            let shortcut_x = area.right().saturating_sub(shortcut_str.len() as u16 + 1);
            let shortcut_style = if is_selected {
                Style::default().fg(VOID).bg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT_MUTED).bg(SURFACE_0)
            };
            if shortcut_x > col + truncated_label.len() as u16 + 2 {
                buf.set_string(shortcut_x, y, &shortcut_str, shortcut_style);
            }
        }
    }

    /// Format a command for display.
    fn format_command(command: &[String]) -> String {
        if command.is_empty() {
            return String::new();
        }

        // Join command parts, handling shell commands specially
        if command.len() >= 3
            && (command[0].ends_with("sh") || command[0].ends_with("bash"))
            && (command[1] == "-c" || command[1] == "-lc")
        {
            // It's a shell command, show just the actual command
            return command[2..].join(" ");
        }

        command.join(" ")
    }

    /// Truncate a string to fit within the given width.
    fn truncate_str(s: &str, max_len: usize) -> String {
        if s.len() <= max_len {
            s.to_string()
        } else if max_len > 3 {
            format!("{}...", &s[..max_len - 3])
        } else {
            s[..max_len].to_string()
        }
    }
}

impl Widget for &ApprovalOverlay {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render(area, buf);
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_exec_request() -> ApprovalRequest {
        ApprovalRequest::Exec {
            id: "test-1".to_string(),
            command: vec!["git".into(), "add".into(), "src/main.rs".into()],
            reason: Some("Adding modified files".to_string()),
        }
    }

    fn make_patch_request() -> ApprovalRequest {
        ApprovalRequest::ApplyPatch {
            id: "test-2".to_string(),
            reason: Some("Refactoring code".to_string()),
            changes: vec![
                FileChange::new(PathBuf::from("src/main.rs"), ChangeType::Modify, 15, 3),
                FileChange::new(PathBuf::from("src/lib.rs"), ChangeType::Add, 42, 0),
            ],
        }
    }

    #[test]
    fn test_new_overlay() {
        let overlay = ApprovalOverlay::new(make_exec_request());
        assert!(!overlay.is_complete());
        assert!(overlay.current_request().is_some());
        assert_eq!(overlay.pending_count(), 0);
    }

    #[test]
    fn test_enqueue() {
        let mut overlay = ApprovalOverlay::new(make_exec_request());
        overlay.enqueue(make_patch_request());
        assert_eq!(overlay.pending_count(), 1);
    }

    #[test]
    fn test_exec_options() {
        let options = exec_options();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].shortcut, 'y');
        assert_eq!(options[1].shortcut, 'n');
    }

    #[test]
    fn test_patch_options() {
        let options = patch_options();
        assert_eq!(options.len(), 3);
        assert_eq!(options[0].shortcut, 'y');
        assert_eq!(options[1].shortcut, 'a');
        assert_eq!(options[2].shortcut, 'n');
    }

    #[test]
    fn test_shortcut_approval() {
        let mut overlay = ApprovalOverlay::new(make_exec_request());
        let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        let result = overlay.handle_key(key);
        assert!(result.is_some());
        let (id, decision) = result.unwrap();
        assert_eq!(id, "test-1");
        assert_eq!(decision, ApprovalDecision::Approved);
        assert!(overlay.is_complete());
    }

    #[test]
    fn test_shortcut_rejection() {
        let mut overlay = ApprovalOverlay::new(make_exec_request());
        let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        let result = overlay.handle_key(key);
        assert!(result.is_some());
        let (id, decision) = result.unwrap();
        assert_eq!(id, "test-1");
        assert_eq!(decision, ApprovalDecision::Rejected);
    }

    #[test]
    fn test_escape_rejects() {
        let mut overlay = ApprovalOverlay::new(make_exec_request());
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = overlay.handle_key(key);
        assert!(result.is_some());
        let (_, decision) = result.unwrap();
        assert_eq!(decision, ApprovalDecision::Rejected);
    }

    #[test]
    fn test_queue_advancement() {
        let mut overlay = ApprovalOverlay::new(make_exec_request());
        overlay.enqueue(make_patch_request());

        // Approve first request
        let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        let result = overlay.handle_key(key);
        assert_eq!(result.unwrap().0, "test-1");

        // Should now show patch request
        assert!(!overlay.is_complete());
        assert!(matches!(
            overlay.current_request(),
            Some(ApprovalRequest::ApplyPatch { .. })
        ));

        // Approve second request
        let result = overlay.handle_key(key);
        assert_eq!(result.unwrap().0, "test-2");
        assert!(overlay.is_complete());
    }

    #[test]
    fn test_session_approval_for_patch() {
        let mut overlay = ApprovalOverlay::new(make_patch_request());
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        let result = overlay.handle_key(key);
        assert!(result.is_some());
        let (_, decision) = result.unwrap();
        assert_eq!(decision, ApprovalDecision::ApprovedForSession);
    }

    #[test]
    fn test_format_command() {
        // Simple command
        let cmd = vec!["git".into(), "add".into(), "file.rs".into()];
        assert_eq!(ApprovalOverlay::format_command(&cmd), "git add file.rs");

        // Shell command
        let cmd = vec!["/bin/bash".into(), "-c".into(), "echo hello".into()];
        assert_eq!(ApprovalOverlay::format_command(&cmd), "echo hello");

        // Empty command
        let cmd: Vec<String> = vec![];
        assert_eq!(ApprovalOverlay::format_command(&cmd), "");
    }

    #[test]
    fn test_file_change_summary() {
        let change = FileChange::new(PathBuf::from("test.rs"), ChangeType::Modify, 10, 5);
        assert_eq!(change.summary(), "+10 -5");
    }

    #[test]
    fn test_change_type_symbol() {
        assert_eq!(ChangeType::Add.symbol(), "+");
        assert_eq!(ChangeType::Modify.symbol(), "~");
        assert_eq!(ChangeType::Delete.symbol(), "-");
    }

    #[test]
    fn test_request_id() {
        let exec = make_exec_request();
        assert_eq!(exec.id(), "test-1");

        let patch = make_patch_request();
        assert_eq!(patch.id(), "test-2");
    }

    #[test]
    fn test_desired_height() {
        let overlay = ApprovalOverlay::new(make_exec_request());
        let height = overlay.desired_height();
        assert!(height >= 5); // At least header + command + 2 options

        let overlay = ApprovalOverlay::new(make_patch_request());
        let height = overlay.desired_height();
        assert!(height >= 7); // At least header + 2 files + 3 options
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(ApprovalOverlay::truncate_str("hello", 10), "hello");
        assert_eq!(ApprovalOverlay::truncate_str("hello world", 8), "hello...");
        assert_eq!(ApprovalOverlay::truncate_str("hi", 2), "hi");
    }

    #[test]
    fn test_render_does_not_panic() {
        let overlay = ApprovalOverlay::new(make_exec_request());
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        overlay.render(Rect::new(0, 0, 80, 20), &mut buf);
        // Just verify it doesn't panic
    }

    #[test]
    fn test_render_patch_does_not_panic() {
        let overlay = ApprovalOverlay::new(make_patch_request());
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        overlay.render(Rect::new(0, 0, 80, 20), &mut buf);
        // Just verify it doesn't panic
    }

    #[test]
    fn test_enter_selects() {
        let mut overlay = ApprovalOverlay::new(make_exec_request());
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = overlay.handle_key(key);
        assert!(result.is_some());
        let (_, decision) = result.unwrap();
        // First option is "Yes, proceed"
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[test]
    fn test_navigation() {
        let mut overlay = ApprovalOverlay::new(make_exec_request());

        // Move down
        overlay.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(overlay.list.selected_index(), Some(1));

        // Move back up
        overlay.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(overlay.list.selected_index(), Some(0));
    }
}
