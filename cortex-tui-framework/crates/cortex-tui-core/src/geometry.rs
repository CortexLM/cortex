//! Geometry types for terminal UI layout and positioning.
//!
//! This module provides fundamental geometry primitives used throughout Cortex TUI:
//! - [`Point`]: A 2D point with signed coordinates
//! - [`Size`]: A 2D size with unsigned dimensions
//! - [`Rect`]: A rectangle combining position and size
//!
//! All types are designed to be efficient, Copy, and suitable for terminal-based UIs
//! where coordinates are typically measured in character cells.

use std::ops::{Add, AddAssign, Sub, SubAssign};

/// A 2D point with signed integer coordinates.
///
/// Points can have negative coordinates to represent positions relative to
/// a viewport or parent container.
///
/// # Examples
///
/// ```
/// use cortex_tui_core::geometry::Point;
///
/// let p1 = Point::new(10, 20);
/// let p2 = Point::new(5, 5);
///
/// let p3 = p1 + p2;
/// assert_eq!(p3, Point::new(15, 25));
///
/// let p4 = p1 - p2;
/// assert_eq!(p4, Point::new(5, 15));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Point {
    /// The x coordinate (column position).
    pub x: i32,
    /// The y coordinate (row position).
    pub y: i32,
}

impl Point {
    /// The origin point (0, 0).
    pub const ZERO: Self = Self { x: 0, y: 0 };

    /// Creates a new point at the given coordinates.
    #[inline]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Creates a point from unsigned coordinates.
    ///
    /// # Panics
    ///
    /// Panics if either coordinate exceeds `i32::MAX`.
    #[inline]
    pub fn from_unsigned(x: u32, y: u32) -> Self {
        Self {
            x: x.try_into().expect("x coordinate overflow"),
            y: y.try_into().expect("y coordinate overflow"),
        }
    }

    /// Returns the point offset by the given amounts.
    #[inline]
    pub const fn offset(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
        }
    }

    /// Returns whether this point has non-negative coordinates.
    #[inline]
    pub const fn is_non_negative(self) -> bool {
        self.x >= 0 && self.y >= 0
    }

    /// Converts to unsigned coordinates, clamping negative values to 0.
    #[inline]
    pub const fn to_unsigned(self) -> (u32, u32) {
        (
            if self.x < 0 { 0 } else { self.x as u32 },
            if self.y < 0 { 0 } else { self.y as u32 },
        )
    }

    /// Returns the Manhattan distance to another point.
    #[inline]
    pub fn manhattan_distance(self, other: Self) -> u32 {
        self.x.abs_diff(other.x) + self.y.abs_diff(other.y)
    }

    /// Returns the squared Euclidean distance to another point.
    ///
    /// Use this for distance comparisons to avoid the cost of square root.
    #[inline]
    pub fn distance_squared(self, other: Self) -> i64 {
        let dx = (self.x - other.x) as i64;
        let dy = (self.y - other.y) as i64;
        dx * dx + dy * dy
    }

    /// Clamps this point to be within the given bounds.
    #[inline]
    pub fn clamp(self, min: Self, max: Self) -> Self {
        Self {
            x: self.x.clamp(min.x, max.x),
            y: self.y.clamp(min.y, max.y),
        }
    }

    /// Returns the component-wise minimum of two points.
    #[inline]
    pub fn min(self, other: Self) -> Self {
        Self {
            x: self.x.min(other.x),
            y: self.y.min(other.y),
        }
    }

    /// Returns the component-wise maximum of two points.
    #[inline]
    pub fn max(self, other: Self) -> Self {
        Self {
            x: self.x.max(other.x),
            y: self.y.max(other.y),
        }
    }
}

impl Add for Point {
    type Output = Self;

    #[inline]
    fn add(self, rhs: Self) -> Self::Output {
        Self {
            x: self.x.saturating_add(rhs.x),
            y: self.y.saturating_add(rhs.y),
        }
    }
}

impl AddAssign for Point {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for Point {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            x: self.x.saturating_sub(rhs.x),
            y: self.y.saturating_sub(rhs.y),
        }
    }
}

impl SubAssign for Point {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl From<(i32, i32)> for Point {
    #[inline]
    fn from((x, y): (i32, i32)) -> Self {
        Self::new(x, y)
    }
}

impl From<Point> for (i32, i32) {
    #[inline]
    fn from(p: Point) -> Self {
        (p.x, p.y)
    }
}

/// A 2D size with unsigned dimensions.
///
/// Represents the dimensions of a rectangular area in character cells.
/// Width represents columns and height represents rows.
///
/// # Examples
///
/// ```
/// use cortex_tui_core::geometry::Size;
///
/// let size = Size::new(80, 24);
/// assert_eq!(size.area(), 1920);
///
/// let double = size.scale(2, 2);
/// assert_eq!(double, Size::new(160, 48));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Size {
    /// The width in columns.
    pub width: u16,
    /// The height in rows.
    pub height: u16,
}

impl Size {
    /// A zero-sized area.
    pub const ZERO: Self = Self {
        width: 0,
        height: 0,
    };

    /// The maximum possible size.
    pub const MAX: Self = Self {
        width: u16::MAX,
        height: u16::MAX,
    };

    /// Creates a new size with the given dimensions.
    #[inline]
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }

    /// Creates a square size with equal width and height.
    #[inline]
    pub const fn square(side: u16) -> Self {
        Self {
            width: side,
            height: side,
        }
    }

    /// Returns the total area (width × height).
    #[inline]
    pub const fn area(self) -> u32 {
        self.width as u32 * self.height as u32
    }

    /// Returns whether either dimension is zero.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Returns whether both dimensions are non-zero.
    #[inline]
    pub const fn is_non_empty(self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// Returns the size scaled by the given factors.
    #[inline]
    pub const fn scale(self, width_factor: u16, height_factor: u16) -> Self {
        Self {
            width: self.width.saturating_mul(width_factor),
            height: self.height.saturating_mul(height_factor),
        }
    }

    /// Returns the size with width and height swapped.
    #[inline]
    pub const fn transpose(self) -> Self {
        Self {
            width: self.height,
            height: self.width,
        }
    }

    /// Returns the component-wise minimum of two sizes.
    #[inline]
    pub fn min(self, other: Self) -> Self {
        Self {
            width: self.width.min(other.width),
            height: self.height.min(other.height),
        }
    }

    /// Returns the component-wise maximum of two sizes.
    #[inline]
    pub fn max(self, other: Self) -> Self {
        Self {
            width: self.width.max(other.width),
            height: self.height.max(other.height),
        }
    }

    /// Returns the size expanded by the given amounts.
    #[inline]
    pub const fn expand(self, dw: u16, dh: u16) -> Self {
        Self {
            width: self.width.saturating_add(dw),
            height: self.height.saturating_add(dh),
        }
    }

    /// Returns the size shrunk by the given amounts.
    #[inline]
    pub const fn shrink(self, dw: u16, dh: u16) -> Self {
        Self {
            width: self.width.saturating_sub(dw),
            height: self.height.saturating_sub(dh),
        }
    }

    /// Returns whether this size can contain the other size.
    #[inline]
    pub const fn contains_size(self, other: Self) -> bool {
        self.width >= other.width && self.height >= other.height
    }
}

impl From<(u16, u16)> for Size {
    #[inline]
    fn from((width, height): (u16, u16)) -> Self {
        Self::new(width, height)
    }
}

impl From<Size> for (u16, u16) {
    #[inline]
    fn from(size: Size) -> Self {
        (size.width, size.height)
    }
}

/// A rectangle defined by its position and size.
///
/// The rectangle is defined by an origin point (top-left corner) and a size.
/// The position can be negative for relative positioning.
///
/// # Coordinate System
///
/// The coordinate system uses (0, 0) as the top-left corner, with x increasing
/// to the right and y increasing downward:
///
/// ```text
/// (0,0) ──────► x
///   │
///   │
///   ▼
///   y
/// ```
///
/// # Examples
///
/// ```
/// use cortex_tui_core::geometry::{Point, Size, Rect};
///
/// let rect = Rect::new(10, 20, 80, 24);
/// assert_eq!(rect.left(), 10);
/// assert_eq!(rect.top(), 20);
/// assert_eq!(rect.right(), 90);
/// assert_eq!(rect.bottom(), 44);
///
/// let point = Point::new(50, 30);
/// assert!(rect.contains_point(point));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    /// The x coordinate of the left edge.
    pub x: i32,
    /// The y coordinate of the top edge.
    pub y: i32,
    /// The width of the rectangle.
    pub width: u16,
    /// The height of the rectangle.
    pub height: u16,
}

impl Rect {
    /// A zero-sized rectangle at the origin.
    pub const ZERO: Self = Self {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };

    /// Creates a new rectangle at the given position with the given size.
    #[inline]
    pub const fn new(x: i32, y: i32, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Creates a rectangle from a position point and size.
    #[inline]
    pub const fn from_point_size(origin: Point, size: Size) -> Self {
        Self {
            x: origin.x,
            y: origin.y,
            width: size.width,
            height: size.height,
        }
    }

    /// Creates a rectangle from two corner points.
    ///
    /// The corners can be specified in any order; the resulting rectangle
    /// will have the top-left at the minimum coordinates.
    #[inline]
    pub fn from_corners(p1: Point, p2: Point) -> Self {
        let min_x = p1.x.min(p2.x);
        let min_y = p1.y.min(p2.y);
        let max_x = p1.x.max(p2.x);
        let max_y = p1.y.max(p2.y);

        // Calculate dimensions, handling potential overflow
        let width = (max_x - min_x).min(u16::MAX as i32) as u16;
        let height = (max_y - min_y).min(u16::MAX as i32) as u16;

        Self {
            x: min_x,
            y: min_y,
            width,
            height,
        }
    }

    /// Creates a rectangle from left, top, right, bottom coordinates.
    #[inline]
    pub fn from_ltrb(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        let width = (right - left).max(0).min(u16::MAX as i32) as u16;
        let height = (bottom - top).max(0).min(u16::MAX as i32) as u16;
        Self {
            x: left,
            y: top,
            width,
            height,
        }
    }

    /// Creates a rectangle at the origin with the given size.
    #[inline]
    pub const fn from_size(size: Size) -> Self {
        Self {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
        }
    }

    /// Returns the position (top-left corner) of the rectangle.
    #[inline]
    pub const fn position(self) -> Point {
        Point {
            x: self.x,
            y: self.y,
        }
    }

    /// Returns the size of the rectangle.
    #[inline]
    pub const fn size(self) -> Size {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    /// Returns the x coordinate of the left edge.
    #[inline]
    pub const fn left(self) -> i32 {
        self.x
    }

    /// Returns the y coordinate of the top edge.
    #[inline]
    pub const fn top(self) -> i32 {
        self.y
    }

    /// Returns the x coordinate of the right edge (exclusive).
    #[inline]
    pub const fn right(self) -> i32 {
        self.x.saturating_add(self.width as i32)
    }

    /// Returns the y coordinate of the bottom edge (exclusive).
    #[inline]
    pub const fn bottom(self) -> i32 {
        self.y.saturating_add(self.height as i32)
    }

    /// Returns the top-left corner point.
    #[inline]
    pub const fn top_left(self) -> Point {
        Point {
            x: self.x,
            y: self.y,
        }
    }

    /// Returns the top-right corner point.
    #[inline]
    pub const fn top_right(self) -> Point {
        Point {
            x: self.right(),
            y: self.y,
        }
    }

    /// Returns the bottom-left corner point.
    #[inline]
    pub const fn bottom_left(self) -> Point {
        Point {
            x: self.x,
            y: self.bottom(),
        }
    }

    /// Returns the bottom-right corner point.
    #[inline]
    pub const fn bottom_right(self) -> Point {
        Point {
            x: self.right(),
            y: self.bottom(),
        }
    }

    /// Returns the center point of the rectangle.
    #[inline]
    pub const fn center(self) -> Point {
        Point {
            x: self.x.saturating_add(self.width as i32 / 2),
            y: self.y.saturating_add(self.height as i32 / 2),
        }
    }

    /// Returns the total area of the rectangle.
    #[inline]
    pub const fn area(self) -> u32 {
        self.width as u32 * self.height as u32
    }

    /// Returns whether the rectangle has zero area.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Returns whether the rectangle contains the given point.
    #[inline]
    pub const fn contains_point(self, point: Point) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }

    /// Returns whether this rectangle completely contains another rectangle.
    #[inline]
    pub const fn contains_rect(self, other: Self) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    /// Returns whether this rectangle intersects with another rectangle.
    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// Returns the intersection of this rectangle with another.
    ///
    /// Returns `None` if the rectangles do not intersect.
    #[inline]
    pub fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());

        if x < right && y < bottom {
            Some(Self {
                x,
                y,
                width: (right - x) as u16,
                height: (bottom - y) as u16,
            })
        } else {
            None
        }
    }

    /// Returns the smallest rectangle that contains both this and another rectangle.
    #[inline]
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }

        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());

        Self {
            x,
            y,
            width: (right - x).min(u16::MAX as i32) as u16,
            height: (bottom - y).min(u16::MAX as i32) as u16,
        }
    }

    /// Returns the rectangle moved by the given offset.
    #[inline]
    pub const fn translate(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            width: self.width,
            height: self.height,
        }
    }

    /// Returns the rectangle moved to the given position.
    #[inline]
    pub const fn with_position(self, position: Point) -> Self {
        Self {
            x: position.x,
            y: position.y,
            width: self.width,
            height: self.height,
        }
    }

    /// Returns the rectangle with the given size.
    #[inline]
    pub const fn with_size(self, size: Size) -> Self {
        Self {
            x: self.x,
            y: self.y,
            width: size.width,
            height: size.height,
        }
    }

    /// Returns the rectangle inset by the given amounts on all sides.
    ///
    /// If the inset would result in negative dimensions, the dimensions
    /// are clamped to zero.
    #[inline]
    pub const fn inset(self, horizontal: u16, vertical: u16) -> Self {
        let double_h = (horizontal as u32) * 2;
        let double_v = (vertical as u32) * 2;

        let new_width = if (self.width as u32) > double_h {
            self.width - horizontal * 2
        } else {
            0
        };
        let new_height = if (self.height as u32) > double_v {
            self.height - vertical * 2
        } else {
            0
        };

        Self {
            x: self.x.saturating_add(horizontal as i32),
            y: self.y.saturating_add(vertical as i32),
            width: new_width,
            height: new_height,
        }
    }

    /// Returns the rectangle inset by different amounts on each side.
    #[inline]
    pub const fn inset_sides(self, left: u16, top: u16, right: u16, bottom: u16) -> Self {
        let total_h = left as u32 + right as u32;
        let total_v = top as u32 + bottom as u32;

        let new_width = if (self.width as u32) > total_h {
            (self.width as u32 - total_h) as u16
        } else {
            0
        };
        let new_height = if (self.height as u32) > total_v {
            (self.height as u32 - total_v) as u16
        } else {
            0
        };

        Self {
            x: self.x.saturating_add(left as i32),
            y: self.y.saturating_add(top as i32),
            width: new_width,
            height: new_height,
        }
    }

    /// Returns the rectangle expanded by the given amounts on all sides.
    #[inline]
    pub const fn expand(self, horizontal: u16, vertical: u16) -> Self {
        Self {
            x: self.x.saturating_sub(horizontal as i32),
            y: self.y.saturating_sub(vertical as i32),
            width: self.width.saturating_add(horizontal * 2),
            height: self.height.saturating_add(vertical * 2),
        }
    }

    /// Splits the rectangle horizontally at the given offset from the left.
    ///
    /// Returns `(left, right)` rectangles. If the offset is greater than
    /// the width, the right rectangle will be empty.
    #[inline]
    pub const fn split_horizontal(self, offset: u16) -> (Self, Self) {
        let left_width = if offset > self.width {
            self.width
        } else {
            offset
        };
        let right_width = self.width.saturating_sub(offset);

        let left = Self {
            x: self.x,
            y: self.y,
            width: left_width,
            height: self.height,
        };
        let right = Self {
            x: self.x.saturating_add(left_width as i32),
            y: self.y,
            width: right_width,
            height: self.height,
        };

        (left, right)
    }

    /// Splits the rectangle vertically at the given offset from the top.
    ///
    /// Returns `(top, bottom)` rectangles. If the offset is greater than
    /// the height, the bottom rectangle will be empty.
    #[inline]
    pub const fn split_vertical(self, offset: u16) -> (Self, Self) {
        let top_height = if offset > self.height {
            self.height
        } else {
            offset
        };
        let bottom_height = self.height.saturating_sub(offset);

        let top = Self {
            x: self.x,
            y: self.y,
            width: self.width,
            height: top_height,
        };
        let bottom = Self {
            x: self.x,
            y: self.y.saturating_add(top_height as i32),
            width: self.width,
            height: bottom_height,
        };

        (top, bottom)
    }

    /// Returns an iterator over all points in the rectangle.
    #[inline]
    pub fn points(self) -> impl Iterator<Item = Point> {
        let x_start = self.x;
        let y_start = self.y;
        let x_end = self.right();
        let y_end = self.bottom();

        (y_start..y_end).flat_map(move |y| (x_start..x_end).map(move |x| Point::new(x, y)))
    }

    /// Returns an iterator over the row indices in the rectangle.
    #[inline]
    pub fn rows(self) -> impl Iterator<Item = i32> {
        self.y..self.bottom()
    }

    /// Returns an iterator over the column indices in the rectangle.
    #[inline]
    pub fn columns(self) -> impl Iterator<Item = i32> {
        self.x..self.right()
    }

    /// Clamps this rectangle to fit within another rectangle.
    #[inline]
    pub fn clamp_to(self, bounds: Self) -> Self {
        self.intersection(bounds).unwrap_or(Self::ZERO)
    }

    /// Alias for `clamp_to` - clamps this rectangle within bounds.
    #[inline]
    pub fn clamp_within(self, bounds: Self) -> Self {
        self.clamp_to(bounds)
    }

    /// Returns whether the rectangle contains the point at (x, y).
    #[inline]
    pub const fn contains_xy(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Converts a point in absolute coordinates to relative coordinates
    /// within this rectangle.
    #[inline]
    pub const fn to_local(self, point: Point) -> Point {
        Point {
            x: point.x.saturating_sub(self.x),
            y: point.y.saturating_sub(self.y),
        }
    }

    /// Converts a point in relative coordinates (within this rectangle)
    /// to absolute coordinates.
    #[inline]
    pub const fn to_absolute(self, point: Point) -> Point {
        Point {
            x: point.x.saturating_add(self.x),
            y: point.y.saturating_add(self.y),
        }
    }
}

impl From<(i32, i32, u16, u16)> for Rect {
    #[inline]
    fn from((x, y, width, height): (i32, i32, u16, u16)) -> Self {
        Self::new(x, y, width, height)
    }
}

impl From<Rect> for (i32, i32, u16, u16) {
    #[inline]
    fn from(rect: Rect) -> Self {
        (rect.x, rect.y, rect.width, rect.height)
    }
}

impl From<Size> for Rect {
    #[inline]
    fn from(size: Size) -> Self {
        Self::from_size(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod point_tests {
        use super::*;

        #[test]
        fn test_point_new() {
            let p = Point::new(10, 20);
            assert_eq!(p.x, 10);
            assert_eq!(p.y, 20);
        }

        #[test]
        fn test_point_zero() {
            assert_eq!(Point::ZERO, Point::new(0, 0));
        }

        #[test]
        fn test_point_offset() {
            let p = Point::new(10, 20);
            let offset = p.offset(5, -3);
            assert_eq!(offset, Point::new(15, 17));
        }

        #[test]
        fn test_point_add() {
            let p1 = Point::new(10, 20);
            let p2 = Point::new(5, 5);
            assert_eq!(p1 + p2, Point::new(15, 25));
        }

        #[test]
        fn test_point_sub() {
            let p1 = Point::new(10, 20);
            let p2 = Point::new(5, 5);
            assert_eq!(p1 - p2, Point::new(5, 15));
        }

        #[test]
        fn test_point_manhattan_distance() {
            let p1 = Point::new(0, 0);
            let p2 = Point::new(3, 4);
            assert_eq!(p1.manhattan_distance(p2), 7);
        }

        #[test]
        fn test_point_distance_squared() {
            let p1 = Point::new(0, 0);
            let p2 = Point::new(3, 4);
            assert_eq!(p1.distance_squared(p2), 25); // 3² + 4² = 25
        }

        #[test]
        fn test_point_clamp() {
            let p = Point::new(100, 200);
            let clamped = p.clamp(Point::new(0, 0), Point::new(50, 100));
            assert_eq!(clamped, Point::new(50, 100));
        }

        #[test]
        fn test_point_is_non_negative() {
            assert!(Point::new(0, 0).is_non_negative());
            assert!(Point::new(10, 20).is_non_negative());
            assert!(!Point::new(-1, 0).is_non_negative());
            assert!(!Point::new(0, -1).is_non_negative());
        }

        #[test]
        fn test_point_to_unsigned() {
            assert_eq!(Point::new(10, 20).to_unsigned(), (10, 20));
            assert_eq!(Point::new(-5, -10).to_unsigned(), (0, 0));
            assert_eq!(Point::new(-5, 20).to_unsigned(), (0, 20));
        }

        #[test]
        fn test_point_from_tuple() {
            let p: Point = (10, 20).into();
            assert_eq!(p, Point::new(10, 20));
        }
    }

    mod size_tests {
        use super::*;

        #[test]
        fn test_size_new() {
            let s = Size::new(80, 24);
            assert_eq!(s.width, 80);
            assert_eq!(s.height, 24);
        }

        #[test]
        fn test_size_zero() {
            assert_eq!(Size::ZERO, Size::new(0, 0));
            assert!(Size::ZERO.is_empty());
        }

        #[test]
        fn test_size_area() {
            let s = Size::new(80, 24);
            assert_eq!(s.area(), 1920);
        }

        #[test]
        fn test_size_is_empty() {
            assert!(Size::new(0, 10).is_empty());
            assert!(Size::new(10, 0).is_empty());
            assert!(Size::new(0, 0).is_empty());
            assert!(!Size::new(10, 10).is_empty());
        }

        #[test]
        fn test_size_scale() {
            let s = Size::new(10, 20);
            assert_eq!(s.scale(2, 3), Size::new(20, 60));
        }

        #[test]
        fn test_size_transpose() {
            let s = Size::new(80, 24);
            assert_eq!(s.transpose(), Size::new(24, 80));
        }

        #[test]
        fn test_size_expand_shrink() {
            let s = Size::new(10, 20);
            assert_eq!(s.expand(5, 10), Size::new(15, 30));
            assert_eq!(s.shrink(3, 5), Size::new(7, 15));
        }

        #[test]
        fn test_size_contains_size() {
            let outer = Size::new(100, 100);
            let inner = Size::new(50, 50);
            assert!(outer.contains_size(inner));
            assert!(!inner.contains_size(outer));
        }
    }

    mod rect_tests {
        use super::*;

        #[test]
        fn test_rect_new() {
            let r = Rect::new(10, 20, 80, 24);
            assert_eq!(r.x, 10);
            assert_eq!(r.y, 20);
            assert_eq!(r.width, 80);
            assert_eq!(r.height, 24);
        }

        #[test]
        fn test_rect_edges() {
            let r = Rect::new(10, 20, 80, 24);
            assert_eq!(r.left(), 10);
            assert_eq!(r.top(), 20);
            assert_eq!(r.right(), 90);
            assert_eq!(r.bottom(), 44);
        }

        #[test]
        fn test_rect_corners() {
            let r = Rect::new(10, 20, 80, 24);
            assert_eq!(r.top_left(), Point::new(10, 20));
            assert_eq!(r.top_right(), Point::new(90, 20));
            assert_eq!(r.bottom_left(), Point::new(10, 44));
            assert_eq!(r.bottom_right(), Point::new(90, 44));
        }

        #[test]
        fn test_rect_center() {
            let r = Rect::new(0, 0, 100, 100);
            assert_eq!(r.center(), Point::new(50, 50));
        }

        #[test]
        fn test_rect_from_corners() {
            let r = Rect::from_corners(Point::new(10, 20), Point::new(90, 44));
            assert_eq!(r, Rect::new(10, 20, 80, 24));

            // Test with corners in reverse order
            let r2 = Rect::from_corners(Point::new(90, 44), Point::new(10, 20));
            assert_eq!(r2, Rect::new(10, 20, 80, 24));
        }

        #[test]
        fn test_rect_contains_point() {
            let r = Rect::new(10, 20, 80, 24);
            assert!(r.contains_point(Point::new(10, 20))); // top-left
            assert!(r.contains_point(Point::new(50, 30))); // center
            assert!(!r.contains_point(Point::new(90, 20))); // right edge (exclusive)
            assert!(!r.contains_point(Point::new(10, 44))); // bottom edge (exclusive)
            assert!(!r.contains_point(Point::new(5, 30))); // outside left
        }

        #[test]
        fn test_rect_contains_rect() {
            let outer = Rect::new(0, 0, 100, 100);
            let inner = Rect::new(10, 10, 50, 50);
            assert!(outer.contains_rect(inner));
            assert!(!inner.contains_rect(outer));
        }

        #[test]
        fn test_rect_intersects() {
            let r1 = Rect::new(0, 0, 50, 50);
            let r2 = Rect::new(25, 25, 50, 50);
            let r3 = Rect::new(100, 100, 50, 50);

            assert!(r1.intersects(r2));
            assert!(r2.intersects(r1));
            assert!(!r1.intersects(r3));
        }

        #[test]
        fn test_rect_intersection() {
            let r1 = Rect::new(0, 0, 50, 50);
            let r2 = Rect::new(25, 25, 50, 50);

            let intersection = r1.intersection(r2);
            assert_eq!(intersection, Some(Rect::new(25, 25, 25, 25)));

            let r3 = Rect::new(100, 100, 50, 50);
            assert_eq!(r1.intersection(r3), None);
        }

        #[test]
        fn test_rect_union() {
            let r1 = Rect::new(0, 0, 50, 50);
            let r2 = Rect::new(25, 25, 50, 50);

            let union = r1.union(r2);
            assert_eq!(union, Rect::new(0, 0, 75, 75));
        }

        #[test]
        fn test_rect_translate() {
            let r = Rect::new(10, 20, 80, 24);
            let moved = r.translate(5, -5);
            assert_eq!(moved, Rect::new(15, 15, 80, 24));
        }

        #[test]
        fn test_rect_inset() {
            let r = Rect::new(0, 0, 100, 100);
            let inset = r.inset(10, 20);
            assert_eq!(inset, Rect::new(10, 20, 80, 60));
        }

        #[test]
        fn test_rect_inset_sides() {
            let r = Rect::new(0, 0, 100, 100);
            let inset = r.inset_sides(5, 10, 15, 20);
            assert_eq!(inset, Rect::new(5, 10, 80, 70));
        }

        #[test]
        fn test_rect_expand() {
            let r = Rect::new(10, 10, 80, 80);
            let expanded = r.expand(5, 10);
            assert_eq!(expanded, Rect::new(5, 0, 90, 100));
        }

        #[test]
        fn test_rect_split_horizontal() {
            let r = Rect::new(0, 0, 100, 50);
            let (left, right) = r.split_horizontal(40);
            assert_eq!(left, Rect::new(0, 0, 40, 50));
            assert_eq!(right, Rect::new(40, 0, 60, 50));
        }

        #[test]
        fn test_rect_split_vertical() {
            let r = Rect::new(0, 0, 100, 50);
            let (top, bottom) = r.split_vertical(20);
            assert_eq!(top, Rect::new(0, 0, 100, 20));
            assert_eq!(bottom, Rect::new(0, 20, 100, 30));
        }

        #[test]
        fn test_rect_points_iterator() {
            let r = Rect::new(0, 0, 3, 2);
            let points: Vec<Point> = r.points().collect();
            assert_eq!(
                points,
                vec![
                    Point::new(0, 0),
                    Point::new(1, 0),
                    Point::new(2, 0),
                    Point::new(0, 1),
                    Point::new(1, 1),
                    Point::new(2, 1),
                ]
            );
        }

        #[test]
        fn test_rect_to_local_absolute() {
            let r = Rect::new(10, 20, 80, 24);
            let absolute = Point::new(50, 30);
            let local = r.to_local(absolute);
            assert_eq!(local, Point::new(40, 10));

            let back = r.to_absolute(local);
            assert_eq!(back, absolute);
        }
    }
}
