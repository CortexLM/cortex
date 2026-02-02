//! Table builder for incremental table construction.
//!
//! Useful when parsing markdown tables where content arrives piece by piece.

use ratatui::text::Span;

use super::types::{Alignment, Table, TableCell};

/// Builder for constructing tables incrementally.
///
/// Useful when parsing markdown tables where content arrives piece by piece.
#[derive(Debug, Default)]
pub struct TableBuilder {
    headers: Vec<TableCell>,
    rows: Vec<Vec<TableCell>>,
    alignments: Vec<Alignment>,
    current_row: Vec<TableCell>,
    in_header: bool,
}

impl TableBuilder {
    /// Creates a new empty table builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts building the header row.
    ///
    /// Cells added after this call will be added to the header.
    pub fn start_header(&mut self) {
        self.in_header = true;
        self.current_row.clear();
    }

    /// Ends the header row.
    ///
    /// The accumulated cells become the table headers.
    pub fn end_header(&mut self) {
        if self.in_header {
            self.headers = std::mem::take(&mut self.current_row);
            self.in_header = false;
        }
    }

    /// Starts a new data row.
    ///
    /// Cells added after this call will be added to the current row.
    pub fn start_row(&mut self) {
        self.current_row.clear();
    }

    /// Ends the current data row.
    ///
    /// The accumulated cells are added as a new row.
    pub fn end_row(&mut self) {
        if !self.in_header && !self.current_row.is_empty() {
            self.rows.push(std::mem::take(&mut self.current_row));
        }
    }

    /// Adds a cell to the current row (header or data).
    ///
    /// # Arguments
    /// * `content` - The text content of the cell
    pub fn add_cell(&mut self, content: String) {
        self.current_row.push(TableCell::new(content));
    }

    /// Adds a cell with styled spans.
    ///
    /// # Arguments
    /// * `content` - The raw text content (for width calculation)
    /// * `spans` - Pre-styled spans for rendering
    pub fn add_cell_with_spans(&mut self, content: String, spans: Vec<Span<'static>>) {
        self.current_row.push(TableCell::with_spans(content, spans));
    }

    /// Sets the column alignments.
    ///
    /// # Arguments
    /// * `alignments` - Vector of alignments, one per column
    pub fn set_alignments(&mut self, alignments: Vec<Alignment>) {
        self.alignments = alignments;
    }

    /// Builds the final table.
    ///
    /// Note: Call `calculate_column_widths` on the result before rendering.
    pub fn build(self) -> Table {
        Table::new(self.headers, self.rows, self.alignments)
    }
}
