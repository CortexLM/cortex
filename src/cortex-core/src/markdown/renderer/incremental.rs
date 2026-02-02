//! IncrementalMarkdownRenderer - Incremental markdown rendering with caching.

use ratatui::text::Line;

use super::MarkdownRenderer;
use super::helpers::hash_string;

/// Incremental markdown renderer with intelligent caching.
///
/// This renderer caches the output and only re-renders when the source
/// content changes. Optimized for streaming scenarios where content is
/// appended frequently.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_engine::markdown::{MarkdownRenderer, IncrementalMarkdownRenderer};
///
/// let renderer = MarkdownRenderer::new();
/// let mut incremental = IncrementalMarkdownRenderer::new(renderer);
///
/// // Append content incrementally
/// incremental.append("# Hello ");
/// incremental.append("World\n\n");
/// incremental.append("Some **bold** text.");
///
/// // Get rendered lines (caches result)
/// let lines = incremental.get_lines();
///
/// // Subsequent calls return cached result if source unchanged
/// let lines2 = incremental.get_lines();
/// ```
#[derive(Debug)]
pub struct IncrementalMarkdownRenderer {
    /// The underlying renderer.
    renderer: MarkdownRenderer,
    /// The source markdown content.
    source: String,
    /// Hash of the source for quick change detection.
    source_hash: u64,
    /// Cached rendered result.
    cached_result: Option<Vec<Line<'static>>>,
    /// Whether the cache is dirty (needs re-render).
    dirty: bool,
    /// Last rendered width (to detect width changes).
    last_width: u16,
}

impl IncrementalMarkdownRenderer {
    /// Creates a new incremental renderer.
    pub fn new(renderer: MarkdownRenderer) -> Self {
        let last_width = renderer.width();
        Self {
            renderer,
            source: String::new(),
            source_hash: 0,
            cached_result: None,
            dirty: true,
            last_width,
        }
    }

    /// Set width for rendering.
    ///
    /// If width changes, marks the cache as dirty.
    pub fn set_width(&mut self, width: u16) {
        if self.last_width != width {
            self.renderer = self.renderer.clone().with_width(width);
            self.last_width = width;
            self.dirty = true;
        }
    }

    /// Set source (marks dirty if changed).
    ///
    /// Uses hash comparison for efficient change detection.
    pub fn set_source(&mut self, source: &str) {
        let new_hash = hash_string(source);
        if new_hash != self.source_hash || self.source != source {
            self.source.clear();
            self.source.push_str(source);
            self.source_hash = new_hash;
            self.dirty = true;
        }
    }

    /// Append to source (marks dirty).
    pub fn append(&mut self, content: &str) {
        self.source.push_str(content);
        self.source_hash = hash_string(&self.source);
        self.dirty = true;
    }

    /// Get rendered lines (re-renders if dirty).
    ///
    /// This method caches the result, so subsequent calls with unchanged
    /// content will return the cached lines.
    pub fn get_lines(&mut self) -> Vec<Line<'static>> {
        if self.dirty || self.cached_result.is_none() {
            let lines = self.renderer.render(&self.source);
            self.cached_result = Some(lines);
            self.dirty = false;
        }
        self.cached_result.clone().unwrap_or_default()
    }

    /// Check if needs re-render.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Force re-render on next get_lines.
    pub fn invalidate(&mut self) {
        self.dirty = true;
        self.cached_result = None;
    }

    /// Clear all content.
    pub fn clear(&mut self) {
        self.source.clear();
        self.source_hash = 0;
        self.cached_result = None;
        self.dirty = true;
    }

    /// Get current source.
    pub fn source(&self) -> &str {
        &self.source
    }
}
