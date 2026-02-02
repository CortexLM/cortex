//! Layout node types.
//!
//! This module provides the `LayoutNode` type which represents a node
//! in the layout tree with its style properties and computed layout.

use crate::computed::ComputedLayout;
use crate::style::{
    AlignContent, AlignItems, AlignSelf, Dimension, Display, Edges, FlexDirection, FlexWrap,
    JustifyContent, LengthPercentage, LengthPercentageAuto, Overflow, Position, Size,
};

/// Style properties for a layout node.
///
/// This struct contains all the flexbox style properties that affect
/// how a node is laid out within its parent.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutStyle {
    /// Display mode (Flex or None).
    pub display: Display,
    /// Position type (Relative or Absolute).
    pub position: Position,
    /// Flex container direction.
    pub flex_direction: FlexDirection,
    /// Flex item wrapping behavior.
    pub flex_wrap: FlexWrap,
    /// Alignment of items along the main axis.
    pub justify_content: JustifyContent,
    /// Alignment of items along the cross axis.
    pub align_items: AlignItems,
    /// Alignment of lines in multi-line containers.
    pub align_content: AlignContent,
    /// Self-alignment override.
    pub align_self: AlignSelf,
    /// Flex grow factor.
    pub flex_grow: f32,
    /// Flex shrink factor.
    pub flex_shrink: f32,
    /// Flex basis (initial main size).
    pub flex_basis: Dimension,
    /// Node size.
    pub size: Size<Dimension>,
    /// Minimum size constraints.
    pub min_size: Size<Dimension>,
    /// Maximum size constraints.
    pub max_size: Size<Dimension>,
    /// Aspect ratio constraint.
    pub aspect_ratio: Option<f32>,
    /// Padding values.
    pub padding: Edges<LengthPercentage>,
    /// Margin values.
    pub margin: Edges<LengthPercentageAuto>,
    /// Border values.
    pub border: Edges<LengthPercentage>,
    /// Position insets for absolute positioning.
    pub inset: Edges<LengthPercentageAuto>,
    /// Gap between flex items.
    pub gap: Size<LengthPercentage>,
    /// Overflow behavior.
    pub overflow: Overflow,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        Self {
            display: Display::Flex,
            position: Position::Relative,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            justify_content: JustifyContent::Start,
            align_items: AlignItems::Stretch,
            align_content: AlignContent::Stretch,
            align_self: AlignSelf::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Dimension::Auto,
            size: Size::default(),
            min_size: Size::default(),
            max_size: Size::default(),
            aspect_ratio: None,
            padding: Edges::default(),
            margin: Edges::default(),
            border: Edges::default(),
            inset: Edges::default(),
            gap: Size::default(),
            overflow: Overflow::Visible,
        }
    }
}

impl LayoutStyle {
    /// Converts this style to a taffy style.
    #[must_use]
    pub fn to_taffy(&self) -> taffy::Style {
        taffy::Style {
            display: self.display.into(),
            position: self.position.into(),
            flex_direction: self.flex_direction.into(),
            flex_wrap: self.flex_wrap.into(),
            justify_content: Some(self.justify_content.into()),
            align_items: Some(self.align_items.into()),
            align_content: Some(self.align_content.into()),
            align_self: self.align_self.to_taffy_option(),
            flex_grow: self.flex_grow,
            flex_shrink: self.flex_shrink,
            flex_basis: self.flex_basis.into(),
            size: self.size.into(),
            min_size: self.min_size.into(),
            max_size: self.max_size.into(),
            aspect_ratio: self.aspect_ratio,
            padding: self.padding.into(),
            margin: self.margin.into(),
            border: self.border.into(),
            inset: self.inset.into(),
            gap: self.gap.into(),
            overflow: taffy::Point {
                x: self.overflow.into(),
                y: self.overflow.into(),
            },
            ..Default::default()
        }
    }
}

/// A node in the layout tree.
///
/// `LayoutNode` combines style properties with computed layout values
/// and parent-child relationships.
#[derive(Debug, Clone)]
pub struct LayoutNode {
    /// The taffy node ID for this node.
    pub(crate) taffy_node: taffy::NodeId,
    /// Style properties.
    pub style: LayoutStyle,
    /// Cached computed layout.
    pub computed: ComputedLayout,
    /// Parent node key (if any).
    pub parent: Option<slotmap::DefaultKey>,
    /// Child node keys in layout order.
    pub children: Vec<slotmap::DefaultKey>,
    /// Z-index for rendering order.
    pub z_index: i32,
    /// Translation offset for scrolling.
    pub translate_x: f32,
    /// Translation offset for scrolling.
    pub translate_y: f32,
    /// Whether this node is visible.
    pub visible: bool,
    /// User-defined data tag.
    pub tag: Option<String>,
}

impl LayoutNode {
    /// Creates a new layout node with the given taffy node ID and style.
    #[must_use]
    pub(crate) fn new(taffy_node: taffy::NodeId, style: LayoutStyle) -> Self {
        Self {
            taffy_node,
            style,
            computed: ComputedLayout::new(),
            parent: None,
            children: Vec::new(),
            z_index: 0,
            translate_x: 0.0,
            translate_y: 0.0,
            visible: true,
            tag: None,
        }
    }

    /// Returns true if this node has no parent.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.parent.is_none()
    }

    /// Returns true if this node has no children.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    /// Returns the number of children.
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// Returns the world X position accounting for translation.
    #[must_use]
    pub fn world_x(&self, parent_world_x: f32) -> f32 {
        parent_world_x + self.computed.x + self.translate_x
    }

    /// Returns the world Y position accounting for translation.
    #[must_use]
    pub fn world_y(&self, parent_world_y: f32) -> f32 {
        parent_world_y + self.computed.y + self.translate_y
    }

    /// Checks if the computed layout has valid dimensions.
    #[must_use]
    pub fn has_valid_size(&self) -> bool {
        self.computed.width > 0.0 && self.computed.height > 0.0
    }
}

/// Builder for creating layout nodes with a fluent API.
#[derive(Debug, Clone, Default)]
pub struct LayoutNodeBuilder {
    style: LayoutStyle,
    z_index: i32,
    translate_x: f32,
    translate_y: f32,
    visible: bool,
    tag: Option<String>,
}

impl LayoutNodeBuilder {
    /// Creates a new builder with default values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            style: LayoutStyle::default(),
            z_index: 0,
            translate_x: 0.0,
            translate_y: 0.0,
            visible: true,
            tag: None,
        }
    }

    /// Sets the display mode.
    #[must_use]
    pub fn display(mut self, display: Display) -> Self {
        self.style.display = display;
        self
    }

    /// Sets the position type.
    #[must_use]
    pub fn position(mut self, position: Position) -> Self {
        self.style.position = position;
        self
    }

    /// Sets the flex direction.
    #[must_use]
    pub fn flex_direction(mut self, direction: FlexDirection) -> Self {
        self.style.flex_direction = direction;
        self
    }

    /// Sets the flex wrap behavior.
    #[must_use]
    pub fn flex_wrap(mut self, wrap: FlexWrap) -> Self {
        self.style.flex_wrap = wrap;
        self
    }

    /// Sets the justify content alignment.
    #[must_use]
    pub fn justify_content(mut self, justify: JustifyContent) -> Self {
        self.style.justify_content = justify;
        self
    }

    /// Sets the align items alignment.
    #[must_use]
    pub fn align_items(mut self, align: AlignItems) -> Self {
        self.style.align_items = align;
        self
    }

    /// Sets the align content alignment.
    #[must_use]
    pub fn align_content(mut self, align: AlignContent) -> Self {
        self.style.align_content = align;
        self
    }

    /// Sets the align self override.
    #[must_use]
    pub fn align_self(mut self, align: AlignSelf) -> Self {
        self.style.align_self = align;
        self
    }

    /// Sets the flex grow factor.
    #[must_use]
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.style.flex_grow = grow;
        self
    }

    /// Sets the flex shrink factor.
    #[must_use]
    pub fn flex_shrink(mut self, shrink: f32) -> Self {
        self.style.flex_shrink = shrink;
        self
    }

    /// Sets the flex basis.
    #[must_use]
    pub fn flex_basis(mut self, basis: Dimension) -> Self {
        self.style.flex_basis = basis;
        self
    }

    /// Sets flex shorthand (grow, shrink, basis).
    #[must_use]
    pub fn flex(mut self, grow: f32, shrink: f32, basis: Dimension) -> Self {
        self.style.flex_grow = grow;
        self.style.flex_shrink = shrink;
        self.style.flex_basis = basis;
        self
    }

    /// Sets the width.
    #[must_use]
    pub fn width(mut self, width: impl Into<Dimension>) -> Self {
        self.style.size.width = width.into();
        self
    }

    /// Sets the height.
    #[must_use]
    pub fn height(mut self, height: impl Into<Dimension>) -> Self {
        self.style.size.height = height.into();
        self
    }

    /// Sets both width and height.
    #[must_use]
    pub fn size(mut self, width: impl Into<Dimension>, height: impl Into<Dimension>) -> Self {
        self.style.size.width = width.into();
        self.style.size.height = height.into();
        self
    }

    /// Sets the minimum width.
    #[must_use]
    pub fn min_width(mut self, width: impl Into<Dimension>) -> Self {
        self.style.min_size.width = width.into();
        self
    }

    /// Sets the minimum height.
    #[must_use]
    pub fn min_height(mut self, height: impl Into<Dimension>) -> Self {
        self.style.min_size.height = height.into();
        self
    }

    /// Sets the maximum width.
    #[must_use]
    pub fn max_width(mut self, width: impl Into<Dimension>) -> Self {
        self.style.max_size.width = width.into();
        self
    }

    /// Sets the maximum height.
    #[must_use]
    pub fn max_height(mut self, height: impl Into<Dimension>) -> Self {
        self.style.max_size.height = height.into();
        self
    }

    /// Sets the aspect ratio.
    #[must_use]
    pub fn aspect_ratio(mut self, ratio: f32) -> Self {
        self.style.aspect_ratio = Some(ratio);
        self
    }

    /// Sets padding on all sides.
    #[must_use]
    pub fn padding_all(mut self, padding: impl Into<LengthPercentage>) -> Self {
        self.style.padding = Edges::all(padding.into());
        self
    }

    /// Sets padding individually for each side.
    #[must_use]
    pub fn padding(
        mut self,
        top: impl Into<LengthPercentage>,
        right: impl Into<LengthPercentage>,
        bottom: impl Into<LengthPercentage>,
        left: impl Into<LengthPercentage>,
    ) -> Self {
        self.style.padding = Edges::new(top.into(), right.into(), bottom.into(), left.into());
        self
    }

    /// Sets horizontal and vertical padding.
    #[must_use]
    pub fn padding_axes(
        mut self,
        vertical: impl Into<LengthPercentage>,
        horizontal: impl Into<LengthPercentage>,
    ) -> Self {
        self.style.padding = Edges::axes(vertical.into(), horizontal.into());
        self
    }

    /// Sets margin on all sides.
    #[must_use]
    pub fn margin_all(mut self, margin: impl Into<LengthPercentageAuto>) -> Self {
        self.style.margin = Edges::all(margin.into());
        self
    }

    /// Sets margin individually for each side.
    #[must_use]
    pub fn margin(
        mut self,
        top: impl Into<LengthPercentageAuto>,
        right: impl Into<LengthPercentageAuto>,
        bottom: impl Into<LengthPercentageAuto>,
        left: impl Into<LengthPercentageAuto>,
    ) -> Self {
        self.style.margin = Edges::new(top.into(), right.into(), bottom.into(), left.into());
        self
    }

    /// Sets horizontal and vertical margin.
    #[must_use]
    pub fn margin_axes(
        mut self,
        vertical: impl Into<LengthPercentageAuto>,
        horizontal: impl Into<LengthPercentageAuto>,
    ) -> Self {
        self.style.margin = Edges::axes(vertical.into(), horizontal.into());
        self
    }

    /// Sets border on all sides.
    #[must_use]
    pub fn border_all(mut self, border: impl Into<LengthPercentage>) -> Self {
        self.style.border = Edges::all(border.into());
        self
    }

    /// Sets border individually for each side.
    #[must_use]
    pub fn border(
        mut self,
        top: impl Into<LengthPercentage>,
        right: impl Into<LengthPercentage>,
        bottom: impl Into<LengthPercentage>,
        left: impl Into<LengthPercentage>,
    ) -> Self {
        self.style.border = Edges::new(top.into(), right.into(), bottom.into(), left.into());
        self
    }

    /// Sets the gap between flex items.
    #[must_use]
    pub fn gap(mut self, gap: impl Into<LengthPercentage>) -> Self {
        let gap = gap.into();
        self.style.gap = Size::new(gap, gap);
        self
    }

    /// Sets row and column gap separately.
    #[must_use]
    pub fn gap_axes(
        mut self,
        row_gap: impl Into<LengthPercentage>,
        column_gap: impl Into<LengthPercentage>,
    ) -> Self {
        self.style.gap = Size::new(column_gap.into(), row_gap.into());
        self
    }

    /// Sets the overflow behavior.
    #[must_use]
    pub fn overflow(mut self, overflow: Overflow) -> Self {
        self.style.overflow = overflow;
        self
    }

    /// Sets the top inset (for absolute positioning).
    #[must_use]
    pub fn top(mut self, top: impl Into<LengthPercentageAuto>) -> Self {
        self.style.inset.top = top.into();
        self
    }

    /// Sets the right inset (for absolute positioning).
    #[must_use]
    pub fn right(mut self, right: impl Into<LengthPercentageAuto>) -> Self {
        self.style.inset.right = right.into();
        self
    }

    /// Sets the bottom inset (for absolute positioning).
    #[must_use]
    pub fn bottom(mut self, bottom: impl Into<LengthPercentageAuto>) -> Self {
        self.style.inset.bottom = bottom.into();
        self
    }

    /// Sets the left inset (for absolute positioning).
    #[must_use]
    pub fn left(mut self, left: impl Into<LengthPercentageAuto>) -> Self {
        self.style.inset.left = left.into();
        self
    }

    /// Sets the z-index for rendering order.
    #[must_use]
    pub fn z_index(mut self, z_index: i32) -> Self {
        self.z_index = z_index;
        self
    }

    /// Sets the initial translation offset.
    #[must_use]
    pub fn translate(mut self, x: f32, y: f32) -> Self {
        self.translate_x = x;
        self.translate_y = y;
        self
    }

    /// Sets the visibility.
    #[must_use]
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// Sets a user-defined tag.
    #[must_use]
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// Returns the style for this builder.
    #[must_use]
    pub fn style(&self) -> &LayoutStyle {
        &self.style
    }

    /// Consumes the builder and returns the style.
    #[must_use]
    pub fn into_style(self) -> LayoutStyle {
        self.style
    }

    /// Internal method to build a node with a taffy node ID.
    pub(crate) fn build_with_taffy_node(self, taffy_node: taffy::NodeId) -> LayoutNode {
        let mut node = LayoutNode::new(taffy_node, self.style);
        node.z_index = self.z_index;
        node.translate_x = self.translate_x;
        node.translate_y = self.translate_y;
        node.visible = self.visible;
        node.tag = self.tag;
        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_style() {
        let style = LayoutStyle::default();
        assert_eq!(style.display, Display::Flex);
        assert_eq!(style.position, Position::Relative);
        assert_eq!(style.flex_direction, FlexDirection::Row);
        assert_eq!(style.flex_grow, 0.0);
        assert_eq!(style.flex_shrink, 1.0);
    }

    #[test]
    fn test_builder_chaining() {
        let builder = LayoutNodeBuilder::new()
            .flex_direction(FlexDirection::Column)
            .justify_content(JustifyContent::Center)
            .align_items(AlignItems::Center)
            .width(100.0)
            .height(50.0)
            .padding_all(5.0)
            .z_index(10);

        let style = builder.style();
        assert_eq!(style.flex_direction, FlexDirection::Column);
        assert_eq!(style.justify_content, JustifyContent::Center);
        assert_eq!(style.align_items, AlignItems::Center);
        assert_eq!(style.size.width, Dimension::Points(100.0));
        assert_eq!(style.size.height, Dimension::Points(50.0));
    }

    #[test]
    fn test_builder_flex_shorthand() {
        let builder = LayoutNodeBuilder::new().flex(1.0, 0.0, Dimension::Points(100.0));

        let style = builder.style();
        assert_eq!(style.flex_grow, 1.0);
        assert_eq!(style.flex_shrink, 0.0);
        assert_eq!(style.flex_basis, Dimension::Points(100.0));
    }

    #[test]
    fn test_builder_padding() {
        let builder = LayoutNodeBuilder::new().padding(1.0, 2.0, 3.0, 4.0);

        let style = builder.style();
        assert_eq!(style.padding.top, LengthPercentage::Points(1.0));
        assert_eq!(style.padding.right, LengthPercentage::Points(2.0));
        assert_eq!(style.padding.bottom, LengthPercentage::Points(3.0));
        assert_eq!(style.padding.left, LengthPercentage::Points(4.0));
    }

    #[test]
    fn test_builder_with_tag() {
        let builder = LayoutNodeBuilder::new().tag("container").visible(false);

        assert_eq!(builder.tag, Some("container".to_string()));
        assert!(!builder.visible);
    }
}
