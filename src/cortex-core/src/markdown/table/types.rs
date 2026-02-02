//! Core types for table rendering.
//!
//! Contains the `Alignment`, `TableCell`, and `Table` types.

use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use super::utils::longest_word_width;

/// Minimum column width (excluding borders)
pub(crate) const MIN_COLUMN_WIDTH: usize = 3;

/// Padding on each side of cell content
pub(crate) const CELL_PADDING: usize = 1;

// ============================================================
// ALIGNMENT ENUM
// ============================================================

/// Alignment for table columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    /// Left-align content (default)
    #[default]
    Left,
    /// Center content
    Center,
    /// Right-align content
    Right,
}

// ============================================================
// TABLE CELL
// ============================================================

/// A single table cell containing text content and optional styled spans.
#[derive(Debug, Clone)]
pub struct TableCell {
    /// Raw text content of the cell
    pub content: String,
    /// Styled spans for rendering (if different from plain content)
    pub spans: Vec<Span<'static>>,
}

impl TableCell {
    /// Creates a new table cell with the given content.
    ///
    /// The spans will be set to a single unstyled span containing the content.
    pub fn new(content: impl Into<String>) -> Self {
        let content = content.into();
        let spans = vec![Span::raw(content.clone())];
        Self { content, spans }
    }

    /// Creates a new table cell with pre-styled spans.
    ///
    /// # Arguments
    /// * `content` - The raw text content (used for width calculation)
    /// * `spans` - Pre-styled spans for rendering
    pub fn with_spans(content: String, spans: Vec<Span<'static>>) -> Self {
        Self { content, spans }
    }

    /// Returns the display width of the cell content.
    ///
    /// Uses unicode-width for proper handling of:
    /// - Multi-byte UTF-8 characters
    /// - Wide characters (CJK, emoji)
    /// - Zero-width characters
    #[inline]
    pub fn width(&self) -> usize {
        UnicodeWidthStr::width(self.content.as_str())
    }
}

impl Default for TableCell {
    fn default() -> Self {
        Self::new("")
    }
}

// ============================================================
// TABLE
// ============================================================

/// A complete table with headers, rows, alignments, and calculated widths.
#[derive(Debug, Clone)]
pub struct Table {
    /// Header cells
    pub headers: Vec<TableCell>,
    /// Data rows (each row is a vector of cells)
    pub rows: Vec<Vec<TableCell>>,
    /// Column alignments
    pub alignments: Vec<Alignment>,
    /// Calculated column widths (content width, excluding borders/padding)
    pub column_widths: Vec<usize>,
}

impl Table {
    /// Creates a new table with the given headers, rows, and alignments.
    ///
    /// Column widths are initialized to zero and should be calculated
    /// using `calculate_column_widths` before rendering.
    pub fn new(
        headers: Vec<TableCell>,
        rows: Vec<Vec<TableCell>>,
        alignments: Vec<Alignment>,
    ) -> Self {
        let num_cols = headers
            .len()
            .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));

        // Extend alignments to match column count
        let mut alignments = alignments;
        alignments.resize(num_cols, Alignment::default());

        Self {
            headers,
            rows,
            alignments,
            column_widths: vec![0; num_cols],
        }
    }

    /// Returns true if the table has no content.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.rows.is_empty()
    }

    /// Returns the number of columns in the table.
    #[inline]
    pub fn num_columns(&self) -> usize {
        self.column_widths.len()
    }

    /// Calculates optimal column widths based on content and max_width.
    ///
    /// The algorithm:
    /// 1. Calculate minimum width per column (longest word or MIN_COLUMN_WIDTH)
    /// 2. Calculate preferred width (full content width)
    /// 3. Distribute available space proportionally
    /// 4. Respect max_width parameter
    ///
    /// # Arguments
    /// * `max_width` - Maximum total table width including borders
    pub fn calculate_column_widths(&mut self, max_width: u16) {
        let num_cols = self.num_columns();
        if num_cols == 0 {
            return;
        }

        // Calculate minimum and preferred widths for each column
        let mut min_widths = vec![MIN_COLUMN_WIDTH; num_cols];
        let mut pref_widths = vec![MIN_COLUMN_WIDTH; num_cols];

        // Process headers
        for (i, cell) in self.headers.iter().enumerate() {
            if i < num_cols {
                let cell_width = cell.width();
                let min_word = longest_word_width(&cell.content);
                min_widths[i] = min_widths[i].max(min_word).max(MIN_COLUMN_WIDTH);
                pref_widths[i] = pref_widths[i].max(cell_width);
            }
        }

        // Process data rows
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < num_cols {
                    let cell_width = cell.width();
                    let min_word = longest_word_width(&cell.content);
                    min_widths[i] = min_widths[i].max(min_word).max(MIN_COLUMN_WIDTH);
                    pref_widths[i] = pref_widths[i].max(cell_width);
                }
            }
        }

        // Calculate available width for content
        // Border chars: │ at start, │ between each column, │ at end = num_cols + 1
        // Padding: CELL_PADDING on each side of content = 2 * CELL_PADDING * num_cols
        let border_overhead = num_cols + 1;
        let padding_overhead = 2 * CELL_PADDING * num_cols;
        let total_overhead = border_overhead + padding_overhead;

        let available_width = if (max_width as usize) > total_overhead {
            (max_width as usize) - total_overhead
        } else {
            // Not enough space, use minimum possible
            num_cols * MIN_COLUMN_WIDTH
        };

        // Calculate total minimum and preferred widths
        let total_min: usize = min_widths.iter().sum();
        let total_pref: usize = pref_widths.iter().sum();

        if total_pref <= available_width {
            // All preferred widths fit
            self.column_widths = pref_widths;
        } else if total_min <= available_width {
            // Distribute extra space proportionally
            let extra_space = available_width - total_min;
            let pref_extra: usize = pref_widths
                .iter()
                .zip(min_widths.iter())
                .map(|(p, m)| p.saturating_sub(*m))
                .sum();

            self.column_widths = min_widths.clone();

            if pref_extra > 0 {
                for i in 0..num_cols {
                    let col_extra = pref_widths[i].saturating_sub(min_widths[i]);
                    let allocated =
                        (col_extra as f64 / pref_extra as f64 * extra_space as f64) as usize;
                    self.column_widths[i] += allocated;
                }
            }
        } else {
            // Not enough space even for minimums, use minimums anyway
            self.column_widths = min_widths;
        }
    }
}

impl Default for Table {
    fn default() -> Self {
        Self {
            headers: Vec::new(),
            rows: Vec::new(),
            alignments: Vec::new(),
            column_widths: Vec::new(),
        }
    }
}
