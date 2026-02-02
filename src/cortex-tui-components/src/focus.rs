//! Focus management utilities.
//!
//! Provides focus cycling and management for multi-component layouts.

/// Direction of focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    /// Move focus forward (Tab, Down, Right)
    Forward,
    /// Move focus backward (Shift+Tab, Up, Left)
    Backward,
}

/// Manages focus state across multiple focusable elements.
///
/// # Example
///
/// ```rust
/// use cortex_tui_components::focus::FocusManager;
///
/// let mut focus = FocusManager::new(3); // 3 focusable elements
///
/// assert_eq!(focus.current(), 0);
///
/// focus.next();
/// assert_eq!(focus.current(), 1);
///
/// focus.next();
/// focus.next();
/// assert_eq!(focus.current(), 0); // Wrapped around
/// ```
#[derive(Debug, Clone)]
pub struct FocusManager {
    /// Current focused index
    current: usize,
    /// Total number of focusable elements
    count: usize,
    /// Whether focus should wrap around
    wrap: bool,
}

impl FocusManager {
    /// Create a new focus manager with the given number of elements.
    pub fn new(count: usize) -> Self {
        Self {
            current: 0,
            count,
            wrap: true,
        }
    }

    /// Set whether focus should wrap around at boundaries.
    pub fn with_wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// Get the current focused index.
    pub fn current(&self) -> usize {
        self.current
    }

    /// Set the current focused index.
    pub fn set(&mut self, index: usize) {
        if index < self.count {
            self.current = index;
        }
    }

    /// Move focus to the next element.
    pub fn next(&mut self) {
        if self.count == 0 {
            return;
        }

        if self.current + 1 < self.count {
            self.current += 1;
        } else if self.wrap {
            self.current = 0;
        }
    }

    /// Move focus to the previous element.
    pub fn prev(&mut self) {
        if self.count == 0 {
            return;
        }

        if self.current > 0 {
            self.current -= 1;
        } else if self.wrap {
            self.current = self.count.saturating_sub(1);
        }
    }

    /// Move focus in the given direction.
    pub fn move_focus(&mut self, direction: FocusDirection) {
        match direction {
            FocusDirection::Forward => self.next(),
            FocusDirection::Backward => self.prev(),
        }
    }

    /// Check if a given index is focused.
    pub fn is_focused(&self, index: usize) -> bool {
        self.current == index
    }

    /// Update the count of focusable elements.
    ///
    /// If the current focus is beyond the new count, it's moved to the last element.
    pub fn set_count(&mut self, count: usize) {
        self.count = count;
        if self.current >= count && count > 0 {
            self.current = count - 1;
        }
    }

    /// Get the total count of focusable elements.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Move focus to the first element.
    pub fn first(&mut self) {
        self.current = 0;
    }

    /// Move focus to the last element.
    pub fn last(&mut self) {
        if self.count > 0 {
            self.current = self.count - 1;
        }
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_focus_manager_basic() {
        let mut fm = FocusManager::new(3);

        assert_eq!(fm.current(), 0);
        assert!(fm.is_focused(0));

        fm.next();
        assert_eq!(fm.current(), 1);

        fm.next();
        assert_eq!(fm.current(), 2);

        fm.next();
        assert_eq!(fm.current(), 0); // Wrapped
    }

    #[test]
    fn test_focus_manager_prev() {
        let mut fm = FocusManager::new(3);

        fm.prev();
        assert_eq!(fm.current(), 2); // Wrapped to end

        fm.prev();
        assert_eq!(fm.current(), 1);
    }

    #[test]
    fn test_focus_manager_no_wrap() {
        let mut fm = FocusManager::new(3).with_wrap(false);

        fm.set(2);
        fm.next();
        assert_eq!(fm.current(), 2); // Didn't wrap

        fm.set(0);
        fm.prev();
        assert_eq!(fm.current(), 0); // Didn't wrap
    }

    #[test]
    fn test_focus_manager_set() {
        let mut fm = FocusManager::new(5);

        fm.set(3);
        assert_eq!(fm.current(), 3);

        // Setting beyond count should be ignored
        fm.set(10);
        assert_eq!(fm.current(), 3);
    }

    #[test]
    fn test_focus_manager_count_change() {
        let mut fm = FocusManager::new(5);
        fm.set(4);

        // Reduce count - focus should move
        fm.set_count(3);
        assert_eq!(fm.current(), 2);

        // Increase count - focus should stay
        fm.set_count(10);
        assert_eq!(fm.current(), 2);
    }

    #[test]
    fn test_focus_manager_first_last() {
        let mut fm = FocusManager::new(5);
        fm.set(2);

        fm.last();
        assert_eq!(fm.current(), 4);

        fm.first();
        assert_eq!(fm.current(), 0);
    }

    #[test]
    fn test_focus_direction() {
        let mut fm = FocusManager::new(3);

        fm.move_focus(FocusDirection::Forward);
        assert_eq!(fm.current(), 1);

        fm.move_focus(FocusDirection::Backward);
        assert_eq!(fm.current(), 0);
    }
}
