//! Markdown Renderer - Main entry point for markdown rendering.
//!
//! This module provides the main `MarkdownRenderer` and `IncrementalMarkdownRenderer`
//! for converting markdown text to styled ratatui `Line`s.
//!
//! # Architecture
//!
//! The renderer uses pulldown-cmark for parsing and delegates to specialized
//! renderers for different block types:
//! - `CodeBlockRenderer` for fenced code blocks with syntax highlighting
//! - `TableBuilder` for tables with ASCII borders
//! - `ListContext` for ordered, unordered, and task lists
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_engine::markdown::{MarkdownRenderer, IncrementalMarkdownRenderer};
//!
//! // One-shot rendering
//! let renderer = MarkdownRenderer::new().with_width(80);
//! let lines = renderer.render("# Hello **World**");
//!
//! // Streaming/incremental rendering
//! let mut incremental = IncrementalMarkdownRenderer::new(renderer);
//! incremental.append("Some ");
//! incremental.append("content...");
//! let lines = incremental.get_lines();
//! ```

mod handlers;
mod helpers;
mod incremental;
mod state;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use pulldown_cmark::{Options, Parser};
use ratatui::text::Line;

use crate::markdown::code_block::CodeBlockRenderer;
use crate::markdown::theme::MarkdownTheme;

pub use self::incremental::IncrementalMarkdownRenderer;
use self::state::RenderState;

// Re-export helpers for tests
pub(crate) use self::helpers::{get_bullet, hash_string, heading_level_to_u8};

// ============================================================
// Main MarkdownRenderer
// ============================================================

/// Main markdown renderer.
///
/// Converts markdown text to styled ratatui `Line`s using pulldown-cmark
/// for parsing and specialized renderers for different block types.
#[derive(Clone)]
pub struct MarkdownRenderer {
    /// Theme for styling markdown elements.
    pub(crate) theme: Arc<MarkdownTheme>,
    /// Optional code block renderer with syntax highlighting.
    pub(crate) code_renderer: Option<Arc<CodeBlockRenderer>>,
    /// Maximum width for rendering (used for word wrapping).
    pub(crate) width: u16,
}

impl std::fmt::Debug for MarkdownRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkdownRenderer")
            .field("theme", &"<MarkdownTheme>")
            .field("code_renderer", &self.code_renderer.is_some())
            .field("width", &self.width)
            .finish()
    }
}

impl MarkdownRenderer {
    /// Creates a new markdown renderer with default theme.
    pub fn new() -> Self {
        Self {
            theme: Arc::new(MarkdownTheme::default()),
            code_renderer: None,
            width: 80,
        }
    }

    /// Creates a renderer with a custom theme.
    pub fn with_theme(theme: MarkdownTheme) -> Self {
        Self {
            theme: Arc::new(theme),
            code_renderer: None,
            width: 80,
        }
    }

    /// Sets a code block renderer for syntax highlighting.
    #[must_use]
    pub fn with_code_renderer(mut self, renderer: Arc<CodeBlockRenderer>) -> Self {
        self.code_renderer = Some(renderer);
        self
    }

    /// Sets the rendering width.
    #[must_use]
    pub fn with_width(mut self, width: u16) -> Self {
        self.width = width;
        self
    }

    /// Renders markdown to lines.
    ///
    /// This is the main entry point for rendering markdown content.
    ///
    /// # Arguments
    /// * `markdown` - The markdown source text
    ///
    /// # Returns
    /// A vector of styled `Line`s ready for display.
    pub fn render(&self, markdown: &str) -> Vec<Line<'static>> {
        let mut state = RenderState::new(self);
        let parser = Parser::new_ext(markdown, Self::options());

        for event in parser {
            state.handle_event(event);
        }

        state.finish()
    }

    /// Get pulldown-cmark options for parsing.
    fn options() -> Options {
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_HEADING_ATTRIBUTES
    }

    /// Returns the theme.
    pub fn theme(&self) -> &MarkdownTheme {
        &self.theme
    }

    /// Returns the width.
    pub fn width(&self) -> u16 {
        self.width
    }
}

impl Default for MarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}
