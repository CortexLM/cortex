//! Layout types for widget positioning.
//!
//! This module defines layout-related types including dimensions, flex properties,
//! and the overall layout style used by widgets.

/// A dimension value that can be auto, fixed, or percentage.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Dimension {
    /// Automatically determined by content or layout algorithm.
    #[default]
    Auto,
    /// Fixed number of units (terminal cells).
    Points(f32),
    /// Percentage of parent's size (0.0 to 100.0).
    Percent(f32),
}

impl Dimension {
    /// Creates an auto dimension.
    pub const fn auto() -> Self {
        Self::Auto
    }

    /// Creates a fixed dimension.
    pub const fn points(value: f32) -> Self {
        Self::Points(value)
    }

    /// Creates a percentage dimension.
    pub const fn percent(value: f32) -> Self {
        Self::Percent(value)
    }

    /// Resolves this dimension to a concrete value.
    pub fn resolve(&self, parent_size: f32) -> Option<f32> {
        match self {
            Self::Auto => None,
            Self::Points(p) => Some(*p),
            Self::Percent(pct) => Some(parent_size * pct / 100.0),
        }
    }

    /// Returns true if this is an auto dimension.
    pub const fn is_auto(&self) -> bool {
        matches!(self, Self::Auto)
    }
}

impl From<u16> for Dimension {
    fn from(value: u16) -> Self {
        Self::Points(value as f32)
    }
}

impl From<f32> for Dimension {
    fn from(value: f32) -> Self {
        Self::Points(value)
    }
}

/// Flex direction for layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlexDirection {
    /// Items are laid out in a row (left to right).
    #[default]
    Row,
    /// Items are laid out in a column (top to bottom).
    Column,
    /// Items are laid out in a reversed row (right to left).
    RowReverse,
    /// Items are laid out in a reversed column (bottom to top).
    ColumnReverse,
}

impl FlexDirection {
    /// Returns true if this is a row direction.
    pub const fn is_row(&self) -> bool {
        matches!(self, Self::Row | Self::RowReverse)
    }

    /// Returns true if this is a column direction.
    pub const fn is_column(&self) -> bool {
        matches!(self, Self::Column | Self::ColumnReverse)
    }

    /// Returns true if this is a reversed direction.
    pub const fn is_reverse(&self) -> bool {
        matches!(self, Self::RowReverse | Self::ColumnReverse)
    }
}

/// How items are aligned on the cross axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlignItems {
    /// Items are aligned at the start of the cross axis.
    #[default]
    Start,
    /// Items are aligned at the end of the cross axis.
    End,
    /// Items are centered on the cross axis.
    Center,
    /// Items are stretched to fill the cross axis.
    Stretch,
    /// Items are aligned to their baselines.
    Baseline,
}

/// How items are justified on the main axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JustifyContent {
    /// Items are packed at the start of the main axis.
    #[default]
    Start,
    /// Items are packed at the end of the main axis.
    End,
    /// Items are centered on the main axis.
    Center,
    /// Items are evenly distributed; first item at start, last at end.
    SpaceBetween,
    /// Items are evenly distributed with equal space around each.
    SpaceAround,
    /// Items are evenly distributed with equal space between.
    SpaceEvenly,
}

/// How content overflows the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overflow {
    /// Content is not clipped and may overflow.
    #[default]
    Visible,
    /// Content is clipped to the container bounds.
    Hidden,
    /// Scrollbars are added if content overflows.
    Scroll,
}

/// Position type for layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionType {
    /// Element is positioned relative to its normal position.
    #[default]
    Relative,
    /// Element is positioned relative to its containing block.
    Absolute,
}

/// Edge values for padding, margin, and position offsets.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EdgeDimensions {
    pub top: Dimension,
    pub right: Dimension,
    pub bottom: Dimension,
    pub left: Dimension,
}

impl EdgeDimensions {
    /// Creates new edge dimensions.
    pub const fn new(top: Dimension, right: Dimension, bottom: Dimension, left: Dimension) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Creates uniform edge dimensions.
    pub const fn uniform(value: Dimension) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    /// Creates symmetric edge dimensions (vertical, horizontal).
    pub const fn symmetric(vertical: Dimension, horizontal: Dimension) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    /// Zero dimensions (all points at 0).
    pub const ZERO: Self = Self {
        top: Dimension::Points(0.0),
        right: Dimension::Points(0.0),
        bottom: Dimension::Points(0.0),
        left: Dimension::Points(0.0),
    };

    /// Auto dimensions (all auto).
    pub const AUTO: Self = Self {
        top: Dimension::Auto,
        right: Dimension::Auto,
        bottom: Dimension::Auto,
        left: Dimension::Auto,
    };
}

/// Complete layout style for a widget.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutStyle {
    /// Width of the widget.
    pub width: Dimension,
    /// Height of the widget.
    pub height: Dimension,
    /// Minimum width constraint.
    pub min_width: Dimension,
    /// Maximum width constraint.
    pub max_width: Dimension,
    /// Minimum height constraint.
    pub min_height: Dimension,
    /// Maximum height constraint.
    pub max_height: Dimension,

    /// Flex grow factor.
    pub flex_grow: f32,
    /// Flex shrink factor.
    pub flex_shrink: f32,
    /// Flex basis (initial size before growing/shrinking).
    pub flex_basis: Dimension,
    /// Flex direction (for containers).
    pub flex_direction: FlexDirection,
    /// How to wrap flex items.
    pub flex_wrap: FlexWrap,

    /// Gap between children (for containers).
    pub gap: f32,
    /// Column gap (horizontal gap between children).
    pub column_gap: f32,
    /// Row gap (vertical gap between children).
    pub row_gap: f32,

    /// How items are aligned on the cross axis.
    pub align_items: AlignItems,
    /// How this item is aligned on the cross axis (overrides parent's align_items).
    pub align_self: AlignSelf,
    /// How items are justified on the main axis.
    pub justify_content: JustifyContent,

    /// Padding inside the widget.
    pub padding: EdgeDimensions,
    /// Margin outside the widget.
    pub margin: EdgeDimensions,

    /// Position type.
    pub position_type: PositionType,
    /// Position offsets (for absolute positioning).
    pub position: EdgeDimensions,

    /// Overflow handling.
    pub overflow: Overflow,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        Self {
            width: Dimension::Auto,
            height: Dimension::Auto,
            min_width: Dimension::Auto,
            max_width: Dimension::Auto,
            min_height: Dimension::Auto,
            max_height: Dimension::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Dimension::Auto,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            gap: 0.0,
            column_gap: 0.0,
            row_gap: 0.0,
            align_items: AlignItems::Stretch,
            align_self: AlignSelf::Auto,
            justify_content: JustifyContent::Start,
            padding: EdgeDimensions::ZERO,
            margin: EdgeDimensions::ZERO,
            position_type: PositionType::Relative,
            position: EdgeDimensions::AUTO,
            overflow: Overflow::Visible,
        }
    }
}

impl LayoutStyle {
    /// Creates a new default layout style.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the width.
    pub fn width(mut self, width: impl Into<Dimension>) -> Self {
        self.width = width.into();
        self
    }

    /// Sets the height.
    pub fn height(mut self, height: impl Into<Dimension>) -> Self {
        self.height = height.into();
        self
    }

    /// Sets both width and height.
    pub fn size(mut self, width: impl Into<Dimension>, height: impl Into<Dimension>) -> Self {
        self.width = width.into();
        self.height = height.into();
        self
    }

    /// Sets the flex grow factor.
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.flex_grow = grow;
        self
    }

    /// Sets the flex shrink factor.
    pub fn flex_shrink(mut self, shrink: f32) -> Self {
        self.flex_shrink = shrink;
        self
    }

    /// Sets the flex direction.
    pub fn flex_direction(mut self, direction: FlexDirection) -> Self {
        self.flex_direction = direction;
        self
    }

    /// Sets uniform padding.
    pub fn padding(mut self, padding: impl Into<Dimension>) -> Self {
        self.padding = EdgeDimensions::uniform(padding.into());
        self
    }

    /// Sets padding for each edge.
    pub fn padding_edges(
        mut self,
        top: impl Into<Dimension>,
        right: impl Into<Dimension>,
        bottom: impl Into<Dimension>,
        left: impl Into<Dimension>,
    ) -> Self {
        self.padding = EdgeDimensions::new(top.into(), right.into(), bottom.into(), left.into());
        self
    }

    /// Sets uniform margin.
    pub fn margin(mut self, margin: impl Into<Dimension>) -> Self {
        self.margin = EdgeDimensions::uniform(margin.into());
        self
    }

    /// Sets the gap between children.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self.column_gap = gap;
        self.row_gap = gap;
        self
    }

    /// Sets item alignment.
    pub fn align_items(mut self, align: AlignItems) -> Self {
        self.align_items = align;
        self
    }

    /// Sets content justification.
    pub fn justify_content(mut self, justify: JustifyContent) -> Self {
        self.justify_content = justify;
        self
    }

    /// Sets overflow handling.
    pub fn overflow(mut self, overflow: Overflow) -> Self {
        self.overflow = overflow;
        self
    }

    /// Sets position type to absolute.
    pub fn absolute(mut self) -> Self {
        self.position_type = PositionType::Absolute;
        self
    }
}

/// How flex items wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlexWrap {
    /// Items do not wrap.
    #[default]
    NoWrap,
    /// Items wrap to additional rows/columns.
    Wrap,
    /// Items wrap in reverse order.
    WrapReverse,
}

/// Self alignment (overrides parent's align_items).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlignSelf {
    /// Use the parent's align_items value.
    #[default]
    Auto,
    /// Align at start of cross axis.
    Start,
    /// Align at end of cross axis.
    End,
    /// Center on cross axis.
    Center,
    /// Stretch to fill cross axis.
    Stretch,
    /// Align to baseline.
    Baseline,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dimension_resolve() {
        assert_eq!(Dimension::Points(10.0).resolve(100.0), Some(10.0));
        assert_eq!(Dimension::Percent(50.0).resolve(100.0), Some(50.0));
        assert_eq!(Dimension::Auto.resolve(100.0), None);
    }

    #[test]
    fn test_layout_style_builder() {
        let style = LayoutStyle::new()
            .width(100u16)
            .height(50u16)
            .flex_grow(1.0)
            .padding(2u16);

        assert_eq!(style.width, Dimension::Points(100.0));
        assert_eq!(style.height, Dimension::Points(50.0));
        assert_eq!(style.flex_grow, 1.0);
        assert_eq!(style.padding.top, Dimension::Points(2.0));
    }
}
