//! Buffer diffing algorithm for efficient terminal updates.
//!
//! This module provides functionality to compare two buffers and generate
//! a minimal set of changes needed to transform one into the other.
//! This is essential for efficient terminal rendering - instead of
//! redrawing the entire screen, we only update cells that changed.

use crate::{Buffer, Cell};

/// A single cell change in the diff.
#[derive(Clone, Debug, PartialEq)]
pub struct CellChange {
    /// X coordinate (column) of the change.
    pub x: u16,
    /// Y coordinate (row) of the change.
    pub y: u16,
    /// The new cell value.
    pub cell: Cell,
}

impl CellChange {
    /// Creates a new cell change.
    #[inline]
    pub const fn new(x: u16, y: u16, cell: Cell) -> Self {
        Self { x, y, cell }
    }
}

/// A run of consecutive cell changes on the same row.
///
/// Batching consecutive changes allows for more efficient rendering,
/// as the terminal cursor only needs to be positioned once for each run.
#[derive(Clone, Debug)]
pub struct ChangeRun {
    /// Starting X coordinate of the run.
    pub x: u16,
    /// Y coordinate (row) of the run.
    pub y: u16,
    /// Consecutive cells in this run.
    pub cells: Vec<Cell>,
}

impl ChangeRun {
    /// Creates a new change run starting at the given position.
    #[inline]
    pub fn new(x: u16, y: u16) -> Self {
        Self {
            x,
            y,
            cells: Vec::new(),
        }
    }

    /// Creates a change run with a single cell.
    #[inline]
    pub fn with_cell(x: u16, y: u16, cell: Cell) -> Self {
        Self {
            x,
            y,
            cells: vec![cell],
        }
    }

    /// Adds a cell to the run.
    #[inline]
    pub fn push(&mut self, cell: Cell) {
        self.cells.push(cell);
    }

    /// Returns the length of the run in cells.
    #[inline]
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Returns true if the run is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Returns the ending X coordinate (exclusive).
    #[inline]
    pub fn end_x(&self) -> u16 {
        self.x + self.cells.len() as u16
    }
}

/// Result of comparing two buffers.
///
/// Contains the list of changes needed to transform the "current" buffer
/// into the "next" buffer.
#[derive(Clone, Debug, Default)]
pub struct BufferDiff {
    /// Individual cell changes (unbatched).
    pub changes: Vec<CellChange>,
    /// Batched runs of consecutive changes.
    pub runs: Vec<ChangeRun>,
    /// Total number of changed cells.
    pub changed_count: usize,
}

impl BufferDiff {
    /// Creates an empty diff.
    pub fn new() -> Self {
        Self {
            changes: Vec::new(),
            runs: Vec::new(),
            changed_count: 0,
        }
    }

    /// Creates a diff with pre-allocated capacity.
    pub fn with_capacity(cell_capacity: usize, run_capacity: usize) -> Self {
        Self {
            changes: Vec::with_capacity(cell_capacity),
            runs: Vec::with_capacity(run_capacity),
            changed_count: 0,
        }
    }

    /// Returns true if there are no changes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.changed_count == 0
    }

    /// Clears all changes.
    pub fn clear(&mut self) {
        self.changes.clear();
        self.runs.clear();
        self.changed_count = 0;
    }

    /// Returns an iterator over all changed cells (from runs).
    pub fn iter_cells(&self) -> impl Iterator<Item = CellChange> + '_ {
        self.runs.iter().flat_map(|run| {
            run.cells
                .iter()
                .enumerate()
                .map(move |(i, cell)| CellChange::new(run.x + i as u16, run.y, *cell))
        })
    }
}

/// Configuration options for the diffing algorithm.
#[derive(Clone, Debug)]
pub struct DiffOptions {
    /// Epsilon for floating-point color comparison.
    /// Colors within this threshold are considered equal.
    pub color_epsilon: f32,

    /// Maximum gap between cells before starting a new run.
    /// If unchanged cells between two changes exceed this, a new run starts.
    pub max_gap: u16,

    /// Whether to generate individual changes in addition to runs.
    pub generate_individual_changes: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            color_epsilon: 0.00001,
            max_gap: 3,
            generate_individual_changes: false,
        }
    }
}

/// Computes the difference between two buffers.
///
/// Compares `current` (what's currently displayed) with `next` (what we want
/// to display) and returns the changes needed to transform current into next.
///
/// # Arguments
///
/// * `current` - The buffer currently displayed on screen
/// * `next` - The buffer we want to display
/// * `options` - Diffing configuration options
///
/// # Returns
///
/// A [`BufferDiff`] containing the changes needed.
///
/// # Examples
///
/// ```
/// use cortex_tui_buffer::{Buffer, diff};
///
/// let current = Buffer::new(80, 24);
/// let mut next = Buffer::new(80, 24);
/// next.draw_str_default(10, 5, "Hello");
///
/// let diff = diff::compute(&current, &next, &diff::DiffOptions::default());
/// assert!(!diff.is_empty());
/// ```
pub fn compute(current: &Buffer, next: &Buffer, options: &DiffOptions) -> BufferDiff {
    // Ensure buffers have same dimensions
    assert_eq!(
        (current.width(), current.height()),
        (next.width(), next.height()),
        "Buffers must have same dimensions for diffing"
    );

    let width = current.width();
    let height = current.height();

    // Pre-allocate based on expected change ratio (usually <10% changes)
    let estimated_changes = (width as usize * height as usize) / 10;
    let estimated_runs = height as usize;

    let mut diff = BufferDiff::with_capacity(
        if options.generate_individual_changes {
            estimated_changes
        } else {
            0
        },
        estimated_runs,
    );

    for y in 0..height {
        compute_row_diff(current, next, y, options, &mut diff);
    }

    diff
}

/// Computes the diff for a single row.
fn compute_row_diff(
    current: &Buffer,
    next: &Buffer,
    y: u16,
    options: &DiffOptions,
    diff: &mut BufferDiff,
) {
    let width = current.width();
    let mut current_run: Option<ChangeRun> = None;
    let mut gap_count: u16 = 0;

    for x in 0..width {
        // SAFETY: We know x and y are within bounds due to the loop bounds
        let current_cell = unsafe { current.get_unchecked(x, y) };
        let next_cell = unsafe { next.get_unchecked(x, y) };

        // Check if cells are equal (with epsilon for colors)
        let cells_equal = current_cell.approx_eq(next_cell, options.color_epsilon);

        if cells_equal {
            // Cell unchanged
            if let Some(ref mut run) = current_run {
                gap_count += 1;

                // If gap exceeds threshold, finalize this run
                if gap_count > options.max_gap {
                    if !run.is_empty() {
                        diff.runs.push(current_run.take().unwrap());
                    }
                    current_run = None;
                    gap_count = 0;
                }
            }
        } else {
            // Cell changed
            diff.changed_count += 1;

            if options.generate_individual_changes {
                diff.changes.push(CellChange::new(x, y, *next_cell));
            }

            if let Some(ref mut run) = current_run {
                // Fill in the gap with current cells (they'll be overwritten anyway)
                if gap_count > 0 {
                    for gap_x in (x - gap_count)..x {
                        // SAFETY: gap_x is within bounds since it's between previous x and current x
                        let gap_cell = unsafe { next.get_unchecked(gap_x, y) };
                        run.push(*gap_cell);
                    }
                }
                gap_count = 0;
                run.push(*next_cell);
            } else {
                // Start a new run
                current_run = Some(ChangeRun::with_cell(x, y, *next_cell));
                gap_count = 0;
            }
        }
    }

    // Finalize any remaining run
    if let Some(run) = current_run {
        if !run.is_empty() {
            diff.runs.push(run);
        }
    }
}

/// Computes the difference between two buffers with default options.
pub fn compute_default(current: &Buffer, next: &Buffer) -> BufferDiff {
    compute(current, next, &DiffOptions::default())
}

/// Computes a full-buffer diff (all cells changed).
///
/// This is useful for the first frame or after a terminal resize.
pub fn compute_full(buffer: &Buffer) -> BufferDiff {
    let width = buffer.width();
    let height = buffer.height();

    let mut diff = BufferDiff::with_capacity(0, height as usize);
    diff.changed_count = width as usize * height as usize;

    for y in 0..height {
        let mut run = ChangeRun::new(0, y);
        run.cells.reserve(width as usize);

        for x in 0..width {
            if let Some(cell) = buffer.get(x, y) {
                run.push(*cell);
            }
        }

        diff.runs.push(run);
    }

    diff
}

/// Applies a diff to a buffer, updating cells to match the changes.
///
/// This is typically used to update the "current" buffer after rendering
/// the diff to the terminal.
pub fn apply(diff: &BufferDiff, buffer: &mut Buffer) {
    for run in &diff.runs {
        for (i, cell) in run.cells.iter().enumerate() {
            buffer.set_raw(run.x + i as u16, run.y, *cell);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_tui_core::Color;

    #[test]
    fn test_empty_diff() {
        let a = Buffer::new(10, 10);
        let b = Buffer::new(10, 10);

        let diff = compute_default(&a, &b);
        assert!(diff.is_empty());
        assert_eq!(diff.changed_count, 0);
    }

    #[test]
    fn test_single_cell_change() {
        let a = Buffer::new(10, 10);
        let mut b = Buffer::new(10, 10);
        b.set_raw(5, 5, Cell::new('X'));

        let diff = compute_default(&a, &b);
        assert!(!diff.is_empty());
        assert_eq!(diff.changed_count, 1);
        assert_eq!(diff.runs.len(), 1);
        assert_eq!(diff.runs[0].x, 5);
        assert_eq!(diff.runs[0].y, 5);
        assert_eq!(diff.runs[0].cells[0].character, 'X');
    }

    #[test]
    fn test_consecutive_changes() {
        let a = Buffer::new(20, 10);
        let mut b = Buffer::new(20, 10);

        // Write "Hello" at position (5, 3)
        b.draw_str_default(5, 3, "Hello");

        let diff = compute_default(&a, &b);
        assert_eq!(diff.changed_count, 5);

        // Should be batched into a single run
        assert_eq!(diff.runs.len(), 1);
        assert_eq!(diff.runs[0].x, 5);
        assert_eq!(diff.runs[0].y, 3);
        assert_eq!(diff.runs[0].len(), 5);
    }

    #[test]
    fn test_multiple_runs_same_row() {
        let a = Buffer::new(30, 10);
        let mut b = Buffer::new(30, 10);

        // Two separate strings on same row with large gap
        b.draw_str_default(0, 5, "AAA");
        b.draw_str_default(20, 5, "BBB");

        let options = DiffOptions {
            max_gap: 3,
            ..Default::default()
        };
        let diff = compute(&a, &b, &options);

        // Should create 2 separate runs due to gap > max_gap
        assert_eq!(diff.runs.len(), 2);
        assert_eq!(diff.runs[0].x, 0);
        assert_eq!(diff.runs[1].x, 20);
    }

    #[test]
    fn test_multiple_rows() {
        let a = Buffer::new(20, 20);
        let mut b = Buffer::new(20, 20);

        b.draw_str_default(0, 0, "Line 1");
        b.draw_str_default(0, 1, "Line 2");
        b.draw_str_default(0, 2, "Line 3");

        let diff = compute_default(&a, &b);

        // Should have 3 runs (one per row)
        assert_eq!(diff.runs.len(), 3);
        assert_eq!(diff.runs[0].y, 0);
        assert_eq!(diff.runs[1].y, 1);
        assert_eq!(diff.runs[2].y, 2);
    }

    #[test]
    fn test_color_epsilon() {
        let mut a = Buffer::new(10, 10);
        let mut b = Buffer::new(10, 10);

        // Set nearly identical colors
        a.set_raw(5, 5, Cell::new('X').with_fg(Color::rgb(0.5, 0.5, 0.5)));
        b.set_raw(
            5,
            5,
            Cell::new('X').with_fg(Color::rgb(0.5000001, 0.5, 0.5)),
        );

        let options = DiffOptions {
            color_epsilon: 0.001,
            ..Default::default()
        };
        let diff = compute(&a, &b, &options);

        // Should be considered equal
        assert!(diff.is_empty());
    }

    #[test]
    fn test_apply_diff() {
        let mut current = Buffer::new(10, 10);
        let mut next = Buffer::new(10, 10);

        next.draw_str_default(2, 2, "Test");

        let diff = compute_default(&current, &next);
        apply(&diff, &mut current);

        // Current should now match next
        assert_eq!(current.get(2, 2).unwrap().character, 'T');
        assert_eq!(current.get(3, 2).unwrap().character, 'e');
        assert_eq!(current.get(4, 2).unwrap().character, 's');
        assert_eq!(current.get(5, 2).unwrap().character, 't');
    }

    #[test]
    fn test_compute_full() {
        let mut buffer = Buffer::new(5, 3);
        buffer.draw_str_default(0, 0, "12345");
        buffer.draw_str_default(0, 1, "67890");

        let diff = compute_full(&buffer);

        // Should have one run per row
        assert_eq!(diff.runs.len(), 3);
        assert_eq!(diff.changed_count, 15); // 5 * 3

        // First row should have "12345"
        assert_eq!(diff.runs[0].cells[0].character, '1');
        assert_eq!(diff.runs[0].cells[4].character, '5');
    }

    #[test]
    fn test_iter_cells() {
        let a = Buffer::new(10, 10);
        let mut b = Buffer::new(10, 10);

        b.draw_str_default(2, 2, "AB");

        let diff = compute_default(&a, &b);
        let cells: Vec<_> = diff.iter_cells().collect();

        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].x, 2);
        assert_eq!(cells[0].y, 2);
        assert_eq!(cells[0].cell.character, 'A');
        assert_eq!(cells[1].x, 3);
        assert_eq!(cells[1].y, 2);
        assert_eq!(cells[1].cell.character, 'B');
    }

    #[test]
    fn test_individual_changes_option() {
        let a = Buffer::new(10, 10);
        let mut b = Buffer::new(10, 10);
        b.draw_str_default(0, 0, "XY");

        let options = DiffOptions {
            generate_individual_changes: true,
            ..Default::default()
        };
        let diff = compute(&a, &b, &options);

        // Should have individual changes populated
        assert_eq!(diff.changes.len(), 2);
        assert_eq!(diff.changes[0].cell.character, 'X');
        assert_eq!(diff.changes[1].cell.character, 'Y');
    }
}
