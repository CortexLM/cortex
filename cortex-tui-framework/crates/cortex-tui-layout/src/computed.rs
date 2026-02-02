//! Computed layout values.
//!
//! This module provides types representing the resolved layout values
//! after the layout algorithm has run. All values are in f32 to match
//! taffy's internal representation.

/// A 2D point with f32 coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LayoutPoint {
    /// X coordinate.
    pub x: f32,
    /// Y coordinate.
    pub y: f32,
}

impl LayoutPoint {
    /// Creates a new point.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Creates a point at the origin.
    #[must_use]
    pub const fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    /// Offsets this point by the given delta.
    #[must_use]
    pub fn offset(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
        }
    }
}

/// A 2D size with f32 dimensions.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LayoutSize {
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl LayoutSize {
    /// Creates a new size.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// Creates a zero size.
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            width: 0.0,
            height: 0.0,
        }
    }

    /// Returns the area.
    #[must_use]
    pub fn area(self) -> f32 {
        self.width * self.height
    }

    /// Checks if empty (either dimension is zero or negative).
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

/// A rectangle with f32 coordinates and dimensions.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LayoutRect {
    /// X coordinate.
    pub x: f32,
    /// Y coordinate.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl LayoutRect {
    /// Creates a new rectangle.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Creates a zero rectangle.
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        }
    }

    /// Returns the right edge x coordinate.
    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Returns the bottom edge y coordinate.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Checks if this rectangle contains a point.
    #[must_use]
    pub fn contains_point(&self, point: LayoutPoint) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }

    /// Checks if this rectangle intersects with another.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }

    /// Returns the intersection of two rectangles, or None if they don't intersect.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        if !self.intersects(other) {
            return None;
        }

        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());

        Some(Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }
}

/// Resolved edge values in points.
///
/// Represents the computed values for padding, margin, or border
/// after layout calculation.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ResolvedEdges {
    /// Top edge value in points.
    pub top: f32,
    /// Right edge value in points.
    pub right: f32,
    /// Bottom edge value in points.
    pub bottom: f32,
    /// Left edge value in points.
    pub left: f32,
}

impl ResolvedEdges {
    /// Creates new resolved edges.
    #[must_use]
    pub const fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Creates resolved edges with all values set to zero.
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    /// Creates resolved edges with the same value on all sides.
    #[must_use]
    pub const fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    /// Returns the total horizontal (left + right) value.
    #[must_use]
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    /// Returns the total vertical (top + bottom) value.
    #[must_use]
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }

    /// Returns the sum of all edge values.
    #[must_use]
    pub fn total(&self) -> f32 {
        self.top + self.right + self.bottom + self.left
    }
}

/// The computed layout values for a node after layout calculation.
///
/// This struct contains all the resolved position and size values
/// that result from running the layout algorithm.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ComputedLayout {
    /// X position relative to parent's content box.
    pub x: f32,
    /// Y position relative to parent's content box.
    pub y: f32,
    /// Total width including padding and border.
    pub width: f32,
    /// Total height including padding and border.
    pub height: f32,
    /// Width of the content area (excluding padding and border).
    pub content_width: f32,
    /// Height of the content area (excluding padding and border).
    pub content_height: f32,
    /// Resolved padding values.
    pub padding: ResolvedEdges,
    /// Resolved border values.
    pub border: ResolvedEdges,
    /// Resolved margin values.
    pub margin: ResolvedEdges,
    /// Scroll offset for scrollable containers.
    pub scroll_x: f32,
    /// Scroll offset for scrollable containers.
    pub scroll_y: f32,
}

impl ComputedLayout {
    /// Creates a new computed layout with default values.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            content_width: 0.0,
            content_height: 0.0,
            padding: ResolvedEdges::zero(),
            border: ResolvedEdges::zero(),
            margin: ResolvedEdges::zero(),
            scroll_x: 0.0,
            scroll_y: 0.0,
        }
    }

    /// Creates a computed layout from a taffy layout.
    #[must_use]
    pub fn from_taffy(layout: &taffy::Layout) -> Self {
        let padding = ResolvedEdges::new(
            layout.padding.top,
            layout.padding.right,
            layout.padding.bottom,
            layout.padding.left,
        );

        let border = ResolvedEdges::new(
            layout.border.top,
            layout.border.right,
            layout.border.bottom,
            layout.border.left,
        );

        // Note: taffy doesn't expose margin in Layout, so we use zero
        let margin = ResolvedEdges::zero();

        // Content size is the total size minus padding and border
        let content_width =
            (layout.size.width - padding.horizontal() - border.horizontal()).max(0.0);
        let content_height = (layout.size.height - padding.vertical() - border.vertical()).max(0.0);

        Self {
            x: layout.location.x,
            y: layout.location.y,
            width: layout.size.width.max(0.0),
            height: layout.size.height.max(0.0),
            content_width,
            content_height,
            padding,
            border,
            margin,
            scroll_x: layout.scrollbar_size.width,
            scroll_y: layout.scrollbar_size.height,
        }
    }

    /// Returns the position as a [`LayoutPoint`].
    #[must_use]
    pub const fn position(&self) -> LayoutPoint {
        LayoutPoint {
            x: self.x,
            y: self.y,
        }
    }

    /// Returns the size (width and height) as a [`LayoutSize`].
    #[must_use]
    pub const fn size(&self) -> LayoutSize {
        LayoutSize {
            width: self.width,
            height: self.height,
        }
    }

    /// Returns the content size (excluding padding and border).
    #[must_use]
    pub const fn content_size(&self) -> LayoutSize {
        LayoutSize {
            width: self.content_width,
            height: self.content_height,
        }
    }

    /// Returns the bounding rectangle of this layout.
    #[must_use]
    pub const fn bounds(&self) -> LayoutRect {
        LayoutRect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }

    /// Returns the content rectangle (excluding padding and border).
    #[must_use]
    pub fn content_bounds(&self) -> LayoutRect {
        LayoutRect {
            x: self.x + self.border.left + self.padding.left,
            y: self.y + self.border.top + self.padding.top,
            width: self.content_width,
            height: self.content_height,
        }
    }

    /// Returns the inner rectangle (excluding border, including padding).
    #[must_use]
    pub fn inner_bounds(&self) -> LayoutRect {
        LayoutRect {
            x: self.x + self.border.left,
            y: self.y + self.border.top,
            width: self.width - self.border.horizontal(),
            height: self.height - self.border.vertical(),
        }
    }

    /// Returns the right edge x coordinate.
    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Returns the bottom edge y coordinate.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Returns the center x coordinate.
    #[must_use]
    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    /// Returns the center y coordinate.
    #[must_use]
    pub fn center_y(&self) -> f32 {
        self.y + self.height / 2.0
    }

    /// Returns the center point.
    #[must_use]
    pub fn center(&self) -> LayoutPoint {
        LayoutPoint {
            x: self.center_x(),
            y: self.center_y(),
        }
    }

    /// Checks if a point is within this layout's bounds.
    #[must_use]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Checks if a [`LayoutPoint`] is within this layout's bounds.
    #[must_use]
    pub fn contains_point(&self, point: LayoutPoint) -> bool {
        self.contains(point.x, point.y)
    }

    /// Checks if a point is within this layout's content bounds.
    #[must_use]
    pub fn content_contains(&self, x: f32, y: f32) -> bool {
        self.content_bounds().contains_point(LayoutPoint::new(x, y))
    }

    /// Returns the total horizontal space taken by border, padding, and margin.
    #[must_use]
    pub fn horizontal_spacing(&self) -> f32 {
        self.margin.horizontal() + self.border.horizontal() + self.padding.horizontal()
    }

    /// Returns the total vertical space taken by border, padding, and margin.
    #[must_use]
    pub fn vertical_spacing(&self) -> f32 {
        self.margin.vertical() + self.border.vertical() + self.padding.vertical()
    }

    /// Offsets this layout by the given delta.
    #[must_use]
    pub fn offset(&self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }

    /// Converts local coordinates to world coordinates given parent's world position.
    #[must_use]
    pub fn to_world(&self, parent_x: f32, parent_y: f32) -> Self {
        self.offset(parent_x, parent_y)
    }
}

/// Cached world-space layout values.
///
/// This struct caches the absolute (world-space) position and size
/// of a node, computed by walking up the parent chain.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WorldLayout {
    /// Absolute X position in world coordinates.
    pub x: f32,
    /// Absolute Y position in world coordinates.
    pub y: f32,
    /// Width of the node.
    pub width: f32,
    /// Height of the node.
    pub height: f32,
}

impl WorldLayout {
    /// Creates a new world layout.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Creates from a computed layout and parent world position.
    #[must_use]
    pub fn from_computed(computed: &ComputedLayout, parent_x: f32, parent_y: f32) -> Self {
        Self {
            x: parent_x + computed.x,
            y: parent_y + computed.y,
            width: computed.width,
            height: computed.height,
        }
    }

    /// Returns the bounding rectangle.
    #[must_use]
    pub const fn bounds(&self) -> LayoutRect {
        LayoutRect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }

    /// Checks if a point is within this layout's bounds.
    #[must_use]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// Checks if a [`LayoutPoint`] is within this layout's bounds.
    #[must_use]
    pub fn contains_point(&self, point: LayoutPoint) -> bool {
        self.contains(point.x, point.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_point() {
        let p = LayoutPoint::new(10.0, 20.0);
        let offset = p.offset(5.0, -5.0);
        assert_eq!(offset.x, 15.0);
        assert_eq!(offset.y, 15.0);
    }

    #[test]
    fn test_layout_size() {
        let s = LayoutSize::new(100.0, 50.0);
        assert_eq!(s.area(), 5000.0);
        assert!(!s.is_empty());
        assert!(LayoutSize::zero().is_empty());
    }

    #[test]
    fn test_layout_rect_contains() {
        let r = LayoutRect::new(10.0, 10.0, 100.0, 50.0);
        assert!(r.contains_point(LayoutPoint::new(50.0, 30.0)));
        assert!(!r.contains_point(LayoutPoint::new(0.0, 0.0)));
        assert!(!r.contains_point(LayoutPoint::new(110.0, 30.0)));
    }

    #[test]
    fn test_layout_rect_intersection() {
        let r1 = LayoutRect::new(0.0, 0.0, 100.0, 100.0);
        let r2 = LayoutRect::new(50.0, 50.0, 100.0, 100.0);
        let intersection = r1.intersection(&r2).unwrap();
        assert_eq!(intersection.x, 50.0);
        assert_eq!(intersection.y, 50.0);
        assert_eq!(intersection.width, 50.0);
        assert_eq!(intersection.height, 50.0);
    }

    #[test]
    fn test_resolved_edges() {
        let edges = ResolvedEdges::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(edges.horizontal(), 6.0);
        assert_eq!(edges.vertical(), 4.0);
        assert_eq!(edges.total(), 10.0);
    }

    #[test]
    fn test_computed_layout_bounds() {
        let layout = ComputedLayout {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
            content_width: 80.0,
            content_height: 30.0,
            padding: ResolvedEdges::all(5.0),
            border: ResolvedEdges::all(5.0),
            margin: ResolvedEdges::zero(),
            scroll_x: 0.0,
            scroll_y: 0.0,
        };

        assert_eq!(layout.right(), 110.0);
        assert_eq!(layout.bottom(), 70.0);
        assert_eq!(layout.center_x(), 60.0);
        assert_eq!(layout.center_y(), 45.0);
    }

    #[test]
    fn test_computed_layout_contains() {
        let layout = ComputedLayout {
            x: 10.0,
            y: 10.0,
            width: 100.0,
            height: 100.0,
            ..Default::default()
        };

        assert!(layout.contains(50.0, 50.0));
        assert!(layout.contains(10.0, 10.0));
        assert!(!layout.contains(110.0, 50.0));
        assert!(!layout.contains(5.0, 50.0));
    }

    #[test]
    fn test_computed_layout_content_bounds() {
        let layout = ComputedLayout {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            content_width: 80.0,
            content_height: 80.0,
            padding: ResolvedEdges::all(5.0),
            border: ResolvedEdges::all(5.0),
            margin: ResolvedEdges::zero(),
            scroll_x: 0.0,
            scroll_y: 0.0,
        };

        let content_bounds = layout.content_bounds();
        assert_eq!(content_bounds.x, 10.0);
        assert_eq!(content_bounds.y, 10.0);
        assert_eq!(content_bounds.width, 80.0);
        assert_eq!(content_bounds.height, 80.0);
    }

    #[test]
    fn test_world_layout() {
        let computed = ComputedLayout {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
            ..Default::default()
        };

        let world = WorldLayout::from_computed(&computed, 100.0, 200.0);
        assert_eq!(world.x, 110.0);
        assert_eq!(world.y, 220.0);
        assert_eq!(world.width, 100.0);
        assert_eq!(world.height, 50.0);
    }
}
