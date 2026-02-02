//! # List Renderer
//!
//! Renders markdown lists (ordered, unordered, and task lists) with proper
//! nesting, indentation, and styling support.
//!
//! ## Features
//!
//! - Ordered lists with customizable starting numbers
//! - Unordered lists with depth-aware bullet characters
//! - Task lists with checkbox markers
//! - Mixed nested lists (ordered within unordered and vice versa)
//! - Proper indentation and alignment for continuation lines
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_engine::markdown::list::{ListItem, ListContext, render_list_item};
//! use ratatui::text::Span;
//! use ratatui::style::Style;
//!
//! let item = ListItem::new(vec![Span::raw("First item")])
//!     .with_children(vec![
//!         ListItem::new(vec![Span::raw("Nested item")]),
//!     ]);
//!
//! let mut ctx = ListContext::new_unordered();
//! let lines = render_list_item(
//!     &item,
//!     &mut ctx,
//!     Style::default(),
//!     Style::default(),
//!     Style::default(),
//!     Style::default(),
//!     Style::default(),
//! );
//! ```

use ratatui::style::Style;
use ratatui::text::{Line, Span};

// ============================================================
// CONSTANTS
// ============================================================

/// Maximum nesting depth to prevent infinite recursion
const MAX_DEPTH: usize = 10;

/// Bullet characters by depth level
const BULLETS: [&str; 4] = ["•", "◦", "▪", "▸"];

/// Spaces per indentation level
const INDENT_WIDTH: usize = 2;

// ============================================================
// HELPER FUNCTIONS
// ============================================================

/// Get the bullet character for a given depth level.
///
/// Cycles through bullet styles for deeply nested lists.
#[inline]
fn get_bullet(depth: usize) -> &'static str {
    BULLETS[depth.min(BULLETS.len() - 1)]
}

/// Format an ordered list marker (e.g., "1. ", "2. ")
fn format_ordered_marker(number: u64) -> String {
    format!("{}. ", number)
}

/// Format an unordered list marker with bullet
fn format_unordered_marker(depth: usize) -> String {
    format!("{} ", get_bullet(depth))
}

/// Format a task marker
fn format_task_marker(checked: bool) -> String {
    if checked {
        "[x] ".to_string()
    } else {
        "[ ] ".to_string()
    }
}

/// Create continuation indent for wrapped content.
///
/// This creates an indent string that aligns subsequent lines
/// with the content after the marker.
fn continuation_indent(ctx: &ListContext, marker_width: usize) -> String {
    " ".repeat(ctx.indent_width() + marker_width)
}

// ============================================================
// LIST TYPE
// ============================================================

/// Type of list (ordered or unordered)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListType {
    /// Ordered list with a starting number
    Ordered { start: u64 },
    /// Unordered bullet list
    Unordered,
}

impl ListType {
    /// Check if this is an ordered list
    #[inline]
    pub fn is_ordered(&self) -> bool {
        matches!(self, ListType::Ordered { .. })
    }

    /// Check if this is an unordered list
    #[inline]
    pub fn is_unordered(&self) -> bool {
        matches!(self, ListType::Unordered)
    }
}

// ============================================================
// LIST ITEM
// ============================================================

/// A list item with optional children and task state.
///
/// List items can contain inline content (as spans), nested child items,
/// and optionally be marked as task items with checked/unchecked state.
#[derive(Debug, Clone)]
pub struct ListItem {
    /// The content spans of this item
    pub content: Vec<Span<'static>>,
    /// Nested child items
    pub children: Vec<ListItem>,
    /// Task list state: None = not a task, Some(true) = checked, Some(false) = unchecked
    pub task_state: Option<bool>,
}

impl ListItem {
    /// Create a new list item with the given content.
    pub fn new(content: Vec<Span<'static>>) -> Self {
        Self {
            content,
            children: Vec::new(),
            task_state: None,
        }
    }

    /// Create an empty list item.
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Add children to this list item.
    pub fn with_children(mut self, children: Vec<ListItem>) -> Self {
        self.children = children;
        self
    }

    /// Mark this item as a task with the given checked state.
    pub fn with_task(mut self, checked: bool) -> Self {
        self.task_state = Some(checked);
        self
    }

    /// Check if this item is a task list item.
    #[inline]
    pub fn is_task(&self) -> bool {
        self.task_state.is_some()
    }

    /// Check if this task item is checked.
    ///
    /// Returns false if this is not a task item.
    #[inline]
    pub fn is_checked(&self) -> bool {
        self.task_state.unwrap_or(false)
    }

    /// Check if this item has children.
    #[inline]
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }

    /// Check if this item has content.
    #[inline]
    pub fn has_content(&self) -> bool {
        !self.content.is_empty()
    }
}

impl Default for ListItem {
    fn default() -> Self {
        Self::empty()
    }
}

// ============================================================
// LIST CONTEXT
// ============================================================

/// Saved state for a parent list when entering a nested list.
#[derive(Debug, Clone)]
struct SavedListState {
    /// The type of the parent list
    list_type: ListType,
    /// The current number when we entered the nested list
    current_number: u64,
}

/// Context for rendering lists.
///
/// Tracks the current nesting depth, list type, and numbering state
/// for proper rendering of complex nested list structures.
#[derive(Debug, Clone)]
pub struct ListContext {
    /// Current nesting depth (0 = top level)
    pub depth: usize,
    /// Whether this is an ordered list
    pub ordered: bool,
    /// Starting number for ordered lists
    pub start_number: u64,
    /// Current item number within the list
    pub current_number: u64,
    /// Parent list contexts (for mixed nested lists)
    parent_stack: Vec<SavedListState>,
}

impl ListContext {
    /// Create a new list context with default settings (unordered).
    pub fn new() -> Self {
        Self {
            depth: 0,
            ordered: false,
            start_number: 1,
            current_number: 1,
            parent_stack: Vec::new(),
        }
    }

    /// Create a new ordered list context.
    ///
    /// # Arguments
    /// * `start` - The starting number for the list (typically 1)
    pub fn new_ordered(start: u64) -> Self {
        Self {
            depth: 0,
            ordered: true,
            start_number: start,
            current_number: start,
            parent_stack: Vec::new(),
        }
    }

    /// Create a new unordered list context.
    pub fn new_unordered() -> Self {
        Self::new()
    }

    /// Enter a nested list.
    ///
    /// Pushes the current list type onto the parent stack and
    /// updates the context for the new nested list.
    pub fn push_list(&mut self, list_type: ListType) {
        // Save current state including current_number
        let current_type = if self.ordered {
            ListType::Ordered {
                start: self.start_number,
            }
        } else {
            ListType::Unordered
        };
        self.parent_stack.push(SavedListState {
            list_type: current_type,
            current_number: self.current_number,
        });

        // Update for new list
        self.depth = (self.depth + 1).min(MAX_DEPTH);
        match list_type {
            ListType::Ordered { start } => {
                self.ordered = true;
                self.start_number = start;
                self.current_number = start;
            }
            ListType::Unordered => {
                self.ordered = false;
                self.start_number = 1;
                self.current_number = 1;
            }
        }
    }

    /// Exit the current nested list.
    ///
    /// Pops the parent list type from the stack and restores
    /// the previous list context including the current number.
    pub fn pop_list(&mut self) {
        if let Some(saved_state) = self.parent_stack.pop() {
            self.depth = self.depth.saturating_sub(1);
            // Restore the current_number so parent list continues correctly
            self.current_number = saved_state.current_number;
            match saved_state.list_type {
                ListType::Ordered { start } => {
                    self.ordered = true;
                    self.start_number = start;
                }
                ListType::Unordered => {
                    self.ordered = false;
                    self.start_number = 1;
                }
            }
        }
    }

    /// Get the next item number and increment the counter.
    ///
    /// Returns the current number before incrementing.
    pub fn next_number(&mut self) -> u64 {
        let num = self.current_number;
        self.current_number += 1;
        num
    }

    /// Reset the context for a new list at the same level.
    ///
    /// Resets the current number to the start number.
    pub fn reset(&mut self) {
        self.current_number = self.start_number;
    }

    /// Get the current indentation string.
    ///
    /// Returns a string of spaces (2 spaces per nesting level).
    pub fn indent(&self) -> String {
        " ".repeat(self.indent_width())
    }

    /// Get the current indentation width in characters.
    #[inline]
    pub fn indent_width(&self) -> usize {
        self.depth * INDENT_WIDTH
    }

    /// Get the current list type.
    pub fn current_list_type(&self) -> ListType {
        if self.ordered {
            ListType::Ordered {
                start: self.start_number,
            }
        } else {
            ListType::Unordered
        }
    }

    /// Check if we're at the maximum nesting depth.
    #[inline]
    pub fn at_max_depth(&self) -> bool {
        self.depth >= MAX_DEPTH
    }
}

impl Default for ListContext {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// RENDER FUNCTIONS
// ============================================================

/// Render a single list item with its children.
///
/// This function recursively renders a list item and all its nested children,
/// producing a vector of styled `Line`s ready for display.
///
/// # Arguments
///
/// * `item` - The list item to render
/// * `ctx` - The list context (tracks depth, numbering, etc.)
/// * `bullet_style` - Style for unordered list bullets
/// * `number_style` - Style for ordered list numbers
/// * `task_checked_style` - Style for checked task markers `[x]`
/// * `task_unchecked_style` - Style for unchecked task markers `[ ]`
/// * `text_style` - Style for the item content text
///
/// # Returns
///
/// A vector of `Line`s representing the rendered list item and its children.
pub fn render_list_item(
    item: &ListItem,
    ctx: &mut ListContext,
    bullet_style: Style,
    number_style: Style,
    task_checked_style: Style,
    task_unchecked_style: Style,
    text_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Build the marker based on list type and task state
    let indent = ctx.indent();
    let (marker, marker_style) = if item.is_task() {
        let marker = format_task_marker(item.is_checked());
        let style = if item.is_checked() {
            task_checked_style
        } else {
            task_unchecked_style
        };
        (marker, style)
    } else if ctx.ordered {
        let num = ctx.next_number();
        (format_ordered_marker(num), number_style)
    } else {
        (format_unordered_marker(ctx.depth), bullet_style)
    };

    // If not a task but we're in ordered/unordered, we already incremented
    // If it's a task, we need to handle numbering for the parent context
    if !item.is_task() && !ctx.ordered {
        // Already handled above
    }

    // Build the line spans
    let mut spans = Vec::new();

    // Add indent
    if !indent.is_empty() {
        spans.push(Span::raw(indent.clone()));
    }

    // Add marker with style
    spans.push(Span::styled(marker.clone(), marker_style));

    // Add content with text style
    for span in &item.content {
        let mut styled_span = span.clone();
        // Apply text style as base, but preserve any existing styling
        if styled_span.style == Style::default() {
            styled_span.style = text_style;
        }
        spans.push(styled_span);
    }

    // Create the line (even if empty, to maintain structure)
    lines.push(Line::from(spans));

    // Render children recursively
    if item.has_children() && !ctx.at_max_depth() {
        // Determine child list type (default to unordered for children)
        let child_list_type = ListType::Unordered;
        ctx.push_list(child_list_type);

        for child in &item.children {
            let child_lines = render_list_item(
                child,
                ctx,
                bullet_style,
                number_style,
                task_checked_style,
                task_unchecked_style,
                text_style,
            );
            lines.extend(child_lines);
        }

        ctx.pop_list();
    }

    lines
}

/// Render multiple list items.
///
/// Convenience function for rendering a complete list.
///
/// # Arguments
///
/// * `items` - The list items to render
/// * `list_type` - The type of list (ordered or unordered)
/// * `bullet_style` - Style for unordered list bullets
/// * `number_style` - Style for ordered list numbers
/// * `task_checked_style` - Style for checked task markers
/// * `task_unchecked_style` - Style for unchecked task markers
/// * `text_style` - Style for the item content text
///
/// # Returns
///
/// A vector of `Line`s representing the rendered list.
pub fn render_list(
    items: &[ListItem],
    list_type: ListType,
    bullet_style: Style,
    number_style: Style,
    task_checked_style: Style,
    task_unchecked_style: Style,
    text_style: Style,
) -> Vec<Line<'static>> {
    let mut ctx = match list_type {
        ListType::Ordered { start } => ListContext::new_ordered(start),
        ListType::Unordered => ListContext::new_unordered(),
    };

    let mut lines = Vec::new();
    for item in items {
        let item_lines = render_list_item(
            item,
            &mut ctx,
            bullet_style,
            number_style,
            task_checked_style,
            task_unchecked_style,
            text_style,
        );
        lines.extend(item_lines);
    }

    lines
}

/// Render a list item with default styles.
///
/// Uses `Style::default()` for all styling parameters.
pub fn render_list_item_plain(item: &ListItem, ctx: &mut ListContext) -> Vec<Line<'static>> {
    render_list_item(
        item,
        ctx,
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    )
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_bullet() {
        assert_eq!(get_bullet(0), "•");
        assert_eq!(get_bullet(1), "◦");
        assert_eq!(get_bullet(2), "▪");
        assert_eq!(get_bullet(3), "▸");
        // Should cap at last bullet
        assert_eq!(get_bullet(4), "▸");
        assert_eq!(get_bullet(100), "▸");
    }

    #[test]
    fn test_format_ordered_marker() {
        assert_eq!(format_ordered_marker(1), "1. ");
        assert_eq!(format_ordered_marker(10), "10. ");
        assert_eq!(format_ordered_marker(100), "100. ");
    }

    #[test]
    fn test_format_unordered_marker() {
        assert_eq!(format_unordered_marker(0), "• ");
        assert_eq!(format_unordered_marker(1), "◦ ");
        assert_eq!(format_unordered_marker(2), "▪ ");
    }

    #[test]
    fn test_format_task_marker() {
        assert_eq!(format_task_marker(true), "[x] ");
        assert_eq!(format_task_marker(false), "[ ] ");
    }

    #[test]
    fn test_list_item_new() {
        let item = ListItem::new(vec![Span::raw("Test")]);
        assert!(item.has_content());
        assert!(!item.has_children());
        assert!(!item.is_task());
    }

    #[test]
    fn test_list_item_empty() {
        let item = ListItem::empty();
        assert!(!item.has_content());
        assert!(!item.has_children());
    }

    #[test]
    fn test_list_item_with_children() {
        let child = ListItem::new(vec![Span::raw("Child")]);
        let item = ListItem::new(vec![Span::raw("Parent")]).with_children(vec![child]);
        assert!(item.has_children());
        assert_eq!(item.children.len(), 1);
    }

    #[test]
    fn test_list_item_with_task() {
        let checked = ListItem::new(vec![Span::raw("Done")]).with_task(true);
        let unchecked = ListItem::new(vec![Span::raw("Todo")]).with_task(false);

        assert!(checked.is_task());
        assert!(checked.is_checked());

        assert!(unchecked.is_task());
        assert!(!unchecked.is_checked());
    }

    #[test]
    fn test_list_context_new() {
        let ctx = ListContext::new();
        assert_eq!(ctx.depth, 0);
        assert!(!ctx.ordered);
        assert_eq!(ctx.current_number, 1);
    }

    #[test]
    fn test_list_context_new_ordered() {
        let ctx = ListContext::new_ordered(5);
        assert!(ctx.ordered);
        assert_eq!(ctx.start_number, 5);
        assert_eq!(ctx.current_number, 5);
    }

    #[test]
    fn test_list_context_next_number() {
        let mut ctx = ListContext::new_ordered(1);
        assert_eq!(ctx.next_number(), 1);
        assert_eq!(ctx.next_number(), 2);
        assert_eq!(ctx.next_number(), 3);
        assert_eq!(ctx.current_number, 4);
    }

    #[test]
    fn test_list_context_reset() {
        let mut ctx = ListContext::new_ordered(1);
        ctx.next_number();
        ctx.next_number();
        ctx.reset();
        assert_eq!(ctx.current_number, 1);
    }

    #[test]
    fn test_list_context_indent() {
        let mut ctx = ListContext::new();
        assert_eq!(ctx.indent(), "");
        assert_eq!(ctx.indent_width(), 0);

        ctx.push_list(ListType::Unordered);
        assert_eq!(ctx.indent(), "  ");
        assert_eq!(ctx.indent_width(), 2);

        ctx.push_list(ListType::Unordered);
        assert_eq!(ctx.indent(), "    ");
        assert_eq!(ctx.indent_width(), 4);
    }

    #[test]
    fn test_list_context_push_pop() {
        let mut ctx = ListContext::new_unordered();
        assert_eq!(ctx.depth, 0);
        assert!(!ctx.ordered);

        ctx.push_list(ListType::Ordered { start: 1 });
        assert_eq!(ctx.depth, 1);
        assert!(ctx.ordered);

        ctx.push_list(ListType::Unordered);
        assert_eq!(ctx.depth, 2);
        assert!(!ctx.ordered);

        ctx.pop_list();
        assert_eq!(ctx.depth, 1);
        assert!(ctx.ordered);

        ctx.pop_list();
        assert_eq!(ctx.depth, 0);
        assert!(!ctx.ordered);
    }

    #[test]
    fn test_list_context_max_depth() {
        let mut ctx = ListContext::new();

        // Push beyond max depth
        for _ in 0..15 {
            ctx.push_list(ListType::Unordered);
        }

        assert!(ctx.at_max_depth());
        assert_eq!(ctx.depth, MAX_DEPTH);
    }

    #[test]
    fn test_list_type() {
        let ordered = ListType::Ordered { start: 1 };
        let unordered = ListType::Unordered;

        assert!(ordered.is_ordered());
        assert!(!ordered.is_unordered());

        assert!(!unordered.is_ordered());
        assert!(unordered.is_unordered());
    }

    #[test]
    fn test_render_simple_unordered_list() {
        let items = vec![
            ListItem::new(vec![Span::raw("Item 1")]),
            ListItem::new(vec![Span::raw("Item 2")]),
            ListItem::new(vec![Span::raw("Item 3")]),
        ];

        let lines = render_list(
            &items,
            ListType::Unordered,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert_eq!(lines.len(), 3);

        // Check first line contains bullet and text
        let line_str: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line_str.contains("•"));
        assert!(line_str.contains("Item 1"));
    }

    #[test]
    fn test_render_simple_ordered_list() {
        let items = vec![
            ListItem::new(vec![Span::raw("First")]),
            ListItem::new(vec![Span::raw("Second")]),
            ListItem::new(vec![Span::raw("Third")]),
        ];

        let lines = render_list(
            &items,
            ListType::Ordered { start: 1 },
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert_eq!(lines.len(), 3);

        let line1: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let line2: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        let line3: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();

        assert!(line1.contains("1."));
        assert!(line2.contains("2."));
        assert!(line3.contains("3."));
    }

    #[test]
    fn test_render_ordered_list_custom_start() {
        let items = vec![
            ListItem::new(vec![Span::raw("Item")]),
            ListItem::new(vec![Span::raw("Item")]),
        ];

        let lines = render_list(
            &items,
            ListType::Ordered { start: 5 },
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        let line1: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let line2: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();

        assert!(line1.contains("5."));
        assert!(line2.contains("6."));
    }

    #[test]
    fn test_render_nested_list() {
        let items = vec![ListItem::new(vec![Span::raw("Parent")]).with_children(vec![
            ListItem::new(vec![Span::raw("Child 1")]),
            ListItem::new(vec![Span::raw("Child 2")]),
        ])];

        let lines = render_list(
            &items,
            ListType::Unordered,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert_eq!(lines.len(), 3);

        // Parent should not be indented
        let parent_line: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(parent_line.starts_with("•"));

        // Children should be indented
        let child1_line: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        let child2_line: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(child1_line.starts_with("  ")); // 2 spaces indent
        assert!(child2_line.starts_with("  "));
    }

    #[test]
    fn test_render_task_list() {
        let items = vec![
            ListItem::new(vec![Span::raw("Completed task")]).with_task(true),
            ListItem::new(vec![Span::raw("Pending task")]).with_task(false),
        ];

        let lines = render_list(
            &items,
            ListType::Unordered,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert_eq!(lines.len(), 2);

        let checked_line: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let unchecked_line: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();

        assert!(checked_line.contains("[x]"));
        assert!(unchecked_line.contains("[ ]"));
    }

    #[test]
    fn test_render_nested_task_list() {
        let items = vec![
            ListItem::new(vec![Span::raw("Parent task")])
                .with_task(false)
                .with_children(vec![
                    ListItem::new(vec![Span::raw("Sub-task done")]).with_task(true),
                    ListItem::new(vec![Span::raw("Sub-task pending")]).with_task(false),
                ]),
        ];

        let lines = render_list(
            &items,
            ListType::Unordered,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert_eq!(lines.len(), 3);

        let parent: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let child1: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        let child2: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();

        assert!(parent.contains("[ ]"));
        assert!(child1.contains("[x]"));
        assert!(child2.contains("[ ]"));
    }

    #[test]
    fn test_render_deep_nesting() {
        // Create a deeply nested structure
        let mut item = ListItem::new(vec![Span::raw("Level 0")]);

        let mut current = &mut item;
        for i in 1..=5 {
            current.children = vec![ListItem::new(vec![Span::raw(format!("Level {}", i))])];
            current = &mut current.children[0];
        }

        let mut ctx = ListContext::new_unordered();
        let lines = render_list_item_plain(&item, &mut ctx);

        // Should have 6 lines (level 0 through level 5)
        assert_eq!(lines.len(), 6);

        // Check increasing indentation
        for (i, line) in lines.iter().enumerate() {
            let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let expected_indent = "  ".repeat(i);
            assert!(
                content.starts_with(&expected_indent),
                "Line {} should start with {} spaces, got: {:?}",
                i,
                i * 2,
                content
            );
        }
    }

    #[test]
    fn test_render_empty_item() {
        let items = vec![ListItem::empty()];

        let lines = render_list(
            &items,
            ListType::Unordered,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        // Should still render a line with just the bullet
        assert_eq!(lines.len(), 1);
        let content: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(content.contains("•"));
    }

    #[test]
    fn test_render_mixed_nested_lists() {
        // Ordered list with unordered children
        let items = vec![
            ListItem::new(vec![Span::raw("First")]).with_children(vec![
                ListItem::new(vec![Span::raw("Bullet 1")]),
                ListItem::new(vec![Span::raw("Bullet 2")]),
            ]),
            ListItem::new(vec![Span::raw("Second")]),
        ];

        let lines = render_list(
            &items,
            ListType::Ordered { start: 1 },
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert_eq!(lines.len(), 4);

        // First item should be numbered
        let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first.contains("1."));

        // Children should have bullets
        let child1: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        let child2: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(child1.contains("◦")); // Nested bullet
        assert!(child2.contains("◦"));

        // Second item should continue numbering
        let second: String = lines[3].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(second.contains("2."));
    }

    #[test]
    fn test_continuation_indent() {
        let ctx = ListContext::new();
        let indent = continuation_indent(&ctx, 2);
        assert_eq!(indent, "  "); // 0 (depth indent) + 2 (marker width)

        let mut ctx_nested = ListContext::new();
        ctx_nested.push_list(ListType::Unordered);
        let indent_nested = continuation_indent(&ctx_nested, 3);
        assert_eq!(indent_nested, "     "); // 2 (depth indent) + 3 (marker width)
    }

    #[test]
    fn test_list_item_default() {
        let item: ListItem = Default::default();
        assert!(!item.has_content());
        assert!(!item.has_children());
        assert!(!item.is_task());
    }

    #[test]
    fn test_list_context_default() {
        let ctx: ListContext = Default::default();
        assert_eq!(ctx.depth, 0);
        assert!(!ctx.ordered);
    }

    #[test]
    fn test_current_list_type() {
        let ctx = ListContext::new_unordered();
        assert_eq!(ctx.current_list_type(), ListType::Unordered);

        let ctx_ordered = ListContext::new_ordered(5);
        match ctx_ordered.current_list_type() {
            ListType::Ordered { start } => assert_eq!(start, 5),
            _ => panic!("Expected ordered list type"),
        }
    }

    #[test]
    fn test_pop_empty_stack() {
        let mut ctx = ListContext::new();
        // Should not panic when popping empty stack
        ctx.pop_list();
        assert_eq!(ctx.depth, 0);
    }
}
