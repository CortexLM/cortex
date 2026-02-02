//! Syntax highlighting for Cortex TUI.
//!
//! This crate provides tree-sitter based syntax highlighting with support for:
//!
//! - Multiple languages via language registry
//! - Customizable themes (VS Code Dark default)
//! - Incremental highlighting for streaming content
//! - Integration with Cortex TUI's styled text system
//!
//! # Architecture
//!
//! The syntax highlighting system consists of several components:
//!
//! - [`Highlighter`]: Main entry point for highlighting code
//! - [`Theme`]: Maps tree-sitter capture names to visual styles
//! - [`LanguageRegistry`]: Manages language configurations
//! - [`HighlightedText`]: Output type containing styled spans
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui_syntax::{Highlighter, LanguageConfig, Theme};
//! use tree_sitter_rust;
//!
//! // Create a highlighter with the default theme
//! let highlighter = Highlighter::new();
//!
//! // Register a language (requires loading the grammar)
//! highlighter.register_language(
//!     "rust",
//!     LanguageConfig::new(
//!         tree_sitter_rust::LANGUAGE.into(),
//!         include_str!("queries/rust/highlights.scm"),
//!     ),
//! );
//!
//! // Highlight some code
//! let highlighted = highlighter.highlight("fn main() {}", "rust").unwrap();
//!
//! // Convert to styled text for rendering
//! let styled_text = highlighted.to_styled_text();
//! ```
//!
//! # Language Detection
//!
//! The crate includes built-in language detection from file extensions:
//!
//! ```rust
//! use cortex_tui_syntax::languages::language_from_path;
//!
//! assert_eq!(language_from_path("main.rs"), Some("rust"));
//! assert_eq!(language_from_path("script.py"), Some("python"));
//! ```
//!
//! # Themes
//!
//! Several built-in themes are available:
//!
//! ```rust
//! use cortex_tui_syntax::Theme;
//!
//! let dark = Theme::vscode_dark();  // Default
//! let monokai = Theme::monokai();
//! ```
//!
//! Custom themes can be created with the [`ThemeBuilder`]:
//!
//! ```rust
//! use cortex_tui_syntax::ThemeBuilder;
//! use cortex_tui_text::Color;
//!
//! let theme = ThemeBuilder::new()
//!     .fg("keyword", Color::BLUE)
//!     .italic("comment", Color::GREEN)
//!     .build();
//! ```
//!
//! # Incremental Highlighting
//!
//! For streaming content (e.g., LLM output), use [`IncrementalHighlighter`]:
//!
//! ```rust,ignore
//! use cortex_tui_syntax::IncrementalHighlighter;
//!
//! let mut incremental = IncrementalHighlighter::new(highlighter);
//! incremental.set_language("rust")?;
//!
//! // Append content as it arrives
//! incremental.append("fn main")?;
//! incremental.append("() {}")?;
//!
//! // Get highlighted text at any point
//! let highlighted = incremental.highlighted_text();
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::unused_self)]
#![allow(clippy::return_self_not_must_use)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::reversed_empty_ranges)]

pub mod highlighter;
pub mod languages;
pub mod span;
pub mod theme;

// Re-export main types at crate root
pub use highlighter::{
    HighlightError, Highlighter, IncrementalHighlighter, LanguageConfig, Result, SimpleHighlighter,
};
pub use languages::{
    extensions_for_language, global_registry, is_language_supported, language_from_extension,
    language_from_path, register_language, supported_languages, LanguageInfo, LanguageRegistry,
};
pub use span::{ByteRange, HighlightSpan, HighlightedText, RawHighlight};
pub use theme::{Theme, ThemeBuilder};

// Re-export tree-sitter types for language registration
pub use tree_sitter::{Language, Query};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exports() {
        // Ensure all main types are accessible
        let _highlighter = Highlighter::new();
        let _theme = Theme::default();
        let _registry = LanguageRegistry::new();
        let _span = HighlightSpan::plain("test", 0..4);
    }

    #[test]
    fn test_language_detection() {
        assert_eq!(language_from_extension("rs"), Some("rust"));
        assert_eq!(language_from_path("test.py"), Some("python"));
        assert!(is_language_supported("javascript"));
    }

    #[test]
    fn test_theme_defaults() {
        let theme = Theme::vscode_dark();
        assert!(!theme.is_empty());

        let monokai = Theme::monokai();
        assert!(!monokai.is_empty());
    }

    #[test]
    fn test_highlighted_text_conversion() {
        use cortex_tui_text::StyledText;

        let mut highlighted = HighlightedText::new();
        highlighted.push(HighlightSpan::plain("hello", 0..5));

        let styled: StyledText = highlighted.into();
        assert_eq!(styled.plain_text(), "hello");
    }
}
