//! Layout engine for `Cortex TUI` using flexbox-based layout.
//!
//! This crate provides a flexbox layout system built on top of [taffy](https://github.com/DioxusLabs/taffy).
//! It manages a tree of layout nodes, computes their positions and sizes, and provides
//! utilities for building complex UI layouts.
//!
//! # Overview
//!
//! The layout system consists of several key components:
//!
//! - [`LayoutTree`]: The main container that manages layout nodes and coordinates with taffy
//! - [`LayoutNode`]: Individual nodes in the tree with style properties and computed layout
//! - [`LayoutStyle`]: Style properties that control how nodes are laid out (flexbox properties)
//! - [`ComputedLayout`]: The resolved position and size after layout calculation
//!
//! # Example
//!
//! ```rust
//! use cortex_tui_layout::{LayoutTree, LayoutNodeBuilder, FlexDirection, JustifyContent};
//!
//! // Create a layout tree
//! let mut tree = LayoutTree::new();
//!
//! // Create a root container with column direction
//! let root = tree.create_node_with_builder(
//!     LayoutNodeBuilder::new()
//!         .flex_direction(FlexDirection::Column)
//!         .width(100.0)
//!         .height(100.0)
//! ).unwrap();
//!
//! tree.set_root(root).unwrap();
//!
//! // Create child nodes
//! let header = tree.create_node_with_builder(
//!     LayoutNodeBuilder::new()
//!         .height(20.0)
//! ).unwrap();
//!
//! let content = tree.create_node_with_builder(
//!     LayoutNodeBuilder::new()
//!         .flex_grow(1.0)
//! ).unwrap();
//!
//! // Build the tree structure
//! tree.add_child(root, header).unwrap();
//! tree.add_child(root, content).unwrap();
//!
//! // Compute layout
//! tree.compute_layout(100.0, 100.0);
//!
//! // Get computed positions
//! let header_layout = tree.get_computed_layout(header).unwrap();
//! println!("Header: {}x{} at ({}, {})",
//!     header_layout.width, header_layout.height,
//!     header_layout.x, header_layout.y);
//! ```
//!
//! # Flexbox Model
//!
//! This crate implements the CSS Flexbox layout model:
//!
//! - **Flex Direction**: Controls the main axis (row or column)
//! - **Flex Wrap**: Controls whether items wrap to new lines
//! - **Justify Content**: Alignment along the main axis
//! - **Align Items**: Alignment along the cross axis
//! - **Flex Grow/Shrink**: How items grow or shrink to fill space
//!
//! # Performance
//!
//! The layout system uses dirty tracking to avoid unnecessary recomputation.
//! Layout is only recalculated when:
//!
//! - A node's style changes
//! - The tree structure changes (add/remove children)
//! - The available size changes
//! - [`LayoutTree::mark_dirty`] is called explicitly

#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::missing_fields_in_debug)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::float_cmp)]

mod computed;
mod node;
mod style;
mod tree;

// Re-export all public types
pub use computed::{
    ComputedLayout, LayoutPoint, LayoutRect, LayoutSize, ResolvedEdges, WorldLayout,
};
pub use node::{LayoutNode, LayoutNodeBuilder, LayoutStyle};
pub use style::{
    AlignContent, AlignItems, AlignSelf, Dimension, Display, Edges, FlexDirection, FlexWrap,
    JustifyContent, LengthPercentage, LengthPercentageAuto, Overflow, Position, Size as StyleSize,
};
pub use tree::{LayoutError, LayoutNodeKey, LayoutResult, LayoutTree};

/// Prelude module for convenient imports.
///
/// # Example
///
/// ```rust
/// use cortex_tui_layout::prelude::*;
/// ```
pub mod prelude {
    pub use crate::computed::{
        ComputedLayout, LayoutPoint, LayoutRect, LayoutSize, ResolvedEdges, WorldLayout,
    };
    pub use crate::node::{LayoutNode, LayoutNodeBuilder, LayoutStyle};
    pub use crate::style::{
        AlignContent, AlignItems, AlignSelf, Dimension, Display, Edges, FlexDirection, FlexWrap,
        JustifyContent, LengthPercentage, LengthPercentageAuto, Overflow, Position,
    };
    pub use crate::tree::{LayoutError, LayoutNodeKey, LayoutResult, LayoutTree};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_layout() {
        let mut tree = LayoutTree::new();

        // Create root
        let root = tree
            .create_node_with_builder(
                LayoutNodeBuilder::new()
                    .flex_direction(FlexDirection::Column)
                    .width(100.0)
                    .height(100.0),
            )
            .unwrap();

        tree.set_root(root).unwrap();

        // Create children
        let child1 = tree
            .create_node_with_builder(LayoutNodeBuilder::new().height(30.0))
            .unwrap();

        let child2 = tree
            .create_node_with_builder(LayoutNodeBuilder::new().flex_grow(1.0))
            .unwrap();

        tree.add_child(root, child1).unwrap();
        tree.add_child(root, child2).unwrap();

        // Compute layout
        tree.compute_layout(100.0, 100.0);

        // Verify layout
        let root_layout = tree.get_computed_layout(root).unwrap();
        assert_eq!(root_layout.width, 100.0);
        assert_eq!(root_layout.height, 100.0);

        let child1_layout = tree.get_computed_layout(child1).unwrap();
        assert_eq!(child1_layout.height, 30.0);

        let child2_layout = tree.get_computed_layout(child2).unwrap();
        assert_eq!(child2_layout.height, 70.0); // Remaining space
    }

    #[test]
    fn test_nested_layout() {
        let mut tree = LayoutTree::new();

        // Create a nested structure
        let root = tree
            .create_node_with_builder(
                LayoutNodeBuilder::new()
                    .flex_direction(FlexDirection::Row)
                    .width(200.0)
                    .height(100.0),
            )
            .unwrap();

        tree.set_root(root).unwrap();

        // Left panel
        let left = tree
            .create_node_with_builder(
                LayoutNodeBuilder::new()
                    .width(50.0)
                    .flex_direction(FlexDirection::Column),
            )
            .unwrap();

        // Right panel (flexible)
        let right = tree
            .create_node_with_builder(
                LayoutNodeBuilder::new()
                    .flex_grow(1.0)
                    .flex_direction(FlexDirection::Column),
            )
            .unwrap();

        tree.add_child(root, left).unwrap();
        tree.add_child(root, right).unwrap();

        // Add items to left panel
        let left_item = tree
            .create_node_with_builder(LayoutNodeBuilder::new().height(20.0))
            .unwrap();
        tree.add_child(left, left_item).unwrap();

        // Compute
        tree.compute_layout(200.0, 100.0);

        let left_layout = tree.get_computed_layout(left).unwrap();
        assert_eq!(left_layout.width, 50.0);
        assert_eq!(left_layout.x, 0.0);

        let right_layout = tree.get_computed_layout(right).unwrap();
        assert_eq!(right_layout.width, 150.0); // 200 - 50
        assert_eq!(right_layout.x, 50.0);
    }

    #[test]
    fn test_padding_and_margin() {
        let mut tree = LayoutTree::new();

        let root = tree
            .create_node_with_builder(
                LayoutNodeBuilder::new()
                    .width(100.0)
                    .height(100.0)
                    .padding_all(10.0),
            )
            .unwrap();

        tree.set_root(root).unwrap();

        let child = tree
            .create_node_with_builder(LayoutNodeBuilder::new().flex_grow(1.0).margin_all(5.0))
            .unwrap();

        tree.add_child(root, child).unwrap();

        tree.compute_layout(100.0, 100.0);

        let root_layout = tree.get_computed_layout(root).unwrap();
        assert_eq!(root_layout.padding.top, 10.0);
        assert_eq!(root_layout.padding.right, 10.0);

        let child_layout = tree.get_computed_layout(child).unwrap();
        // Child should be inset by parent's padding + its own margin
        assert!(child_layout.x >= 15.0); // padding + margin
    }

    #[test]
    fn test_justify_content_center() {
        let mut tree = LayoutTree::new();

        let root = tree
            .create_node_with_builder(
                LayoutNodeBuilder::new()
                    .flex_direction(FlexDirection::Row)
                    .justify_content(JustifyContent::Center)
                    .width(100.0)
                    .height(50.0),
            )
            .unwrap();

        tree.set_root(root).unwrap();

        let child = tree
            .create_node_with_builder(LayoutNodeBuilder::new().width(40.0).height(20.0))
            .unwrap();

        tree.add_child(root, child).unwrap();

        tree.compute_layout(100.0, 50.0);

        let child_layout = tree.get_computed_layout(child).unwrap();
        // Child should be centered: (100 - 40) / 2 = 30
        assert_eq!(child_layout.x, 30.0);
    }

    #[test]
    fn test_align_items_center() {
        let mut tree = LayoutTree::new();

        let root = tree
            .create_node_with_builder(
                LayoutNodeBuilder::new()
                    .flex_direction(FlexDirection::Row)
                    .align_items(AlignItems::Center)
                    .width(100.0)
                    .height(100.0),
            )
            .unwrap();

        tree.set_root(root).unwrap();

        let child = tree
            .create_node_with_builder(LayoutNodeBuilder::new().width(50.0).height(20.0))
            .unwrap();

        tree.add_child(root, child).unwrap();

        tree.compute_layout(100.0, 100.0);

        let child_layout = tree.get_computed_layout(child).unwrap();
        // Child should be vertically centered: (100 - 20) / 2 = 40
        assert_eq!(child_layout.y, 40.0);
    }
}
