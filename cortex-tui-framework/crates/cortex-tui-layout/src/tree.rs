//! Layout tree management.
//!
//! This module provides the `LayoutTree` type which manages a tree of layout nodes
//! and uses taffy for flexbox layout calculations.

use slotmap::{DefaultKey, SlotMap};
use taffy::{AvailableSpace, NodeId as TaffyNodeId, TaffyTree};

use crate::computed::ComputedLayout;
use crate::node::{LayoutNode, LayoutNodeBuilder, LayoutStyle};

/// A key identifying a node in the layout tree.
pub type LayoutNodeKey = DefaultKey;

/// Result type for layout operations.
pub type LayoutResult<T> = Result<T, LayoutError>;

/// Errors that can occur during layout operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// The specified node was not found in the tree.
    NodeNotFound(LayoutNodeKey),
    /// Failed to create a node in the underlying taffy tree.
    TaffyError(String),
    /// An invalid operation was attempted.
    InvalidOperation(String),
    /// Circular reference detected in the tree structure.
    CircularReference,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NodeNotFound(key) => write!(f, "node not found: {key:?}"),
            Self::TaffyError(msg) => write!(f, "taffy error: {msg}"),
            Self::InvalidOperation(msg) => write!(f, "invalid operation: {msg}"),
            Self::CircularReference => write!(f, "circular reference detected"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// A tree of layout nodes using taffy for flexbox calculations.
///
/// `LayoutTree` manages the relationship between nodes and their layout styles,
/// coordinating with taffy to compute the actual layout positions and sizes.
pub struct LayoutTree {
    /// The underlying taffy tree for layout calculations.
    taffy: TaffyTree<()>,
    /// Storage for layout nodes.
    nodes: SlotMap<LayoutNodeKey, LayoutNode>,
    /// The root node of the tree (if set).
    root: Option<LayoutNodeKey>,
    /// Whether the layout needs to be recalculated.
    dirty: bool,
    /// Cached available size from last computation.
    last_available_width: f32,
    last_available_height: f32,
}

impl Default for LayoutTree {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutTree {
    /// Creates a new empty layout tree.
    #[must_use]
    pub fn new() -> Self {
        Self {
            taffy: TaffyTree::new(),
            nodes: SlotMap::new(),
            root: None,
            dirty: true,
            last_available_width: 0.0,
            last_available_height: 0.0,
        }
    }

    /// Creates a new layout tree with the specified initial capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            taffy: TaffyTree::with_capacity(capacity),
            nodes: SlotMap::with_capacity(capacity),
            root: None,
            dirty: true,
            last_available_width: 0.0,
            last_available_height: 0.0,
        }
    }

    /// Returns the number of nodes in the tree.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns true if the tree contains no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns true if the layout needs to be recalculated.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Marks the layout as needing recalculation.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Returns the root node key, if set.
    #[must_use]
    pub fn root(&self) -> Option<LayoutNodeKey> {
        self.root
    }

    /// Sets the root node of the tree.
    pub fn set_root(&mut self, key: LayoutNodeKey) -> LayoutResult<()> {
        if !self.nodes.contains_key(key) {
            return Err(LayoutError::NodeNotFound(key));
        }
        self.root = Some(key);
        self.dirty = true;
        Ok(())
    }

    /// Clears the root node.
    pub fn clear_root(&mut self) {
        self.root = None;
    }

    /// Creates a new node with the given style and adds it to the tree.
    pub fn create_node(&mut self, style: LayoutStyle) -> LayoutResult<LayoutNodeKey> {
        let taffy_style = style.to_taffy();
        let taffy_node = self
            .taffy
            .new_leaf(taffy_style)
            .map_err(|e| LayoutError::TaffyError(e.to_string()))?;

        let node = LayoutNode::new(taffy_node, style);
        let key = self.nodes.insert(node);
        self.dirty = true;

        Ok(key)
    }

    /// Creates a new node using a builder and adds it to the tree.
    pub fn create_node_with_builder(
        &mut self,
        builder: LayoutNodeBuilder,
    ) -> LayoutResult<LayoutNodeKey> {
        let style = builder.style().clone();
        let taffy_style = style.to_taffy();
        let taffy_node = self
            .taffy
            .new_leaf(taffy_style)
            .map_err(|e| LayoutError::TaffyError(e.to_string()))?;

        let node = builder.build_with_taffy_node(taffy_node);
        let key = self.nodes.insert(node);
        self.dirty = true;

        Ok(key)
    }

    /// Creates a new node with default style.
    pub fn create_default_node(&mut self) -> LayoutResult<LayoutNodeKey> {
        self.create_node(LayoutStyle::default())
    }

    /// Removes a node and all its descendants from the tree.
    pub fn remove_node(&mut self, key: LayoutNodeKey) -> LayoutResult<()> {
        // First, collect all descendants to remove
        let mut to_remove = vec![key];
        let mut i = 0;
        while i < to_remove.len() {
            let current = to_remove[i];
            if let Some(node) = self.nodes.get(current) {
                to_remove.extend(node.children.clone());
            }
            i += 1;
        }

        // Remove the node from its parent's children list
        if let Some(node) = self.nodes.get(key) {
            if let Some(parent_key) = node.parent {
                if let Some(parent) = self.nodes.get_mut(parent_key) {
                    parent.children.retain(|&k| k != key);
                }

                // Remove from taffy parent
                let node = self.nodes.get(key).unwrap();
                if let Some(parent) = self.nodes.get(parent_key) {
                    let _ = self.taffy.remove_child(parent.taffy_node, node.taffy_node);
                }
            }
        }

        // Remove all nodes in reverse order (children first)
        for key_to_remove in to_remove.into_iter().rev() {
            if let Some(node) = self.nodes.remove(key_to_remove) {
                let _ = self.taffy.remove(node.taffy_node);
            }
        }

        // Clear root if it was removed
        if self.root == Some(key) {
            self.root = None;
        }

        self.dirty = true;
        Ok(())
    }

    /// Adds a child to a parent node.
    pub fn add_child(
        &mut self,
        parent_key: LayoutNodeKey,
        child_key: LayoutNodeKey,
    ) -> LayoutResult<()> {
        // Validate both nodes exist
        if !self.nodes.contains_key(parent_key) {
            return Err(LayoutError::NodeNotFound(parent_key));
        }
        if !self.nodes.contains_key(child_key) {
            return Err(LayoutError::NodeNotFound(child_key));
        }

        // Check for circular reference
        if self.is_ancestor(child_key, parent_key) {
            return Err(LayoutError::CircularReference);
        }

        // Remove child from its current parent (if any)
        if let Some(old_parent_key) = self.nodes.get(child_key).and_then(|n| n.parent) {
            if let Some(old_parent) = self.nodes.get_mut(old_parent_key) {
                old_parent.children.retain(|&k| k != child_key);
            }

            // Remove from taffy old parent
            let (old_parent_taffy, child_taffy) = {
                let old_parent = self.nodes.get(old_parent_key).unwrap();
                let child = self.nodes.get(child_key).unwrap();
                (old_parent.taffy_node, child.taffy_node)
            };
            let _ = self.taffy.remove_child(old_parent_taffy, child_taffy);
        }

        // Get taffy node IDs
        let (parent_taffy, child_taffy) = {
            let parent = self.nodes.get(parent_key).unwrap();
            let child = self.nodes.get(child_key).unwrap();
            (parent.taffy_node, child.taffy_node)
        };

        // Add to taffy tree
        self.taffy
            .add_child(parent_taffy, child_taffy)
            .map_err(|e| LayoutError::TaffyError(e.to_string()))?;

        // Update our node structures
        if let Some(parent) = self.nodes.get_mut(parent_key) {
            parent.children.push(child_key);
        }
        if let Some(child) = self.nodes.get_mut(child_key) {
            child.parent = Some(parent_key);
        }

        self.dirty = true;
        Ok(())
    }

    /// Inserts a child at a specific index.
    pub fn insert_child(
        &mut self,
        parent_key: LayoutNodeKey,
        child_key: LayoutNodeKey,
        index: usize,
    ) -> LayoutResult<()> {
        // Validate both nodes exist
        if !self.nodes.contains_key(parent_key) {
            return Err(LayoutError::NodeNotFound(parent_key));
        }
        if !self.nodes.contains_key(child_key) {
            return Err(LayoutError::NodeNotFound(child_key));
        }

        // Check for circular reference
        if self.is_ancestor(child_key, parent_key) {
            return Err(LayoutError::CircularReference);
        }

        // Remove child from its current parent (if any)
        if let Some(old_parent_key) = self.nodes.get(child_key).and_then(|n| n.parent) {
            if let Some(old_parent) = self.nodes.get_mut(old_parent_key) {
                old_parent.children.retain(|&k| k != child_key);
            }

            let (old_parent_taffy, child_taffy) = {
                let old_parent = self.nodes.get(old_parent_key).unwrap();
                let child = self.nodes.get(child_key).unwrap();
                (old_parent.taffy_node, child.taffy_node)
            };
            let _ = self.taffy.remove_child(old_parent_taffy, child_taffy);
        }

        // Get taffy node IDs
        let (parent_taffy, child_taffy) = {
            let parent = self.nodes.get(parent_key).unwrap();
            let child = self.nodes.get(child_key).unwrap();
            (parent.taffy_node, child.taffy_node)
        };

        // Insert in taffy tree
        self.taffy
            .insert_child_at_index(parent_taffy, index, child_taffy)
            .map_err(|e| LayoutError::TaffyError(e.to_string()))?;

        // Update our node structures
        if let Some(parent) = self.nodes.get_mut(parent_key) {
            let insert_index = index.min(parent.children.len());
            parent.children.insert(insert_index, child_key);
        }
        if let Some(child) = self.nodes.get_mut(child_key) {
            child.parent = Some(parent_key);
        }

        self.dirty = true;
        Ok(())
    }

    /// Removes a child from its parent but keeps it in the tree.
    pub fn remove_child(
        &mut self,
        parent_key: LayoutNodeKey,
        child_key: LayoutNodeKey,
    ) -> LayoutResult<()> {
        if !self.nodes.contains_key(parent_key) {
            return Err(LayoutError::NodeNotFound(parent_key));
        }
        if !self.nodes.contains_key(child_key) {
            return Err(LayoutError::NodeNotFound(child_key));
        }

        // Get taffy nodes
        let (parent_taffy, child_taffy) = {
            let parent = self.nodes.get(parent_key).unwrap();
            let child = self.nodes.get(child_key).unwrap();
            (parent.taffy_node, child.taffy_node)
        };

        // Remove from taffy
        self.taffy
            .remove_child(parent_taffy, child_taffy)
            .map_err(|e| LayoutError::TaffyError(e.to_string()))?;

        // Update our structures
        if let Some(parent) = self.nodes.get_mut(parent_key) {
            parent.children.retain(|&k| k != child_key);
        }
        if let Some(child) = self.nodes.get_mut(child_key) {
            child.parent = None;
        }

        self.dirty = true;
        Ok(())
    }

    /// Replaces a node's children with a new list.
    pub fn set_children(
        &mut self,
        parent_key: LayoutNodeKey,
        children: &[LayoutNodeKey],
    ) -> LayoutResult<()> {
        if !self.nodes.contains_key(parent_key) {
            return Err(LayoutError::NodeNotFound(parent_key));
        }

        // Validate all children exist
        for &child_key in children {
            if !self.nodes.contains_key(child_key) {
                return Err(LayoutError::NodeNotFound(child_key));
            }
            // Check for circular reference
            if self.is_ancestor(child_key, parent_key) {
                return Err(LayoutError::CircularReference);
            }
        }

        // Clear old parent references
        let old_children = self
            .nodes
            .get(parent_key)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        for old_child_key in old_children {
            if let Some(old_child) = self.nodes.get_mut(old_child_key) {
                old_child.parent = None;
            }
        }

        // Get taffy node IDs for new children
        let parent_taffy = self.nodes.get(parent_key).unwrap().taffy_node;
        let child_taffy_ids: Vec<TaffyNodeId> = children
            .iter()
            .filter_map(|&k| self.nodes.get(k).map(|n| n.taffy_node))
            .collect();

        // Update taffy tree
        self.taffy
            .set_children(parent_taffy, &child_taffy_ids)
            .map_err(|e| LayoutError::TaffyError(e.to_string()))?;

        // Update our structures
        if let Some(parent) = self.nodes.get_mut(parent_key) {
            parent.children = children.to_vec();
        }
        for &child_key in children {
            if let Some(child) = self.nodes.get_mut(child_key) {
                child.parent = Some(parent_key);
            }
        }

        self.dirty = true;
        Ok(())
    }

    /// Gets a reference to a node.
    #[must_use]
    pub fn get_node(&self, key: LayoutNodeKey) -> Option<&LayoutNode> {
        self.nodes.get(key)
    }

    /// Gets a mutable reference to a node.
    #[must_use]
    pub fn get_node_mut(&mut self, key: LayoutNodeKey) -> Option<&mut LayoutNode> {
        self.dirty = true;
        self.nodes.get_mut(key)
    }

    /// Updates the style of a node.
    pub fn set_style(&mut self, key: LayoutNodeKey, style: LayoutStyle) -> LayoutResult<()> {
        let node = self
            .nodes
            .get_mut(key)
            .ok_or(LayoutError::NodeNotFound(key))?;

        let taffy_style = style.to_taffy();
        self.taffy
            .set_style(node.taffy_node, taffy_style)
            .map_err(|e| LayoutError::TaffyError(e.to_string()))?;

        node.style = style;
        self.dirty = true;

        Ok(())
    }

    /// Returns the computed layout for a node.
    #[must_use]
    pub fn get_computed_layout(&self, key: LayoutNodeKey) -> Option<ComputedLayout> {
        self.nodes.get(key).map(|n| n.computed)
    }

    /// Computes the layout for the entire tree.
    ///
    /// This method runs the taffy layout algorithm on the root node
    /// and updates all computed layouts.
    pub fn compute_layout(&mut self, available_width: f32, available_height: f32) {
        // Only recompute if dirty or size changed
        let size_changed = (self.last_available_width - available_width).abs() > f32::EPSILON
            || (self.last_available_height - available_height).abs() > f32::EPSILON;

        if !self.dirty && !size_changed {
            return;
        }

        self.last_available_width = available_width;
        self.last_available_height = available_height;

        let Some(root_key) = self.root else {
            return;
        };

        let Some(root_node) = self.nodes.get(root_key) else {
            return;
        };

        let root_taffy = root_node.taffy_node;

        // Run taffy layout
        let available_space = taffy::Size {
            width: AvailableSpace::Definite(available_width),
            height: AvailableSpace::Definite(available_height),
        };

        if self
            .taffy
            .compute_layout(root_taffy, available_space)
            .is_ok()
        {
            // Update all computed layouts
            self.update_computed_layouts(root_key);
        }

        self.dirty = false;
    }

    /// Updates computed layouts by traversing the tree.
    fn update_computed_layouts(&mut self, key: LayoutNodeKey) {
        let Some(node) = self.nodes.get(key) else {
            return;
        };

        let taffy_node = node.taffy_node;
        let children = node.children.clone();

        // Get layout from taffy
        if let Ok(layout) = self.taffy.layout(taffy_node) {
            let computed = ComputedLayout::from_taffy(layout);
            if let Some(node) = self.nodes.get_mut(key) {
                node.computed = computed;
            }
        }

        // Recursively update children
        for child_key in children {
            self.update_computed_layouts(child_key);
        }
    }

    /// Gets the world-space position for a node by walking up the parent chain.
    #[must_use]
    pub fn get_world_position(&self, key: LayoutNodeKey) -> Option<(f32, f32)> {
        let node = self.nodes.get(key)?;

        let mut world_x = node.computed.x + node.translate_x;
        let mut world_y = node.computed.y + node.translate_y;

        let mut current_parent = node.parent;
        while let Some(parent_key) = current_parent {
            if let Some(parent) = self.nodes.get(parent_key) {
                world_x += parent.computed.x + parent.translate_x;
                world_y += parent.computed.y + parent.translate_y;
                current_parent = parent.parent;
            } else {
                break;
            }
        }

        Some((world_x, world_y))
    }

    /// Returns all children of a node in layout order.
    #[must_use]
    pub fn get_children(&self, key: LayoutNodeKey) -> Option<&[LayoutNodeKey]> {
        self.nodes.get(key).map(|n| n.children.as_slice())
    }

    /// Returns the parent of a node.
    #[must_use]
    pub fn get_parent(&self, key: LayoutNodeKey) -> Option<LayoutNodeKey> {
        self.nodes.get(key).and_then(|n| n.parent)
    }

    /// Returns all nodes that are descendants of the given node.
    #[must_use]
    pub fn get_descendants(&self, key: LayoutNodeKey) -> Vec<LayoutNodeKey> {
        let mut descendants = Vec::new();
        self.collect_descendants(key, &mut descendants);
        descendants
    }

    /// Helper to collect descendants recursively.
    fn collect_descendants(&self, key: LayoutNodeKey, result: &mut Vec<LayoutNodeKey>) {
        if let Some(node) = self.nodes.get(key) {
            for &child_key in &node.children {
                result.push(child_key);
                self.collect_descendants(child_key, result);
            }
        }
    }

    /// Checks if `ancestor` is an ancestor of `descendant`.
    fn is_ancestor(&self, ancestor: LayoutNodeKey, descendant: LayoutNodeKey) -> bool {
        let mut current = Some(descendant);
        while let Some(key) = current {
            if key == ancestor {
                return true;
            }
            current = self.nodes.get(key).and_then(|n| n.parent);
        }
        false
    }

    /// Iterates over all nodes in the tree.
    pub fn iter(&self) -> impl Iterator<Item = (LayoutNodeKey, &LayoutNode)> {
        self.nodes.iter()
    }

    /// Iterates mutably over all nodes in the tree.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (LayoutNodeKey, &mut LayoutNode)> {
        self.dirty = true;
        self.nodes.iter_mut()
    }

    /// Returns nodes sorted by z-index for rendering.
    #[must_use]
    pub fn nodes_by_z_index(&self, key: LayoutNodeKey) -> Vec<LayoutNodeKey> {
        let Some(node) = self.nodes.get(key) else {
            return Vec::new();
        };

        let mut sorted_children: Vec<_> = node.children.clone();
        sorted_children
            .sort_by_key(|&child_key| self.nodes.get(child_key).map_or(0, |n| n.z_index));

        sorted_children
    }

    /// Clears all nodes from the tree.
    pub fn clear(&mut self) {
        // Remove all nodes from taffy
        for (_, node) in self.nodes.drain() {
            let _ = self.taffy.remove(node.taffy_node);
        }

        self.root = None;
        self.dirty = true;
    }

    /// Finds a node by its tag.
    #[must_use]
    pub fn find_by_tag(&self, tag: &str) -> Option<LayoutNodeKey> {
        self.nodes
            .iter()
            .find(|(_, node)| node.tag.as_deref() == Some(tag))
            .map(|(key, _)| key)
    }

    /// Finds all nodes with the given tag.
    #[must_use]
    pub fn find_all_by_tag(&self, tag: &str) -> Vec<LayoutNodeKey> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.tag.as_deref() == Some(tag))
            .map(|(key, _)| key)
            .collect()
    }

    /// Performs hit testing to find the topmost node at a given point.
    #[must_use]
    pub fn hit_test(&self, x: f32, y: f32) -> Option<LayoutNodeKey> {
        let root = self.root?;
        self.hit_test_recursive(root, x, y, 0.0, 0.0)
    }

    /// Recursive hit testing helper.
    fn hit_test_recursive(
        &self,
        key: LayoutNodeKey,
        x: f32,
        y: f32,
        parent_x: f32,
        parent_y: f32,
    ) -> Option<LayoutNodeKey> {
        let node = self.nodes.get(key)?;

        if !node.visible {
            return None;
        }

        let world_x = parent_x + node.computed.x + node.translate_x;
        let world_y = parent_y + node.computed.y + node.translate_y;

        // Check if point is within bounds
        let in_bounds = x >= world_x
            && x < world_x + node.computed.width
            && y >= world_y
            && y < world_y + node.computed.height;

        if !in_bounds {
            return None;
        }

        // Check children in reverse z-order (topmost first)
        let mut sorted_children = self.nodes_by_z_index(key);
        sorted_children.reverse();

        for child_key in sorted_children {
            if let Some(hit) = self.hit_test_recursive(child_key, x, y, world_x, world_y) {
                return Some(hit);
            }
        }

        // No child hit, return this node
        Some(key)
    }

    /// Marks a specific node's subtree as dirty.
    pub fn mark_node_dirty(&mut self, key: LayoutNodeKey) -> LayoutResult<()> {
        let node = self.nodes.get(key).ok_or(LayoutError::NodeNotFound(key))?;
        self.taffy
            .mark_dirty(node.taffy_node)
            .map_err(|e| LayoutError::TaffyError(e.to_string()))?;
        self.dirty = true;
        Ok(())
    }
}

impl std::fmt::Debug for LayoutTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutTree")
            .field("node_count", &self.nodes.len())
            .field("root", &self.root)
            .field("dirty", &self.dirty)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{Dimension, FlexDirection};

    fn create_test_tree() -> LayoutTree {
        LayoutTree::new()
    }

    #[test]
    fn test_create_node() {
        let mut tree = create_test_tree();
        let key = tree.create_default_node().unwrap();
        assert!(tree.get_node(key).is_some());
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_add_child() {
        let mut tree = create_test_tree();
        let parent = tree.create_default_node().unwrap();
        let child = tree.create_default_node().unwrap();

        tree.add_child(parent, child).unwrap();

        assert_eq!(tree.get_parent(child), Some(parent));
        assert_eq!(tree.get_children(parent).unwrap().len(), 1);
    }

    #[test]
    fn test_remove_node() {
        let mut tree = create_test_tree();
        let parent = tree.create_default_node().unwrap();
        let child = tree.create_default_node().unwrap();

        tree.add_child(parent, child).unwrap();
        tree.remove_node(parent).unwrap();

        assert!(tree.is_empty());
    }

    #[test]
    fn test_circular_reference_detection() {
        let mut tree = create_test_tree();
        let node1 = tree.create_default_node().unwrap();
        let node2 = tree.create_default_node().unwrap();
        let node3 = tree.create_default_node().unwrap();

        tree.add_child(node1, node2).unwrap();
        tree.add_child(node2, node3).unwrap();

        // Trying to add node1 as child of node3 should fail
        let result = tree.add_child(node3, node1);
        assert!(matches!(result, Err(LayoutError::CircularReference)));
    }

    #[test]
    fn test_compute_layout() {
        let mut tree = create_test_tree();

        // Create a simple layout with explicit sizes
        let mut root_style = LayoutStyle::default();
        root_style.size.width = Dimension::Points(100.0);
        root_style.size.height = Dimension::Points(100.0);
        root_style.flex_direction = FlexDirection::Column;

        let root = tree.create_node(root_style).unwrap();
        tree.set_root(root).unwrap();

        // Child with explicit width and height
        let mut child_style = LayoutStyle::default();
        child_style.size.width = Dimension::Points(80.0);
        child_style.size.height = Dimension::Points(50.0);

        let child = tree.create_node(child_style).unwrap();
        tree.add_child(root, child).unwrap();

        // Compute layout
        tree.compute_layout(100.0, 100.0);

        let root_layout = tree.get_computed_layout(root).unwrap();
        assert_eq!(root_layout.width, 100.0);
        assert_eq!(root_layout.height, 100.0);

        let child_layout = tree.get_computed_layout(child).unwrap();
        assert_eq!(child_layout.width, 80.0);
        assert_eq!(child_layout.height, 50.0);
    }

    #[test]
    fn test_hit_testing() {
        let mut tree = create_test_tree();

        let mut root_style = LayoutStyle::default();
        root_style.size.width = Dimension::Points(100.0);
        root_style.size.height = Dimension::Points(100.0);

        let root = tree.create_node(root_style).unwrap();
        tree.set_root(root).unwrap();

        tree.compute_layout(100.0, 100.0);

        // Hit inside
        assert_eq!(tree.hit_test(50.0, 50.0), Some(root));

        // Hit outside
        assert_eq!(tree.hit_test(150.0, 50.0), None);
    }

    #[test]
    fn test_find_by_tag() {
        let mut tree = create_test_tree();

        let builder = LayoutNodeBuilder::new().tag("my-node");
        let key = tree.create_node_with_builder(builder).unwrap();

        assert_eq!(tree.find_by_tag("my-node"), Some(key));
        assert_eq!(tree.find_by_tag("other-tag"), None);
    }

    #[test]
    fn test_get_world_position() {
        let mut tree = create_test_tree();

        let mut parent_style = LayoutStyle::default();
        parent_style.size.width = Dimension::Points(100.0);
        parent_style.size.height = Dimension::Points(100.0);
        parent_style.padding =
            crate::style::Edges::all(crate::style::LengthPercentage::Points(10.0));

        let parent = tree.create_node(parent_style).unwrap();
        tree.set_root(parent).unwrap();

        let mut child_style = LayoutStyle::default();
        child_style.size.width = Dimension::Points(50.0);
        child_style.size.height = Dimension::Points(50.0);

        let child = tree.create_node(child_style).unwrap();
        tree.add_child(parent, child).unwrap();

        tree.compute_layout(100.0, 100.0);

        let (world_x, world_y) = tree.get_world_position(child).unwrap();
        // Child should be at parent's padding offset
        assert!(world_x >= 10.0);
        assert!(world_y >= 10.0);
    }
}
