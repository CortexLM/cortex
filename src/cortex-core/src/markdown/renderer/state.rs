//! RenderState - Internal state for event-driven markdown parsing.

use pulldown_cmark::{Event, HeadingLevel, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::markdown::list::ListContext;
use crate::markdown::table::{TableBuilder, render_table_simple};

use super::MarkdownRenderer;

// ============================================================
// StyleStack
// ============================================================

/// Simple style stack for tracking nested inline styles.
#[derive(Debug, Clone, Default)]
pub(super) struct StyleStack {
    styles: Vec<Style>,
}

impl StyleStack {
    pub fn new() -> Self {
        Self { styles: Vec::new() }
    }

    pub fn push(&mut self, style: Style) {
        self.styles.push(style);
    }

    pub fn pop(&mut self) -> Option<Style> {
        self.styles.pop()
    }

    /// Get the current combined style from all stacked styles.
    pub fn current(&self) -> Style {
        self.styles
            .iter()
            .fold(Style::default(), |acc, s| acc.patch(*s))
    }
}

// ============================================================
// RenderState
// ============================================================

/// Internal state for the rendering process.
///
/// Tracks the current position in the document, accumulated spans,
/// and block-level context (lists, tables, code blocks, etc.).
pub(super) struct RenderState<'a> {
    /// Reference to the renderer (for theme and code renderer).
    pub(super) renderer: &'a MarkdownRenderer,
    /// Accumulated output lines.
    pub(super) lines: Vec<Line<'static>>,
    /// Current line's spans (accumulated until line is flushed).
    pub(super) current_spans: Vec<Span<'static>>,

    // Style tracking
    /// Stack of inline styles (bold, italic, etc.).
    pub(super) style_stack: StyleStack,

    // Block tracking
    /// Stack of list contexts for nested lists.
    pub(super) list_stack: Vec<ListContext>,
    /// Current blockquote nesting depth.
    pub(super) blockquote_depth: usize,

    // Code block state
    /// Whether we're inside a code block.
    pub(super) in_code_block: bool,
    /// Language of the current code block (if any).
    pub(super) code_language: Option<String>,
    /// Buffer for code block content.
    pub(super) code_buffer: String,

    // Table state
    /// Table builder for current table (if any).
    pub(super) table_builder: Option<TableBuilder>,
    /// Whether we're in the table header.
    pub(super) in_table_header: bool,
    /// Current cell content buffer.
    pub(super) current_cell: String,

    // Paragraph state
    /// Whether we're inside a paragraph.
    pub(super) in_paragraph: bool,
    /// Whether we need a blank line before the next block.
    pub(super) needs_newline: bool,

    // Heading state
    /// Current heading level (if in a heading).
    pub(super) current_heading_level: Option<HeadingLevel>,

    // Link state
    /// Current link URL (if in a link).
    pub(super) current_link_url: Option<String>,

    // List item state
    /// Current list item content.
    pub(super) current_list_item: Vec<Span<'static>>,
    /// Task marker for current item.
    pub(super) current_task_marker: Option<bool>,
}

impl<'a> RenderState<'a> {
    /// Creates a new render state.
    pub fn new(renderer: &'a MarkdownRenderer) -> Self {
        Self {
            renderer,
            lines: Vec::new(),
            current_spans: Vec::new(),
            style_stack: StyleStack::new(),
            list_stack: Vec::new(),
            blockquote_depth: 0,
            in_code_block: false,
            code_language: None,
            code_buffer: String::new(),
            table_builder: None,
            in_table_header: false,
            current_cell: String::new(),
            in_paragraph: false,
            needs_newline: false,
            current_heading_level: None,
            current_link_url: None,
            current_list_item: Vec::new(),
            current_task_marker: None,
        }
    }

    /// Handle a markdown event.
    pub fn handle_event(&mut self, event: Event<'_>) {
        match event {
            // Start tags
            Event::Start(tag) => self.handle_start_tag(tag),
            // End tags
            Event::End(tag) => self.handle_end_tag(tag),
            // Content events
            Event::Text(text) => self.handle_text(&text),
            Event::Code(code) => self.handle_code(&code),
            Event::SoftBreak => self.handle_soft_break(),
            Event::HardBreak => self.handle_hard_break(),
            Event::Rule => self.handle_rule(),
            Event::TaskListMarker(checked) => self.handle_task_list_marker(checked),
            // Handle math events (treat as inline code)
            Event::InlineMath(math) => self.handle_code(&math),
            Event::DisplayMath(math) => self.handle_code(&math),
            // Ignore HTML and footnotes for now
            Event::Html(_) => {}
            Event::InlineHtml(_) => {}
            Event::FootnoteReference(_) => {}
        }
    }

    /// Handle a start tag.
    fn handle_start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::CodeBlock(kind) => self.start_code_block(kind),
            Tag::List(start) => self.start_list(start),
            Tag::Item => self.start_item(),
            Tag::BlockQuote(_) => self.start_blockquote(),
            Tag::Emphasis => self.start_emphasis(),
            Tag::Strong => self.start_strong(),
            Tag::Strikethrough => self.start_strikethrough(),
            Tag::Link { dest_url, .. } => self.start_link(&dest_url),
            Tag::Table(_alignments) => self.start_table(),
            Tag::TableHead => self.start_table_head(),
            Tag::TableRow => self.start_table_row(),
            Tag::TableCell => self.start_table_cell(),
            Tag::Image { .. } => {} // Images not supported in TUI
            Tag::FootnoteDefinition(_) => {}
            Tag::HtmlBlock => {}
            Tag::MetadataBlock(_) => {}
            // New pulldown-cmark 0.13+ variants - ignore for now
            _ => {}
        }
    }

    /// Handle an end tag.
    fn handle_end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.end_paragraph(),
            TagEnd::Heading(_) => self.end_heading(),
            TagEnd::CodeBlock => self.end_code_block(),
            TagEnd::List(_) => self.end_list(),
            TagEnd::Item => self.end_item(),
            TagEnd::BlockQuote(_) => self.end_blockquote(),
            TagEnd::Emphasis => self.end_emphasis(),
            TagEnd::Strong => self.end_strong(),
            TagEnd::Strikethrough => self.end_strikethrough(),
            TagEnd::Link => self.end_link(),
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => self.end_table_head(),
            TagEnd::TableRow => self.end_table_row(),
            TagEnd::TableCell => self.end_table_cell(),
            TagEnd::Image => {}
            TagEnd::FootnoteDefinition => {}
            TagEnd::HtmlBlock => {}
            TagEnd::MetadataBlock(_) => {}
            // New pulldown-cmark 0.13+ variants - ignore for now
            _ => {}
        }
    }

    /// Finish rendering and return the accumulated lines.
    pub fn finish(mut self) -> Vec<Line<'static>> {
        // Handle unclosed code blocks gracefully (auto-close them)
        if self.in_code_block {
            // Auto-close the code block to prevent rendering issues
            self.in_code_block = false;

            // Render the code block even if unclosed
            let code_lines = if let Some(ref code_renderer) = self.renderer.code_renderer {
                code_renderer.render(
                    &self.code_buffer,
                    self.code_language.as_deref(),
                    self.renderer.width,
                )
            } else {
                self.render_simple_code_block()
            };

            // Add blockquote prefix to each line if needed
            if self.blockquote_depth > 0 {
                for line in code_lines {
                    let mut prefixed_spans = self.get_blockquote_prefix();
                    prefixed_spans.extend(line.spans);
                    self.lines.push(Line::from(prefixed_spans));
                }
            } else {
                self.lines.extend(code_lines);
            }

            self.code_buffer.clear();
            self.code_language = None;
        }

        // Close any unclosed tables
        if let Some(builder) = self.table_builder.take() {
            let mut table = builder.build();
            table.calculate_column_widths(self.renderer.width);

            // Use simple ASCII table format without outer borders
            // Headers use table_header_text style for colored/bold headers
            let table_lines = render_table_simple(
                &table,
                self.renderer.theme.table_header_text,
                self.renderer.theme.table_cell_text,
                self.renderer.width,
            );

            self.lines.extend(table_lines);
        }

        // Flush any remaining content
        self.flush_line();
        self.lines
    }
}
