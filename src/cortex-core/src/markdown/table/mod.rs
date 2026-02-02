//! ASCII Table Renderer
//!
//! Provides complete ASCII box-drawing table rendering with:
//! - Full box-drawing border characters
//! - Column alignment (left, center, right)
//! - Unicode-aware width calculation
//! - Automatic column width distribution
//! - Content truncation with ellipsis
//!
//! ## Example Output
//!
//! ```text
//! ┌──────────┬───────────┬─────────┐
//! │ Header 1 │ Header 2  │ Header 3│
//! ├──────────┼───────────┼─────────┤
//! │ Cell 1   │ Cell 2    │ Cell 3  │
//! │ Cell 4   │ Cell 5    │ Cell 6  │
//! └──────────┴───────────┴─────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use cortex_engine::markdown::table::{TableBuilder, render_table, Alignment};
//! use ratatui::style::{Style, Color};
//!
//! let mut builder = TableBuilder::new();
//! builder.start_header();
//! builder.add_cell("Name".to_string());
//! builder.add_cell("Value".to_string());
//! builder.end_header();
//!
//! builder.start_row();
//! builder.add_cell("foo".to_string());
//! builder.add_cell("bar".to_string());
//! builder.end_row();
//!
//! builder.set_alignments(vec![Alignment::Left, Alignment::Right]);
//! let table = builder.build();
//!
//! let lines = render_table(
//!     &table,
//!     Color::Gray,
//!     Style::default().fg(Color::White),
//!     Style::default(),
//!     80,
//! );
//! ```

// Sub-modules
pub mod border;
mod builder;
mod render;
#[cfg(test)]
mod tests;
mod types;
pub mod utils;

// Re-exports for backwards compatibility
pub use builder::TableBuilder;
pub use render::{render_table, render_table_simple};
pub use types::{Alignment, Table, TableCell};

// Internal re-exports for use within the crate
pub(crate) use types::{CELL_PADDING, MIN_COLUMN_WIDTH};
