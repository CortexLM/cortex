//! Focus management for terminal UI components.
//!
//! This module provides a focus management system that tracks which UI element
//! currently has focus, supports tab navigation, and manages focus changes.

use smallvec::SmallVec;
use std::collections::HashMap;
use std::fmt;

/// A unique identifier for focusable elements.
pub type FocusId = u64;

/// Represents an element that can receive focus.
pub trait Focusable {
    /// Returns the unique focus ID of this element.
    fn focus_id(&self) -> FocusId;

    /// Returns true if this element can currently receive focus.
    fn is_focusable(&self) -> bool {
        true
    }

    /// Returns the tab index for this element.
    ///
    /// - Negative values: Not reachable via tab navigation
    /// - Zero: Follows document order
    /// - Positive: Tab order priority (lower values come first)
    fn tab_index(&self) -> i32 {
        0
    }

    /// Called when this element gains focus.
    fn on_focus(&mut self) {}

    /// Called when this element loses focus.
    fn on_blur(&mut self) {}
}

/// Events emitted by the focus manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusEvent {
    /// An element gained focus.
    Focused(FocusId),
    /// An element lost focus.
    Blurred(FocusId),
    /// Focus moved from one element to another.
    Changed {
        /// The element that lost focus (if any).
        from: Option<FocusId>,
        /// The element that gained focus (if any).
        to: Option<FocusId>,
    },
}

impl fmt::Display for FocusEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FocusEvent::Focused(id) => write!(f, "Focused({id})"),
            FocusEvent::Blurred(id) => write!(f, "Blurred({id})"),
            FocusEvent::Changed { from, to } => {
                write!(f, "Changed({from:?} -> {to:?})")
            }
        }
    }
}

/// A node in the focus tree representing a focusable element.
#[derive(Debug, Clone)]
struct FocusNode {
    /// Parent node ID (None for root elements).
    parent: Option<FocusId>,
    /// Child node IDs in order.
    children: SmallVec<[FocusId; 8]>,
    /// Whether this element can receive focus.
    focusable: bool,
    /// Tab index for navigation order.
    tab_index: i32,
}

impl FocusNode {
    fn new(focusable: bool, tab_index: i32) -> Self {
        Self {
            parent: None,
            children: SmallVec::new(),
            focusable,
            tab_index,
        }
    }
}

/// Configuration for the focus manager.
#[derive(Debug, Clone)]
pub struct FocusConfig {
    /// Whether to wrap around when tabbing past the last/first element.
    pub wrap_around: bool,
    /// Whether to trap focus within the current focus scope.
    pub trap_focus: bool,
    /// Whether to auto-focus the first focusable element.
    pub auto_focus_first: bool,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            wrap_around: true,
            trap_focus: false,
            auto_focus_first: false,
        }
    }
}

/// Manages focus state for a tree of focusable elements.
///
/// The focus manager maintains a tree structure of focusable elements and
/// provides methods for navigating between them, tracking focus state,
/// and handling focus-related events.
#[derive(Debug)]
pub struct FocusManager {
    /// All registered nodes indexed by their ID.
    nodes: HashMap<FocusId, FocusNode>,
    /// Root nodes (nodes without parents).
    roots: SmallVec<[FocusId; 4]>,
    /// Currently focused element.
    current_focus: Option<FocusId>,
    /// Stack of saved focus states for modal/overlay support.
    focus_stack: Vec<Option<FocusId>>,
    /// Focus scope stack for nested focus trapping.
    scope_stack: Vec<FocusId>,
    /// Configuration.
    config: FocusConfig,
    /// Counter for generating unique IDs.
    next_id: FocusId,
}

impl FocusManager {
    /// Creates a new focus manager with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(FocusConfig::default())
    }

    /// Creates a new focus manager with the specified configuration.
    #[must_use]
    pub fn with_config(config: FocusConfig) -> Self {
        Self {
            nodes: HashMap::new(),
            roots: SmallVec::new(),
            current_focus: None,
            focus_stack: Vec::new(),
            scope_stack: Vec::new(),
            config,
            next_id: 1,
        }
    }

    /// Generates a new unique focus ID.
    pub fn generate_id(&mut self) -> FocusId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Registers a focusable element.
    ///
    /// Returns the focus ID that was assigned to the element.
    pub fn register(
        &mut self,
        id: FocusId,
        parent: Option<FocusId>,
        focusable: bool,
        tab_index: i32,
    ) {
        let mut node = FocusNode::new(focusable, tab_index);
        node.parent = parent;

        if let Some(parent_id) = parent {
            if let Some(parent_node) = self.nodes.get_mut(&parent_id) {
                parent_node.children.push(id);
            }
        } else {
            self.roots.push(id);
        }

        self.nodes.insert(id, node);

        // Auto-focus first element if configured and nothing is focused
        if self.config.auto_focus_first && self.current_focus.is_none() && focusable {
            self.current_focus = Some(id);
        }
    }

    /// Unregisters a focusable element.
    ///
    /// If the element was focused, focus is cleared. Child elements are also removed.
    pub fn unregister(&mut self, id: FocusId) {
        // Clear focus if this element was focused
        if self.current_focus == Some(id) {
            self.current_focus = None;
        }

        // Remove from parent's children
        if let Some(node) = self.nodes.get(&id) {
            if let Some(parent_id) = node.parent {
                if let Some(parent) = self.nodes.get_mut(&parent_id) {
                    parent.children.retain(|child| *child != id);
                }
            }
        }

        // Remove from roots
        self.roots.retain(|root| *root != id);

        // Recursively unregister children
        if let Some(node) = self.nodes.remove(&id) {
            let children: Vec<_> = node.children.into_iter().collect();
            for child_id in children {
                self.unregister(child_id);
            }
        }
    }

    /// Updates the focusable state of an element.
    pub fn set_focusable(&mut self, id: FocusId, focusable: bool) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.focusable = focusable;

            // Clear focus if element became unfocusable while focused
            if !focusable && self.current_focus == Some(id) {
                self.current_focus = None;
            }
        }
    }

    /// Updates the tab index of an element.
    pub fn set_tab_index(&mut self, id: FocusId, tab_index: i32) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.tab_index = tab_index;
        }
    }

    /// Returns the currently focused element ID.
    #[must_use]
    pub fn current(&self) -> Option<FocusId> {
        self.current_focus
    }

    /// Returns true if the specified element is currently focused.
    #[must_use]
    pub fn is_focused(&self, id: FocusId) -> bool {
        self.current_focus == Some(id)
    }

    /// Returns true if any element is currently focused.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.current_focus.is_some()
    }

    /// Sets focus to the specified element.
    ///
    /// Returns a `FocusEvent::Changed` event describing the focus change,
    /// or `None` if no change occurred.
    pub fn focus(&mut self, id: FocusId) -> Option<FocusEvent> {
        // Check if element exists and is focusable
        let node = self.nodes.get(&id)?;
        if !node.focusable {
            return None;
        }

        // Check if already focused
        if self.current_focus == Some(id) {
            return None;
        }

        let previous = self.current_focus;
        self.current_focus = Some(id);

        Some(FocusEvent::Changed {
            from: previous,
            to: Some(id),
        })
    }

    /// Clears focus from all elements.
    ///
    /// Returns a `FocusEvent::Blurred` event if an element was focused.
    pub fn blur(&mut self) -> Option<FocusEvent> {
        let previous = self.current_focus.take()?;
        Some(FocusEvent::Blurred(previous))
    }

    /// Moves focus to the next focusable element in tab order.
    ///
    /// Returns the ID of the newly focused element, if any.
    pub fn focus_next(&mut self) -> Option<FocusId> {
        let focusable = self.get_tab_order();
        if focusable.is_empty() {
            return None;
        }

        let current_idx = self
            .current_focus
            .and_then(|id| focusable.iter().position(|&x| x == id));

        let next_idx = match current_idx {
            Some(idx) => {
                let next = idx + 1;
                if next >= focusable.len() {
                    if self.config.wrap_around {
                        0
                    } else {
                        return None;
                    }
                } else {
                    next
                }
            }
            None => 0,
        };

        let next_id = focusable[next_idx];
        self.current_focus = Some(next_id);
        Some(next_id)
    }

    /// Moves focus to the previous focusable element in tab order.
    ///
    /// Returns the ID of the newly focused element, if any.
    pub fn focus_previous(&mut self) -> Option<FocusId> {
        let focusable = self.get_tab_order();
        if focusable.is_empty() {
            return None;
        }

        let current_idx = self
            .current_focus
            .and_then(|id| focusable.iter().position(|&x| x == id));

        let prev_idx = match current_idx {
            Some(idx) => {
                if idx == 0 {
                    if self.config.wrap_around {
                        focusable.len() - 1
                    } else {
                        return None;
                    }
                } else {
                    idx - 1
                }
            }
            None => focusable.len() - 1,
        };

        let prev_id = focusable[prev_idx];
        self.current_focus = Some(prev_id);
        Some(prev_id)
    }

    /// Moves focus to the first focusable element.
    pub fn focus_first(&mut self) -> Option<FocusId> {
        let focusable = self.get_tab_order();
        let first = *focusable.first()?;
        self.current_focus = Some(first);
        Some(first)
    }

    /// Moves focus to the last focusable element.
    pub fn focus_last(&mut self) -> Option<FocusId> {
        let focusable = self.get_tab_order();
        let last = *focusable.last()?;
        self.current_focus = Some(last);
        Some(last)
    }

    /// Pushes the current focus state onto the stack.
    ///
    /// This is useful when opening modals or overlays that need to
    /// restore focus when closed.
    pub fn push_focus(&mut self) {
        self.focus_stack.push(self.current_focus);
    }

    /// Restores the previously pushed focus state.
    ///
    /// Returns the restored focus ID, if any.
    pub fn pop_focus(&mut self) -> Option<FocusId> {
        let saved = self.focus_stack.pop()?;
        self.current_focus = saved;
        saved
    }

    /// Enters a focus scope, trapping focus within a subtree.
    ///
    /// The scope is identified by the root element ID. Focus is moved to the
    /// first focusable element within the scope.
    pub fn enter_scope(&mut self, scope_root: FocusId) {
        self.push_focus();
        self.scope_stack.push(scope_root);

        // Focus first element in scope
        let scope_elements = self.get_scope_tab_order(scope_root);
        if let Some(&first) = scope_elements.first() {
            self.current_focus = Some(first);
        } else {
            self.current_focus = None;
        }
    }

    /// Exits the current focus scope and restores the previous focus.
    pub fn exit_scope(&mut self) -> Option<FocusId> {
        self.scope_stack.pop();
        self.pop_focus()
    }

    /// Returns the depth of the current focus scope stack.
    #[must_use]
    pub fn scope_depth(&self) -> usize {
        self.scope_stack.len()
    }

    /// Returns elements in tab order, respecting the current scope.
    fn get_tab_order(&self) -> Vec<FocusId> {
        if let Some(&scope_root) = self.scope_stack.last() {
            self.get_scope_tab_order(scope_root)
        } else {
            self.get_full_tab_order()
        }
    }

    /// Returns all focusable elements in tab order.
    fn get_full_tab_order(&self) -> Vec<FocusId> {
        let mut result = Vec::new();
        for &root in &self.roots {
            self.collect_focusable(root, &mut result);
        }
        self.sort_by_tab_index(&mut result);
        result
    }

    /// Returns focusable elements within a scope in tab order.
    fn get_scope_tab_order(&self, scope_root: FocusId) -> Vec<FocusId> {
        let mut result = Vec::new();
        self.collect_focusable(scope_root, &mut result);
        self.sort_by_tab_index(&mut result);
        result
    }

    /// Recursively collects focusable elements in tree order.
    fn collect_focusable(&self, node_id: FocusId, result: &mut Vec<FocusId>) {
        if let Some(node) = self.nodes.get(&node_id) {
            if node.focusable && node.tab_index >= 0 {
                result.push(node_id);
            }
            for &child_id in &node.children {
                self.collect_focusable(child_id, result);
            }
        }
    }

    /// Sorts elements by tab index while preserving document order for equal indices.
    fn sort_by_tab_index(&self, elements: &mut [FocusId]) {
        elements.sort_by(|&a, &b| {
            let ta = self.nodes.get(&a).map(|n| n.tab_index).unwrap_or(0);
            let tb = self.nodes.get(&b).map(|n| n.tab_index).unwrap_or(0);
            ta.cmp(&tb)
        });
    }

    /// Returns the parent ID of an element.
    #[must_use]
    pub fn parent_of(&self, id: FocusId) -> Option<FocusId> {
        self.nodes.get(&id).and_then(|n| n.parent)
    }

    /// Returns the children IDs of an element.
    #[must_use]
    pub fn children_of(&self, id: FocusId) -> Option<&[FocusId]> {
        self.nodes.get(&id).map(|n| n.children.as_slice())
    }

    /// Returns the number of registered elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns true if no elements are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Clears all registered elements and focus state.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.roots.clear();
        self.current_focus = None;
        self.focus_stack.clear();
        self.scope_stack.clear();
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Direction for focus navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusDirection {
    /// Move to the next element (Tab).
    Next,
    /// Move to the previous element (Shift+Tab).
    Previous,
    /// Move to the first element (Home).
    First,
    /// Move to the last element (End).
    Last,
    /// Move up in a 2D layout.
    Up,
    /// Move down in a 2D layout.
    Down,
    /// Move left in a 2D layout.
    Left,
    /// Move right in a 2D layout.
    Right,
}

impl FocusDirection {
    /// Returns true if this is a linear navigation direction (Next/Previous).
    #[must_use]
    pub fn is_linear(&self) -> bool {
        matches!(self, FocusDirection::Next | FocusDirection::Previous)
    }

    /// Returns true if this is a 2D navigation direction.
    #[must_use]
    pub fn is_spatial(&self) -> bool {
        matches!(
            self,
            FocusDirection::Up
                | FocusDirection::Down
                | FocusDirection::Left
                | FocusDirection::Right
        )
    }
}

/// Focus navigation in a 2D grid layout.
///
/// This helper manages focus for grid-based layouts where arrow key
/// navigation is expected.
#[derive(Debug)]
pub struct GridFocusNavigator {
    /// Number of columns in the grid.
    columns: usize,
    /// Number of rows in the grid.
    rows: usize,
    /// Currently focused cell (column, row).
    focus: (usize, usize),
    /// Whether to wrap at edges.
    wrap: bool,
}

impl GridFocusNavigator {
    /// Creates a new grid focus navigator.
    #[must_use]
    pub fn new(columns: usize, rows: usize) -> Self {
        Self {
            columns,
            rows,
            focus: (0, 0),
            wrap: true,
        }
    }

    /// Sets whether navigation wraps at grid edges.
    pub fn set_wrap(&mut self, wrap: bool) {
        self.wrap = wrap;
    }

    /// Returns the current focus position.
    #[must_use]
    pub fn current(&self) -> (usize, usize) {
        self.focus
    }

    /// Returns the linear index of the current focus position.
    #[must_use]
    pub fn current_index(&self) -> usize {
        self.focus.1 * self.columns + self.focus.0
    }

    /// Sets focus to a specific position.
    pub fn set_focus(&mut self, column: usize, row: usize) {
        self.focus = (
            column.min(self.columns.saturating_sub(1)),
            row.min(self.rows.saturating_sub(1)),
        );
    }

    /// Sets focus by linear index.
    pub fn set_focus_index(&mut self, index: usize) {
        let total = self.columns * self.rows;
        if total > 0 {
            let index = index.min(total - 1);
            self.focus = (index % self.columns, index / self.columns);
        }
    }

    /// Moves focus in the specified direction.
    ///
    /// Returns the new position if focus moved, or None if blocked.
    pub fn move_focus(&mut self, direction: FocusDirection) -> Option<(usize, usize)> {
        let (col, row) = self.focus;

        let new_pos = match direction {
            FocusDirection::Up => {
                if row > 0 {
                    Some((col, row - 1))
                } else if self.wrap && self.rows > 0 {
                    Some((col, self.rows - 1))
                } else {
                    None
                }
            }
            FocusDirection::Down => {
                if row + 1 < self.rows {
                    Some((col, row + 1))
                } else if self.wrap {
                    Some((col, 0))
                } else {
                    None
                }
            }
            FocusDirection::Left => {
                if col > 0 {
                    Some((col - 1, row))
                } else if self.wrap && self.columns > 0 {
                    Some((self.columns - 1, row))
                } else {
                    None
                }
            }
            FocusDirection::Right => {
                if col + 1 < self.columns {
                    Some((col + 1, row))
                } else if self.wrap {
                    Some((0, row))
                } else {
                    None
                }
            }
            FocusDirection::Next => {
                let idx = self.current_index() + 1;
                let total = self.columns * self.rows;
                if idx < total {
                    Some((idx % self.columns, idx / self.columns))
                } else if self.wrap {
                    Some((0, 0))
                } else {
                    None
                }
            }
            FocusDirection::Previous => {
                let idx = self.current_index();
                if idx > 0 {
                    let new_idx = idx - 1;
                    Some((new_idx % self.columns, new_idx / self.columns))
                } else if self.wrap {
                    let total = self.columns * self.rows;
                    if total > 0 {
                        let new_idx = total - 1;
                        Some((new_idx % self.columns, new_idx / self.columns))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            FocusDirection::First => Some((0, 0)),
            FocusDirection::Last => {
                if self.columns > 0 && self.rows > 0 {
                    Some((self.columns - 1, self.rows - 1))
                } else {
                    None
                }
            }
        };

        if let Some(pos) = new_pos {
            self.focus = pos;
        }

        new_pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_focus_manager_basic() {
        let mut fm = FocusManager::new();

        let id1 = fm.generate_id();
        let id2 = fm.generate_id();
        let id3 = fm.generate_id();

        fm.register(id1, None, true, 0);
        fm.register(id2, None, true, 0);
        fm.register(id3, None, false, 0); // Not focusable

        assert!(fm.focus(id1).is_some());
        assert!(fm.is_focused(id1));
        assert_eq!(fm.current(), Some(id1));

        // Can't focus unfocusable element
        assert!(fm.focus(id3).is_none());

        // Focus change
        assert!(fm.focus(id2).is_some());
        assert!(!fm.is_focused(id1));
        assert!(fm.is_focused(id2));
    }

    #[test]
    fn test_focus_navigation() {
        let mut fm = FocusManager::new();

        let id1 = fm.generate_id();
        let id2 = fm.generate_id();
        let id3 = fm.generate_id();

        fm.register(id1, None, true, 0);
        fm.register(id2, None, true, 0);
        fm.register(id3, None, true, 0);

        // Start with no focus
        assert!(fm.current().is_none());

        // Tab to first
        assert_eq!(fm.focus_next(), Some(id1));

        // Tab to second
        assert_eq!(fm.focus_next(), Some(id2));

        // Tab to third
        assert_eq!(fm.focus_next(), Some(id3));

        // Wrap around to first
        assert_eq!(fm.focus_next(), Some(id1));

        // Shift+Tab to last
        assert_eq!(fm.focus_previous(), Some(id3));
    }

    #[test]
    fn test_tab_index_ordering() {
        let mut fm = FocusManager::new();

        let id1 = fm.generate_id();
        let id2 = fm.generate_id();
        let id3 = fm.generate_id();

        // Register with different tab indices
        fm.register(id1, None, true, 2);
        fm.register(id2, None, true, 1);
        fm.register(id3, None, true, 0);

        // Should navigate in tab index order: id3, id2, id1
        assert_eq!(fm.focus_first(), Some(id3));
        assert_eq!(fm.focus_next(), Some(id2));
        assert_eq!(fm.focus_next(), Some(id1));
    }

    #[test]
    fn test_focus_stack() {
        let mut fm = FocusManager::new();

        let id1 = fm.generate_id();
        let id2 = fm.generate_id();

        fm.register(id1, None, true, 0);
        fm.register(id2, None, true, 0);

        fm.focus(id1);
        assert!(fm.is_focused(id1));

        // Push and change focus
        fm.push_focus();
        fm.focus(id2);
        assert!(fm.is_focused(id2));

        // Pop to restore
        let restored = fm.pop_focus();
        assert_eq!(restored, Some(id1));
        assert!(fm.is_focused(id1));
    }

    #[test]
    fn test_unregister() {
        let mut fm = FocusManager::new();

        let parent = fm.generate_id();
        let child = fm.generate_id();

        fm.register(parent, None, true, 0);
        fm.register(child, Some(parent), true, 0);

        fm.focus(child);
        assert!(fm.is_focused(child));

        // Unregister parent should also remove child
        fm.unregister(parent);
        assert!(fm.current().is_none());
        assert!(fm.is_empty());
    }

    #[test]
    fn test_grid_navigator() {
        let mut nav = GridFocusNavigator::new(3, 3);

        assert_eq!(nav.current(), (0, 0));
        assert_eq!(nav.current_index(), 0);

        // Move right
        nav.move_focus(FocusDirection::Right);
        assert_eq!(nav.current(), (1, 0));

        // Move down
        nav.move_focus(FocusDirection::Down);
        assert_eq!(nav.current(), (1, 1));

        // Move to end
        nav.move_focus(FocusDirection::Last);
        assert_eq!(nav.current(), (2, 2));

        // Move to start
        nav.move_focus(FocusDirection::First);
        assert_eq!(nav.current(), (0, 0));
    }

    #[test]
    fn test_grid_navigator_wrap() {
        let mut nav = GridFocusNavigator::new(3, 3);
        nav.set_wrap(true);

        // Wrap left from (0,0)
        nav.move_focus(FocusDirection::Left);
        assert_eq!(nav.current(), (2, 0));

        // Wrap up from (2,0)
        nav.move_focus(FocusDirection::Up);
        assert_eq!(nav.current(), (2, 2));
    }

    #[test]
    fn test_focus_scope() {
        let mut fm = FocusManager::new();

        let main = fm.generate_id();
        let modal = fm.generate_id();
        let modal_btn = fm.generate_id();

        fm.register(main, None, true, 0);
        fm.register(modal, None, true, 0);
        fm.register(modal_btn, Some(modal), true, 0);

        fm.focus(main);
        assert!(fm.is_focused(main));

        // Enter modal scope
        fm.enter_scope(modal);
        assert_eq!(fm.scope_depth(), 1);

        // Should auto-focus first element in scope
        // (modal itself is focusable)
        assert!(fm.is_focused(modal) || fm.is_focused(modal_btn));

        // Exit scope restores previous focus
        fm.exit_scope();
        assert_eq!(fm.scope_depth(), 0);
        assert!(fm.is_focused(main));
    }
}
