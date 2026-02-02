//! Core trait definition for edit replacement strategies.

/// Trait for edit replacement strategies.
/// Each strategy implements a different approach to finding and replacing text.
pub trait EditStrategy: Send + Sync {
    /// Returns the name of this strategy
    fn name(&self) -> &'static str;

    /// Attempts to find `old` in `content` and replace it with `new`.
    /// Returns `Some(new_content)` if successful, `None` otherwise.
    fn try_replace(&self, content: &str, old: &str, new: &str) -> Option<String>;

    /// Attempts to find all occurrences of `old` in `content` and replace them with `new`.
    /// Returns `Some(new_content)` if at least one replacement was made.
    fn try_replace_all(&self, content: &str, old: &str, new: &str) -> Option<String> {
        // Default implementation: use single replacement for strategies that don't support replace_all
        self.try_replace(content, old, new)
    }

    /// Returns the confidence level of this strategy (0.0 - 1.0)
    fn confidence(&self) -> f64 {
        1.0
    }
}
