//! Event handlers for RenderState - handles start/end tags and content events.

use pulldown_cmark::{CodeBlockKind, HeadingLevel};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::markdown::inline::{render_blockquote_prefix, render_hr};
use crate::markdown::list::ListContext;
use crate::markdown::table::{TableBuilder, render_table_simple};

use super::helpers::{get_bullet, heading_level_to_u8};
use super::state::RenderState;

// ============================================================
// Start tag handlers
// ============================================================

impl<'a> RenderState<'a> {
    pub(super) fn start_paragraph(&mut self) {
        self.add_blank_line_if_needed();
        self.in_paragraph = true;
        self.current_line_width = 0;
    }

    pub(super) fn start_heading(&mut self, level: HeadingLevel) {
        self.add_blank_line_if_needed();
        self.current_heading_level = Some(level);
    }

    pub(super) fn start_code_block(&mut self, kind: CodeBlockKind<'_>) {
        self.add_blank_line_if_needed();
        self.in_code_block = true;
        self.code_buffer.clear();
        self.code_language = match kind {
            CodeBlockKind::Fenced(lang) => {
                let lang = lang.to_string();
                if lang.is_empty() { None } else { Some(lang) }
            }
            CodeBlockKind::Indented => None,
        };
    }

    pub(super) fn start_list(&mut self, start: Option<u64>) {
        // Add blank line before top-level lists
        if self.list_stack.is_empty() {
            self.add_blank_line_if_needed();
        }

        let ctx = match start {
            Some(n) => ListContext::new_ordered(n),
            None => ListContext::new_unordered(),
        };
        self.list_stack.push(ctx);
    }

    pub(super) fn start_item(&mut self) {
        self.current_list_item.clear();
        self.current_task_marker = None;
    }

    pub(super) fn start_blockquote(&mut self) {
        if self.blockquote_depth == 0 {
            self.add_blank_line_if_needed();
        }
        self.blockquote_depth += 1;
    }

    pub(super) fn start_emphasis(&mut self) {
        self.style_stack.push(self.renderer.theme.italic);
    }

    pub(super) fn start_strong(&mut self) {
        self.style_stack.push(self.renderer.theme.bold);
    }

    pub(super) fn start_strikethrough(&mut self) {
        self.style_stack.push(self.renderer.theme.strikethrough);
    }

    pub(super) fn start_link(&mut self, url: &str) {
        self.current_link_url = Some(url.to_string());
        self.style_stack.push(self.renderer.theme.link_text);
    }

    pub(super) fn start_table(&mut self) {
        self.add_blank_line_if_needed();
        self.table_builder = Some(TableBuilder::new());
    }

    pub(super) fn start_table_head(&mut self) {
        if let Some(ref mut builder) = self.table_builder {
            builder.start_header();
        }
        self.in_table_header = true;
    }

    pub(super) fn start_table_row(&mut self) {
        if let Some(ref mut builder) = self.table_builder {
            if !self.in_table_header {
                builder.start_row();
            }
        }
    }

    pub(super) fn start_table_cell(&mut self) {
        self.current_cell.clear();
    }
}

// ============================================================
// End tag handlers
// ============================================================

impl<'a> RenderState<'a> {
    pub(super) fn end_paragraph(&mut self) {
        self.flush_line();
        self.in_paragraph = false;
        self.needs_newline = true;
        self.current_line_width = 0;
    }

    pub(super) fn end_heading(&mut self) {
        if let Some(level) = self.current_heading_level.take() {
            let style = self.renderer.theme.header_style(heading_level_to_u8(level));

            // Apply heading style to all spans
            let spans: Vec<Span<'static>> = self
                .current_spans
                .drain(..)
                .map(|mut s| {
                    s.style = style.patch(s.style);
                    s
                })
                .collect();

            // Add blockquote prefix if in blockquote
            let mut final_spans = self.get_blockquote_prefix();
            final_spans.extend(spans);

            self.lines.push(Line::from(final_spans));
            self.needs_newline = true;
        }
    }

    pub(super) fn end_code_block(&mut self) {
        self.in_code_block = false;

        // Render the code block
        let code_lines = if let Some(ref code_renderer) = self.renderer.code_renderer {
            code_renderer.render(
                &self.code_buffer,
                self.code_language.as_deref(),
                self.renderer.width,
            )
        } else {
            // Simple rendering without syntax highlighting
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
        self.needs_newline = true;
    }

    pub(super) fn end_list(&mut self) {
        self.list_stack.pop();
        if self.list_stack.is_empty() {
            self.needs_newline = true;
        }
    }

    pub(super) fn end_item(&mut self) {
        if let Some(ctx) = self.list_stack.last_mut() {
            let theme = &self.renderer.theme;

            // Build the marker
            let (marker, marker_style) = if let Some(checked) = self.current_task_marker {
                let marker = if checked { "[x] " } else { "[ ] " };
                let style = if checked {
                    theme.task_checked
                } else {
                    theme.task_unchecked
                };
                (marker.to_string(), style)
            } else if ctx.ordered {
                let num = ctx.next_number();
                (format!("{}. ", num), theme.list_number)
            } else {
                let bullet = get_bullet(ctx.depth);
                (format!("{} ", bullet), theme.list_bullet)
            };

            // Build the line
            let mut spans = Vec::new();

            // Add blockquote prefix if in blockquote
            if self.blockquote_depth > 0 {
                spans.extend(render_blockquote_prefix(
                    self.blockquote_depth,
                    self.renderer.theme.blockquote_border,
                ));
            }

            // Add list indent
            let indent = ctx.indent();
            if !indent.is_empty() {
                spans.push(Span::raw(indent));
            }

            // Add marker
            spans.push(Span::styled(marker, marker_style));

            // Add content
            spans.extend(self.current_list_item.drain(..));

            self.lines.push(Line::from(spans));
        }
        self.current_task_marker = None;
    }

    pub(super) fn end_blockquote(&mut self) {
        self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
        if self.blockquote_depth == 0 {
            self.needs_newline = true;
        }
    }

    pub(super) fn end_emphasis(&mut self) {
        self.style_stack.pop();
    }

    pub(super) fn end_strong(&mut self) {
        self.style_stack.pop();
    }

    pub(super) fn end_strikethrough(&mut self) {
        self.style_stack.pop();
    }

    pub(super) fn end_link(&mut self) {
        self.style_stack.pop();

        // Optionally show URL after link text
        if let Some(url) = self.current_link_url.take() {
            // Only show URL if it's different from the link text
            let link_text: String = self.current_spans.iter().map(|s| &*s.content).collect();
            if url != link_text && !url.is_empty() {
                self.push_span(Span::styled(
                    format!(" ({})", url),
                    self.renderer.theme.link_url,
                ));
            }
        }
    }

    pub(super) fn end_table(&mut self) {
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

            // Add blockquote prefix if needed
            if self.blockquote_depth > 0 {
                for line in table_lines {
                    let mut prefixed_spans = self.get_blockquote_prefix();
                    prefixed_spans.extend(line.spans);
                    self.lines.push(Line::from(prefixed_spans));
                }
            } else {
                self.lines.extend(table_lines);
            }

            self.needs_newline = true;
        }
    }

    pub(super) fn end_table_head(&mut self) {
        if let Some(ref mut builder) = self.table_builder {
            builder.end_header();
        }
        self.in_table_header = false;
    }

    pub(super) fn end_table_row(&mut self) {
        if let Some(ref mut builder) = self.table_builder {
            if !self.in_table_header {
                builder.end_row();
            }
        }
    }

    pub(super) fn end_table_cell(&mut self) {
        if let Some(ref mut builder) = self.table_builder {
            builder.add_cell(std::mem::take(&mut self.current_cell));
        }
    }
}

// ============================================================
// Content handlers
// ============================================================

impl<'a> RenderState<'a> {
    pub(super) fn handle_text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_buffer.push_str(text);
            return;
        }

        if self.table_builder.is_some() {
            self.current_cell.push_str(text);
            return;
        }

        // In a list item?
        if !self.list_stack.is_empty() {
            let style = self.current_style();
            self.current_list_item
                .push(Span::styled(text.to_string(), style));
            return;
        }

        // Paragraph text - apply word wrapping
        if self.in_paragraph {
            self.wrap_paragraph_text(text);
            return;
        }

        // Regular text (headings, etc.) - no wrapping needed
        let style = self.current_style();
        self.push_span(Span::styled(text.to_string(), style));
    }

    /// Wraps paragraph text to fit within the renderer's width.
    ///
    /// Handles word boundaries, preserves inline styles across line breaks,
    /// and properly measures unicode character widths (CJK characters, emoji).
    fn wrap_paragraph_text(&mut self, text: &str) {
        let style = self.current_style();
        let max_width = self.renderer.width as usize;

        // Account for blockquote prefix width
        let prefix_width = if self.blockquote_depth > 0 {
            // Each blockquote level adds "│ " (2 chars)
            self.blockquote_depth * 2
        } else {
            0
        };

        let available_width = max_width.saturating_sub(prefix_width);
        if available_width == 0 {
            // No room for text
            return;
        }

        // Process text word by word
        for word in text.split_inclusive(|c: char| c.is_whitespace()) {
            let word_width = UnicodeWidthStr::width(word);

            // Check if word fits on current line
            if self.current_line_width + word_width <= available_width {
                // Word fits - add it
                self.push_span(Span::styled(word.to_string(), style));
                self.current_line_width += word_width;
            } else if self.current_line_width == 0 {
                // First word on line but too long - break it
                self.wrap_long_word(word, style, available_width);
            } else {
                // Word doesn't fit - start new line
                self.flush_line();
                self.current_line_width = 0;

                // Trim leading whitespace from word when starting new line
                let trimmed = word.trim_start();
                let trimmed_width = UnicodeWidthStr::width(trimmed);

                if trimmed_width <= available_width {
                    self.push_span(Span::styled(trimmed.to_string(), style));
                    self.current_line_width = trimmed_width;
                } else {
                    // Even trimmed word is too long - break it
                    self.wrap_long_word(trimmed, style, available_width);
                }
            }
        }
    }

    /// Wraps a word that is too long to fit on a single line.
    ///
    /// Breaks the word at visual width boundaries, handling multi-width
    /// unicode characters properly.
    fn wrap_long_word(&mut self, word: &str, style: Style, max_width: usize) {
        let mut current_chunk = String::new();
        let mut current_width = 0;

        for ch in word.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);

            if current_width + ch_width > max_width && !current_chunk.is_empty() {
                // Flush current chunk as a line
                self.push_span(Span::styled(current_chunk.clone(), style));
                self.flush_line();
                current_chunk.clear();
                current_width = 0;
            }

            current_chunk.push(ch);
            current_width += ch_width;
        }

        // Handle remaining characters
        if !current_chunk.is_empty() {
            self.push_span(Span::styled(current_chunk, style));
            self.current_line_width = current_width;
        }
    }

    pub(super) fn handle_code(&mut self, code: &str) {
        if self.table_builder.is_some() {
            self.current_cell.push_str(code);
            return;
        }

        if !self.list_stack.is_empty() {
            self.current_list_item.push(Span::styled(
                code.to_string(),
                self.renderer.theme.code_inline,
            ));
            return;
        }

        self.push_span(Span::styled(
            code.to_string(),
            self.renderer.theme.code_inline,
        ));
    }

    pub(super) fn handle_soft_break(&mut self) {
        if self.in_code_block {
            self.code_buffer.push('\n');
            return;
        }

        if !self.list_stack.is_empty() {
            self.current_list_item.push(Span::raw(" "));
            return;
        }

        // Treat soft break as space
        self.push_span(Span::raw(" "));
    }

    pub(super) fn handle_hard_break(&mut self) {
        if self.in_code_block {
            self.code_buffer.push('\n');
            return;
        }

        self.flush_line();
    }

    pub(super) fn handle_rule(&mut self) {
        self.add_blank_line_if_needed();

        let hr_line = render_hr(self.renderer.width, self.renderer.theme.hr);

        // Add blockquote prefix if needed
        if self.blockquote_depth > 0 {
            let mut prefixed_spans = self.get_blockquote_prefix();
            prefixed_spans.extend(hr_line.spans);
            self.lines.push(Line::from(prefixed_spans));
        } else {
            self.lines.push(hr_line);
        }

        self.needs_newline = true;
    }

    pub(super) fn handle_task_list_marker(&mut self, checked: bool) {
        self.current_task_marker = Some(checked);
    }
}

// ============================================================
// Helper methods
// ============================================================

impl<'a> RenderState<'a> {
    /// Flush the current spans as a line.
    pub(super) fn flush_line(&mut self) {
        if self.current_spans.is_empty() {
            return;
        }

        let mut spans = self.get_blockquote_prefix();
        spans.extend(self.current_spans.drain(..));
        self.lines.push(Line::from(spans));
        self.current_line_width = 0;
    }

    /// Push a span to the current line.
    pub(super) fn push_span(&mut self, span: Span<'static>) {
        self.current_spans.push(span);
    }

    /// Get the current combined style from the style stack.
    pub(super) fn current_style(&self) -> Style {
        let base = if self.blockquote_depth > 0 {
            self.renderer.theme.blockquote_text
        } else {
            self.renderer.theme.text
        };
        base.patch(self.style_stack.current())
    }

    /// Add a blank line if needed for spacing.
    pub(super) fn add_blank_line_if_needed(&mut self) {
        if self.needs_newline && !self.lines.is_empty() {
            // Add blockquote prefix to blank line if in blockquote
            if self.blockquote_depth > 0 {
                let prefix = self.get_blockquote_prefix();
                self.lines.push(Line::from(prefix));
            } else {
                self.lines.push(Line::default());
            }
        }
        self.needs_newline = false;
    }

    /// Get blockquote prefix spans.
    pub(super) fn get_blockquote_prefix(&self) -> Vec<Span<'static>> {
        if self.blockquote_depth > 0 {
            render_blockquote_prefix(self.blockquote_depth, self.renderer.theme.blockquote_border)
        } else {
            Vec::new()
        }
    }

    /// Render a simple code block without syntax highlighting.
    pub(super) fn render_simple_code_block(&self) -> Vec<Line<'static>> {
        let style = self.renderer.theme.code_block_text;
        let border_style = Style::default().fg(self.renderer.theme.code_block_border);

        let mut lines = Vec::new();

        // Top border with optional language tag
        let top_border = if let Some(ref lang) = self.code_language {
            format!(
                "{} {} {}",
                "```",
                lang,
                "─".repeat(self.renderer.width.saturating_sub(lang.width() as u16 + 5) as usize)
            )
        } else {
            format!(
                "```{}",
                "─".repeat(self.renderer.width.saturating_sub(3) as usize)
            )
        };
        lines.push(Line::from(Span::styled(top_border, border_style)));

        // Code content
        for code_line in self.code_buffer.lines() {
            lines.push(Line::from(Span::styled(code_line.to_string(), style)));
        }

        // Handle trailing newline - no action needed

        // Bottom border
        lines.push(Line::from(Span::styled(
            "─".repeat(self.renderer.width as usize),
            border_style,
        )));

        lines
    }
}
