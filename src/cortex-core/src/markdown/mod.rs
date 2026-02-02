//! # Markdown Rendering Module
//!
//! Complete markdown rendering for the Cortex TUI with:
//! - Full CommonMark support via pulldown-cmark
//! - Syntax highlighting for code blocks via tree-sitter
//! - ASCII table rendering with complete borders
//! - Incremental rendering for streaming content
//!
//! ## Features
//!
//! - **Headers** (H1-H6) with distinct styles
//! - **Text formatting**: bold, italic, strikethrough, inline code
//! - **Code blocks** with syntax highlighting for 15+ languages
//! - **Tables** with full ASCII borders and alignment
//! - **Lists**: ordered, unordered, task lists with nesting
//! - **Blockquotes** with nested support
//! - **Links** with URL display
//! - **Horizontal rules**
//! - **Word wrapping** with unicode support
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    MarkdownRenderer                          │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
//! │  │   Theme     │  │  Renderer   │  │   IncrementalCache  │  │
//! │  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘  │
//! └─────────┼────────────────┼───────────────────┼──────────────┘
//!           │                │                   │
//!     ┌─────┴─────┐    ┌─────┴─────┐      ┌─────┴─────┐
//!     │           │    │           │      │           │
//!     ▼           ▼    ▼           ▼      ▼           ▼
//! ┌───────┐ ┌───────┐ ┌───────┐ ┌───────┐ ┌───────┐ ┌───────┐
//! │ Table │ │ Code  │ │ List  │ │Inline │ │Block- │ │  HR   │
//! │       │ │ Block │ │       │ │       │ │ quote │ │       │
//! └───────┘ └───────┘ └───────┘ └───────┘ └───────┘ └───────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use cortex_engine::markdown::{MarkdownRenderer, MarkdownTheme};
//!
//! // Create renderer with default theme
//! let renderer = MarkdownRenderer::new();
//!
//! // Render markdown to ratatui Lines
//! let lines = renderer.render("# Hello **World**", 80);
//!
//! // For streaming content, use incremental renderer
//! let mut incremental = IncrementalMarkdownRenderer::new(renderer);
//! incremental.append("Some ");
//! incremental.append("streaming ");
//! incremental.append("content...");
//! let lines = incremental.get_lines();
//! ```

// Sub-modules
pub mod code_block;
pub mod inline;
pub mod languages;
pub mod list;
pub mod renderer;
pub mod table;
pub mod theme;

// Re-exports for convenient access
pub use code_block::{CodeBlockRenderer, IncrementalCodeBlock};
pub use inline::{InlineStyleStack, parse_inline_spans, render_blockquote_prefix, render_hr};
pub use languages::{
    get_default_highlighter, is_language_available, normalize_language_name,
    register_common_languages,
};
pub use list::{ListContext, ListItem, render_list_item};
pub use renderer::{IncrementalMarkdownRenderer, MarkdownRenderer};
pub use table::{Alignment, Table, TableBuilder, TableCell, render_table};
pub use theme::MarkdownTheme;

/// Convenience function to render markdown with default settings.
///
/// # Arguments
/// * `markdown` - The markdown source text
/// * `width` - Maximum width for rendering (for word wrapping)
///
/// # Returns
/// A vector of ratatui `Line`s ready for display.
pub fn render_markdown(markdown: &str, width: u16) -> Vec<ratatui::text::Line<'static>> {
    let renderer = MarkdownRenderer::new().with_width(width);
    renderer.render(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_structure() {
        // Verify all modules are accessible
        let _theme = MarkdownTheme::default();
    }

    #[test]
    fn test_render_simple() {
        let lines = render_markdown("Hello **world**", 80);
        assert!(!lines.is_empty());
    }
}
