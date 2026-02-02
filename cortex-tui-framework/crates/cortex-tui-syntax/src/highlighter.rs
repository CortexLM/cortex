//! Syntax highlighter using tree-sitter.
//!
//! Provides high-performance syntax highlighting with support for incremental updates
//! and streaming content.

use crate::languages::{language_from_extension, language_from_path, LanguageRegistry};
use crate::span::{ByteRange, HighlightSpan, HighlightedText, RawHighlight};
use crate::theme::Theme;
use ahash::AHashMap;
use cortex_tui_text::Style;
use parking_lot::RwLock;
use std::path::Path;
use std::sync::Arc;
use streaming_iterator::StreamingIterator;
use thiserror::Error;
use tree_sitter::{Language, Parser, Query, QueryCursor, Tree};

/// Errors that can occur during highlighting.
#[derive(Debug, Error)]
pub enum HighlightError {
    /// The language is not registered or supported.
    #[error("language not found: {0}")]
    LanguageNotFound(String),

    /// Failed to parse the source code.
    #[error("parse error")]
    ParseError,

    /// Failed to compile the highlight query.
    #[error("query error: {0}")]
    QueryError(#[from] tree_sitter::QueryError),

    /// The language grammar failed to load.
    #[error("failed to set language: {0}")]
    LanguageError(#[from] tree_sitter::LanguageError),

    /// Invalid byte range.
    #[error("invalid range: {0}..{1}")]
    InvalidRange(usize, usize),
}

/// Result type for highlighting operations.
pub type Result<T> = std::result::Result<T, HighlightError>;

/// Configuration for a language's highlighting.
#[derive(Debug, Clone)]
pub struct LanguageConfig {
    /// The tree-sitter language.
    pub language: Language,
    /// The highlight query.
    pub highlight_query: String,
    /// Optional injection query for embedded languages.
    pub injection_query: Option<String>,
}

impl LanguageConfig {
    /// Creates a new language configuration.
    pub fn new(language: Language, highlight_query: impl Into<String>) -> Self {
        Self {
            language,
            highlight_query: highlight_query.into(),
            injection_query: None,
        }
    }

    /// Sets the injection query.
    pub fn with_injection_query(mut self, query: impl Into<String>) -> Self {
        self.injection_query = Some(query.into());
        self
    }
}

/// Cached parser and query for a language.
struct CachedLanguage {
    parser: Parser,
    query: Query,
    #[allow(dead_code)]
    injection_query: Option<Query>,
}

impl CachedLanguage {
    fn new(config: &LanguageConfig) -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(&config.language)?;

        let query = Query::new(&config.language, &config.highlight_query)?;
        let injection_query = config
            .injection_query
            .as_ref()
            .map(|q| Query::new(&config.language, q))
            .transpose()?;

        Ok(Self {
            parser,
            query,
            injection_query,
        })
    }
}

/// Main syntax highlighter.
///
/// Thread-safe highlighter that caches language configurations and provides
/// both one-shot and incremental highlighting.
#[derive(Debug)]
pub struct Highlighter {
    /// Theme for styling.
    theme: Arc<RwLock<Theme>>,
    /// Cached language configurations.
    configs: Arc<RwLock<AHashMap<String, LanguageConfig>>>,
    /// Language registry for name/extension lookup.
    registry: Arc<RwLock<LanguageRegistry>>,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Highlighter {
    fn clone(&self) -> Self {
        Self {
            theme: Arc::clone(&self.theme),
            configs: Arc::clone(&self.configs),
            registry: Arc::clone(&self.registry),
        }
    }
}

impl Highlighter {
    /// Creates a new highlighter with the default VS Code dark theme.
    pub fn new() -> Self {
        Self {
            theme: Arc::new(RwLock::new(Theme::vscode_dark())),
            configs: Arc::new(RwLock::new(AHashMap::new())),
            registry: Arc::new(RwLock::new(LanguageRegistry::new())),
        }
    }

    /// Creates a highlighter with a custom theme.
    pub fn with_theme(theme: Theme) -> Self {
        Self {
            theme: Arc::new(RwLock::new(theme)),
            configs: Arc::new(RwLock::new(AHashMap::new())),
            registry: Arc::new(RwLock::new(LanguageRegistry::new())),
        }
    }

    /// Sets the theme.
    pub fn set_theme(&self, theme: Theme) {
        *self.theme.write() = theme;
    }

    /// Gets a clone of the current theme.
    pub fn theme(&self) -> Theme {
        self.theme.read().clone()
    }

    /// Returns a reference to the theme for reading.
    pub fn theme_ref(&self) -> impl std::ops::Deref<Target = Theme> + '_ {
        self.theme.read()
    }

    /// Registers a language configuration.
    pub fn register_language(&self, name: impl Into<String>, config: LanguageConfig) {
        self.configs.write().insert(name.into(), config);
    }

    /// Returns true if a language is registered.
    pub fn has_language(&self, name: &str) -> bool {
        self.configs.read().contains_key(name)
    }

    /// Detects language from file extension.
    pub fn detect_language(&self, extension: &str) -> Option<String> {
        language_from_extension(extension).map(String::from)
    }

    /// Detects language from file path.
    pub fn detect_language_from_path(&self, path: impl AsRef<Path>) -> Option<String> {
        language_from_path(path).map(String::from)
    }

    /// Highlights source code.
    ///
    /// Returns highlighted text with styled spans.
    pub fn highlight(&self, source: &str, language: &str) -> Result<HighlightedText> {
        let configs = self.configs.read();
        let config = configs
            .get(language)
            .ok_or_else(|| HighlightError::LanguageNotFound(language.to_string()))?;

        // Create parser and query
        let mut cached = CachedLanguage::new(config)?;

        // Parse
        let tree = cached
            .parser
            .parse(source, None)
            .ok_or(HighlightError::ParseError)?;

        // Query for highlights
        let raw_highlights = self.query_highlights(source, &tree, &cached.query);

        // Convert to styled spans
        let theme = self.theme.read();
        let spans = self.convert_to_spans(source, &raw_highlights, &theme);

        Ok(HighlightedText::from_parts(source, spans))
    }

    /// Highlights source code and returns raw highlight data.
    ///
    /// Useful when you need the raw ranges without text extraction.
    pub fn highlight_raw(&self, source: &str, language: &str) -> Result<Vec<RawHighlight>> {
        let configs = self.configs.read();
        let config = configs
            .get(language)
            .ok_or_else(|| HighlightError::LanguageNotFound(language.to_string()))?;

        let mut cached = CachedLanguage::new(config)?;
        let tree = cached
            .parser
            .parse(source, None)
            .ok_or(HighlightError::ParseError)?;

        Ok(self.query_highlights(source, &tree, &cached.query))
    }

    /// Queries the tree for highlight captures.
    fn query_highlights(&self, source: &str, tree: &Tree, query: &Query) -> Vec<RawHighlight> {
        let mut cursor = QueryCursor::new();
        let mut highlights = Vec::new();

        let capture_names = query.capture_names();

        let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
        while let Some(match_) = matches.next() {
            for capture in match_.captures {
                let node = capture.node;
                let start = node.start_byte();
                let end = node.end_byte();

                // Skip empty or invalid ranges
                if start >= end || end > source.len() {
                    continue;
                }

                let capture_name = capture_names[capture.index as usize];
                highlights.push(RawHighlight::new(start, end, capture_name));
            }
        }

        // Sort by start position, then by specificity (longer capture names first)
        highlights.sort_by(|a, b| {
            a.start
                .cmp(&b.start)
                .then_with(|| b.capture.len().cmp(&a.capture.len()))
        });

        highlights
    }

    /// Converts raw highlights to styled spans.
    ///
    /// Handles overlapping ranges by using the most specific capture.
    fn convert_to_spans(
        &self,
        source: &str,
        highlights: &[RawHighlight],
        theme: &Theme,
    ) -> Vec<HighlightSpan> {
        if highlights.is_empty() {
            // Return entire source as plain text
            return if source.is_empty() {
                Vec::new()
            } else {
                vec![HighlightSpan::plain(source, 0..source.len())]
            };
        }

        let mut spans = Vec::new();
        let mut current_pos = 0;

        // Build a map of byte positions to their active captures
        let mut boundaries = Vec::new();
        for (idx, hl) in highlights.iter().enumerate() {
            boundaries.push((hl.start, true, idx)); // start
            boundaries.push((hl.end, false, idx)); // end
        }
        boundaries.sort_by_key(|(pos, is_start, _)| (*pos, !*is_start));

        // Track active highlights at each position
        let mut active: Vec<usize> = Vec::new();
        let mut boundary_iter = boundaries.iter().peekable();

        while current_pos < source.len() {
            // Find the next boundary position
            let next_boundary = boundary_iter
                .peek()
                .map(|(pos, _, _)| *pos)
                .unwrap_or(source.len());

            // Process all boundaries at this position
            while let Some(&(pos, is_start, idx)) = boundary_iter.peek() {
                if *pos != next_boundary {
                    break;
                }
                if *is_start {
                    active.push(*idx);
                } else {
                    active.retain(|&i| i != *idx);
                }
                boundary_iter.next();
            }

            // Emit span from current_pos to next_boundary
            if current_pos < next_boundary && next_boundary <= source.len() {
                let text = &source[current_pos..next_boundary];
                let range = current_pos..next_boundary;

                if active.is_empty() {
                    // No highlighting - use default style
                    spans.push(HighlightSpan::plain(text, range));
                } else {
                    // Use the most specific (longest capture name) active highlight
                    let best = active
                        .iter()
                        .max_by_key(|&&idx| highlights[idx].capture.len())
                        .copied()
                        .unwrap();

                    let capture = &highlights[best].capture;
                    let style = theme.get(capture);
                    spans.push(HighlightSpan::with_capture(text, style, range, capture));
                }

                current_pos = next_boundary;
            } else {
                break;
            }
        }

        // Handle any remaining text
        if current_pos < source.len() {
            spans.push(HighlightSpan::plain(
                &source[current_pos..],
                current_pos..source.len(),
            ));
        }

        spans
    }

    /// Returns the language registry.
    pub fn registry(&self) -> impl std::ops::Deref<Target = LanguageRegistry> + '_ {
        self.registry.read()
    }

    /// Returns mutable access to the language registry.
    pub fn registry_mut(&self) -> impl std::ops::DerefMut<Target = LanguageRegistry> + '_ {
        self.registry.write()
    }
}

/// Incremental highlighter for streaming content.
///
/// Maintains parse tree state for efficient incremental updates.
pub struct IncrementalHighlighter {
    /// The base highlighter.
    highlighter: Highlighter,
    /// Current source content.
    source: String,
    /// Current parse tree.
    tree: Option<Tree>,
    /// Cached parser.
    parser: Option<Parser>,
    /// Cached query.
    query: Option<Query>,
    /// Language name.
    language: String,
    /// Last highlight result (for streaming).
    last_highlights: Vec<RawHighlight>,
}

impl IncrementalHighlighter {
    /// Creates a new incremental highlighter.
    pub fn new(highlighter: Highlighter) -> Self {
        Self {
            highlighter,
            source: String::new(),
            tree: None,
            parser: None,
            query: None,
            language: String::new(),
            last_highlights: Vec::new(),
        }
    }

    /// Sets the language for highlighting.
    pub fn set_language(&mut self, language: &str) -> Result<()> {
        if self.language == language && self.parser.is_some() {
            return Ok(());
        }

        let configs = self.highlighter.configs.read();
        let config = configs
            .get(language)
            .ok_or_else(|| HighlightError::LanguageNotFound(language.to_string()))?;

        let mut parser = Parser::new();
        parser.set_language(&config.language)?;

        let query = Query::new(&config.language, &config.highlight_query)?;

        self.parser = Some(parser);
        self.query = Some(query);
        self.language = language.to_string();
        self.tree = None;
        self.last_highlights.clear();

        Ok(())
    }

    /// Sets the source content and parses it.
    pub fn set_source(&mut self, source: impl Into<String>) -> Result<()> {
        self.source = source.into();
        self.reparse()?;
        Ok(())
    }

    /// Appends content for streaming.
    pub fn append(&mut self, content: &str) -> Result<()> {
        let old_len = self.source.len();
        self.source.push_str(content);

        // For now, just reparse entirely
        // A more sophisticated implementation would use tree.edit()
        self.reparse()?;

        // In a full implementation, we would:
        // 1. Create an InputEdit for the appended content
        // 2. Call tree.edit(&edit)
        // 3. Reparse incrementally
        let _ = old_len;

        Ok(())
    }

    /// Reparses the current source.
    fn reparse(&mut self) -> Result<()> {
        let parser = self
            .parser
            .as_mut()
            .ok_or_else(|| HighlightError::LanguageNotFound(self.language.clone()))?;

        // Incremental parse if we have an existing tree
        self.tree = parser.parse(&self.source, self.tree.as_ref());

        if self.tree.is_none() {
            return Err(HighlightError::ParseError);
        }

        // Update highlights
        if let (Some(tree), Some(query)) = (&self.tree, &self.query) {
            self.last_highlights = self.query_highlights(tree, query);
        }

        Ok(())
    }

    /// Queries for highlights.
    fn query_highlights(&self, tree: &Tree, query: &Query) -> Vec<RawHighlight> {
        let mut cursor = QueryCursor::new();
        let mut highlights = Vec::new();

        let capture_names = query.capture_names();

        let mut matches = cursor.matches(query, tree.root_node(), self.source.as_bytes());
        while let Some(match_) = matches.next() {
            for capture in match_.captures {
                let node = capture.node;
                let start = node.start_byte();
                let end = node.end_byte();

                if start >= end || end > self.source.len() {
                    continue;
                }

                let capture_name = capture_names[capture.index as usize];
                highlights.push(RawHighlight::new(start, end, capture_name));
            }
        }

        highlights.sort_by_key(|h| (h.start, std::cmp::Reverse(h.capture.len())));
        highlights
    }

    /// Gets the current highlighted text.
    pub fn highlighted_text(&self) -> HighlightedText {
        let theme = self.highlighter.theme.read();
        let spans = self
            .highlighter
            .convert_to_spans(&self.source, &self.last_highlights, &theme);
        HighlightedText::from_parts(&self.source, spans)
    }

    /// Gets highlighted text using cached highlights.
    ///
    /// Useful during streaming when you want immediate feedback
    /// before the full reparse completes.
    pub fn highlighted_text_cached(&self) -> HighlightedText {
        let theme = self.highlighter.theme.read();
        let spans = self
            .highlighter
            .convert_to_spans(&self.source, &self.last_highlights, &theme);
        HighlightedText::from_parts(&self.source, spans)
    }

    /// Returns the current source.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the last highlights.
    pub fn last_highlights(&self) -> &[RawHighlight] {
        &self.last_highlights
    }

    /// Clears all state.
    pub fn clear(&mut self) {
        self.source.clear();
        self.tree = None;
        self.last_highlights.clear();
    }
}

/// A lightweight highlighter for one-off highlighting without caching.
pub struct SimpleHighlighter<'a> {
    parser: Parser,
    query: &'a Query,
    theme: &'a Theme,
}

impl<'a> SimpleHighlighter<'a> {
    /// Creates a new simple highlighter.
    pub fn new(language: &Language, query: &'a Query, theme: &'a Theme) -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(language)?;
        Ok(Self {
            parser,
            query,
            theme,
        })
    }

    /// Highlights source code.
    pub fn highlight(&mut self, source: &str) -> Result<HighlightedText> {
        let tree = self
            .parser
            .parse(source, None)
            .ok_or(HighlightError::ParseError)?;

        let mut cursor = QueryCursor::new();
        let mut highlights = Vec::new();

        let capture_names = self.query.capture_names();

        let mut matches = cursor.matches(self.query, tree.root_node(), source.as_bytes());
        while let Some(match_) = matches.next() {
            for capture in match_.captures {
                let node = capture.node;
                let start = node.start_byte();
                let end = node.end_byte();

                if start >= end || end > source.len() {
                    continue;
                }

                let capture_name = capture_names[capture.index as usize];
                highlights.push(RawHighlight::new(start, end, capture_name));
            }
        }

        highlights.sort_by_key(|h| (h.start, std::cmp::Reverse(h.capture.len())));

        let spans = self.convert_to_spans(source, &highlights);
        Ok(HighlightedText::from_parts(source, spans))
    }

    /// Converts raw highlights to spans.
    fn convert_to_spans(&self, source: &str, highlights: &[RawHighlight]) -> Vec<HighlightSpan> {
        if highlights.is_empty() {
            return if source.is_empty() {
                Vec::new()
            } else {
                vec![HighlightSpan::plain(source, 0..source.len())]
            };
        }

        let mut spans = Vec::new();
        let mut pos = 0;

        for hl in highlights {
            // Add unhighlighted text before this highlight
            if pos < hl.start {
                spans.push(HighlightSpan::plain(&source[pos..hl.start], pos..hl.start));
            }

            // Add highlighted span
            if hl.start < hl.end && hl.end <= source.len() {
                let text = &source[hl.start..hl.end];
                let style = self.theme.get(&hl.capture);
                spans.push(HighlightSpan::with_capture(
                    text,
                    style,
                    hl.start..hl.end,
                    &hl.capture,
                ));
                pos = hl.end;
            }
        }

        // Add remaining text
        if pos < source.len() {
            spans.push(HighlightSpan::plain(&source[pos..], pos..source.len()));
        }

        spans
    }
}

/// Applies a style to a byte range in source text.
///
/// Utility function for manual highlighting.
pub fn style_range(source: &str, range: ByteRange, style: Style) -> Option<HighlightSpan> {
    if range.start >= range.end || range.end > source.len() {
        return None;
    }
    Some(HighlightSpan::new(&source[range.clone()], style, range))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlighter_creation() {
        let highlighter = Highlighter::new();
        assert!(!highlighter.has_language("rust"));
    }

    #[test]
    fn test_language_detection() {
        let highlighter = Highlighter::new();
        assert_eq!(highlighter.detect_language("rs"), Some("rust".to_string()));
        assert_eq!(
            highlighter.detect_language("py"),
            Some("python".to_string())
        );
        assert_eq!(highlighter.detect_language("xyz"), None);
    }

    #[test]
    fn test_style_range() {
        let source = "hello world";
        let span = style_range(source, 0..5, Style::default()).unwrap();
        assert_eq!(span.text, "hello");
        assert_eq!(span.range, 0..5);
    }

    #[test]
    fn test_style_range_invalid() {
        let source = "hello";
        assert!(style_range(source, 5..3, Style::default()).is_none());
        assert!(style_range(source, 0..10, Style::default()).is_none());
    }
}
