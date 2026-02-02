//! Click zone management for mouse interaction
//!
//! This module provides a registry system for managing clickable regions in the TUI.
//! Click zones are registered during each render pass and used during mouse event
//! handling to determine which UI element was clicked.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                         Render Pass                                  │
//! │  1. registry.clear()                                                │
//! │  2. Widgets register their bounds:                                  │
//! │     - registry.register(ClickZoneId::Sidebar, sidebar_rect)        │
//! │     - registry.register(ClickZoneId::ChatArea, chat_rect)          │
//! │     - registry.register_sessions(...)                               │
//! └─────────────────────────────────────────────────────────────────────┘
//!                                    │
//!                                    ▼
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                         Mouse Event                                  │
//! │  1. Get click position (x, y)                                       │
//! │  2. zone_id = registry.find(x, y)                                   │
//! │  3. Handle click based on zone_id                                   │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::input::{ClickZoneRegistry, ClickZoneId};
//! use ratatui::layout::Rect;
//!
//! let mut registry = ClickZoneRegistry::new();
//!
//! // During render
//! registry.clear();
//! registry.register(ClickZoneId::Sidebar, Rect::new(0, 0, 30, 24));
//! registry.register(ClickZoneId::ChatArea, Rect::new(30, 0, 50, 20));
//! registry.register(ClickZoneId::InputField, Rect::new(30, 20, 50, 4));
//!
//! // Register multiple session items at once
//! registry.register_sessions(Rect::new(0, 2, 30, 20), 5, 4);
//!
//! // During mouse handling
//! if let Some(zone) = registry.find(10, 5) {
//!     match zone {
//!         ClickZoneId::SessionItem(idx) => {
//!             // Load session at index
//!         }
//!         _ => {}
//!     }
//! }
//! ```

use ratatui::layout::Rect;

// ============================================================================
// CLICK ZONE ID
// ============================================================================

/// Unique identifier for a click zone.
///
/// These IDs represent the semantic meaning of UI elements,
/// allowing click handlers to respond appropriately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClickZoneId {
    /// Sidebar area (general)
    Sidebar,

    /// Specific session in sidebar (index in visible list)
    SessionItem(usize),

    /// New session button in sidebar
    NewSessionButton,

    /// Chat/message area (general)
    ChatArea,

    /// Specific message in chat (index)
    MessageItem(usize),

    /// Input text field
    InputField,

    /// Header area
    Header,

    /// Model selector in header
    ModelSelector,

    /// Provider selector in header
    ProviderSelector,

    /// Status bar at bottom
    StatusBar,

    /// Modal overlay (blocks clicks to content behind)
    Modal,

    /// Scroll up button/area
    ScrollUp,

    /// Scroll down button/area
    ScrollDown,

    /// Context menu item (index)
    ContextMenuItem(usize),

    /// Approval dialog approve button
    ApproveButton,

    /// Approval dialog reject button
    RejectButton,

    /// Diff view area
    DiffView,

    /// Tool output area
    ToolOutput,

    /// Scrollbar track area (for click and drag scrolling)
    Scrollbar,

    /// Custom zone with numeric ID for extensibility
    Custom(u32),
}

impl ClickZoneId {
    /// Returns true if this is a session-related zone.
    pub fn is_session_zone(&self) -> bool {
        matches!(
            self,
            ClickZoneId::Sidebar | ClickZoneId::SessionItem(_) | ClickZoneId::NewSessionButton
        )
    }

    /// Returns true if this is a chat-related zone.
    pub fn is_chat_zone(&self) -> bool {
        matches!(self, ClickZoneId::ChatArea | ClickZoneId::MessageItem(_))
    }

    /// Returns true if this is a modal/overlay zone.
    pub fn is_modal_zone(&self) -> bool {
        matches!(
            self,
            ClickZoneId::Modal
                | ClickZoneId::ApproveButton
                | ClickZoneId::RejectButton
                | ClickZoneId::ContextMenuItem(_)
        )
    }

    /// Returns the session index if this is a SessionItem zone.
    pub fn session_index(&self) -> Option<usize> {
        match self {
            ClickZoneId::SessionItem(idx) => Some(*idx),
            _ => None,
        }
    }

    /// Returns the message index if this is a MessageItem zone.
    pub fn message_index(&self) -> Option<usize> {
        match self {
            ClickZoneId::MessageItem(idx) => Some(*idx),
            _ => None,
        }
    }
}

impl std::fmt::Display for ClickZoneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClickZoneId::Sidebar => write!(f, "Sidebar"),
            ClickZoneId::SessionItem(idx) => write!(f, "SessionItem({})", idx),
            ClickZoneId::NewSessionButton => write!(f, "NewSessionButton"),
            ClickZoneId::ChatArea => write!(f, "ChatArea"),
            ClickZoneId::MessageItem(idx) => write!(f, "MessageItem({})", idx),
            ClickZoneId::InputField => write!(f, "InputField"),
            ClickZoneId::Header => write!(f, "Header"),
            ClickZoneId::ModelSelector => write!(f, "ModelSelector"),
            ClickZoneId::ProviderSelector => write!(f, "ProviderSelector"),
            ClickZoneId::StatusBar => write!(f, "StatusBar"),
            ClickZoneId::Modal => write!(f, "Modal"),
            ClickZoneId::ScrollUp => write!(f, "ScrollUp"),
            ClickZoneId::ScrollDown => write!(f, "ScrollDown"),
            ClickZoneId::ContextMenuItem(idx) => write!(f, "ContextMenuItem({})", idx),
            ClickZoneId::ApproveButton => write!(f, "ApproveButton"),
            ClickZoneId::RejectButton => write!(f, "RejectButton"),
            ClickZoneId::DiffView => write!(f, "DiffView"),
            ClickZoneId::ToolOutput => write!(f, "ToolOutput"),
            ClickZoneId::Scrollbar => write!(f, "Scrollbar"),
            ClickZoneId::Custom(id) => write!(f, "Custom({})", id),
        }
    }
}

// ============================================================================
// CLICK ZONE
// ============================================================================

/// A clickable zone with bounds and state.
#[derive(Debug, Clone)]
pub struct ClickZone {
    /// Identifier for this zone
    pub id: ClickZoneId,
    /// Bounding rectangle
    pub rect: Rect,
    /// Whether this zone is currently enabled for clicks
    pub enabled: bool,
    /// Z-order priority (higher = on top)
    pub z_order: u8,
}

impl ClickZone {
    /// Creates a new click zone.
    ///
    /// # Arguments
    ///
    /// * `id` - Identifier for the zone
    /// * `rect` - Bounding rectangle
    pub fn new(id: ClickZoneId, rect: Rect) -> Self {
        Self {
            id,
            rect,
            enabled: true,
            z_order: 0,
        }
    }

    /// Creates a new click zone with a specific z-order.
    ///
    /// Higher z-order zones are checked first when finding clicks.
    ///
    /// # Arguments
    ///
    /// * `id` - Identifier for the zone
    /// * `rect` - Bounding rectangle
    /// * `z_order` - Z-order priority (0-255)
    pub fn with_z_order(id: ClickZoneId, rect: Rect, z_order: u8) -> Self {
        Self {
            id,
            rect,
            enabled: true,
            z_order,
        }
    }

    /// Sets whether this zone is enabled.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Returns true if the given point is within this zone's bounds.
    ///
    /// # Arguments
    ///
    /// * `x` - X coordinate (column)
    /// * `y` - Y coordinate (row)
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.enabled
            && x >= self.rect.x
            && x < self.rect.x.saturating_add(self.rect.width)
            && y >= self.rect.y
            && y < self.rect.y.saturating_add(self.rect.height)
    }

    /// Returns the area of this zone in cells.
    pub fn area(&self) -> u32 {
        self.rect.width as u32 * self.rect.height as u32
    }
}

// ============================================================================
// CLICK ZONE REGISTRY
// ============================================================================

/// Registry of all click zones, updated each frame.
///
/// The registry maintains a list of clickable zones that should be cleared
/// and repopulated during each render pass. This ensures click zones always
/// match the current UI state.
///
/// # Usage Pattern
///
/// 1. Call `clear()` at the start of each render
/// 2. Register zones as widgets are rendered
/// 3. Use `find()` during mouse event handling
///
/// # Z-Order
///
/// When multiple zones overlap, the zone with the highest z-order is returned.
/// Modal overlays should use high z-order values to capture clicks.
pub struct ClickZoneRegistry {
    /// Registered zones
    zones: Vec<ClickZone>,
    /// Whether the registry needs sorting by z-order
    needs_sort: bool,
}

impl ClickZoneRegistry {
    /// Creates a new empty registry.
    pub fn new() -> Self {
        Self {
            zones: Vec::with_capacity(64), // Pre-allocate for typical UI
            needs_sort: false,
        }
    }

    /// Clears all zones (call at start of each render).
    ///
    /// This should be called before rendering to ensure zones
    /// reflect the current frame's layout.
    pub fn clear(&mut self) {
        self.zones.clear();
        self.needs_sort = false;
    }

    /// Registers a click zone.
    ///
    /// # Arguments
    ///
    /// * `id` - Identifier for the zone
    /// * `rect` - Bounding rectangle
    pub fn register(&mut self, id: ClickZoneId, rect: Rect) {
        // Don't register empty zones
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.zones.push(ClickZone::new(id, rect));
    }

    /// Registers a click zone with z-order.
    ///
    /// Higher z-order zones take priority when overlapping.
    ///
    /// # Arguments
    ///
    /// * `id` - Identifier for the zone
    /// * `rect` - Bounding rectangle
    /// * `z_order` - Z-order priority (0-255)
    pub fn register_with_z_order(&mut self, id: ClickZoneId, rect: Rect, z_order: u8) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.zones.push(ClickZone::with_z_order(id, rect, z_order));
        if z_order > 0 {
            self.needs_sort = true;
        }
    }

    /// Finds the topmost zone at the given position.
    ///
    /// Returns the zone with the highest z-order that contains the point,
    /// or `None` if no zone contains the point.
    ///
    /// # Arguments
    ///
    /// * `x` - X coordinate (column)
    /// * `y` - Y coordinate (row)
    pub fn find(&mut self, x: u16, y: u16) -> Option<ClickZoneId> {
        // Sort by z-order if needed (descending, highest first)
        if self.needs_sort {
            self.zones.sort_by(|a, b| b.z_order.cmp(&a.z_order));
            self.needs_sort = false;
        }

        // Find first (highest z-order) zone containing the point
        self.zones.iter().find(|z| z.contains(x, y)).map(|z| z.id)
    }

    /// Finds all zones at the given position, sorted by z-order (highest first).
    ///
    /// Useful for debugging or when you need to know all zones under a point.
    ///
    /// # Arguments
    ///
    /// * `x` - X coordinate (column)
    /// * `y` - Y coordinate (row)
    pub fn find_all(&mut self, x: u16, y: u16) -> Vec<ClickZoneId> {
        if self.needs_sort {
            self.zones.sort_by(|a, b| b.z_order.cmp(&a.z_order));
            self.needs_sort = false;
        }

        self.zones
            .iter()
            .filter(|z| z.contains(x, y))
            .map(|z| z.id)
            .collect()
    }

    /// Gets a zone by ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The zone ID to find
    pub fn get(&self, id: ClickZoneId) -> Option<&ClickZone> {
        self.zones.iter().find(|z| z.id == id)
    }

    /// Gets the rectangle for a zone by ID.
    ///
    /// This is a convenience method that returns just the Rect.
    ///
    /// # Arguments
    ///
    /// * `id` - The zone ID to find
    pub fn get_zone_rect(&self, id: ClickZoneId) -> Option<Rect> {
        self.get(id).map(|z| z.rect)
    }

    /// Gets a mutable reference to a zone by ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The zone ID to find
    pub fn get_mut(&mut self, id: ClickZoneId) -> Option<&mut ClickZone> {
        self.zones.iter_mut().find(|z| z.id == id)
    }

    /// Registers multiple session items in a sidebar area.
    ///
    /// This is a convenience method for registering a list of session items
    /// with consistent height and positioning.
    ///
    /// # Arguments
    ///
    /// * `base_rect` - The area containing all sessions
    /// * `count` - Number of sessions to register
    /// * `item_height` - Height of each session item in rows
    pub fn register_sessions(&mut self, base_rect: Rect, count: usize, item_height: u16) {
        for i in 0..count {
            let y = base_rect.y.saturating_add(i as u16 * item_height);
            if y.saturating_add(item_height) <= base_rect.y.saturating_add(base_rect.height) {
                let rect = Rect::new(base_rect.x, y, base_rect.width, item_height);
                self.register(ClickZoneId::SessionItem(i), rect);
            }
        }
    }

    /// Registers multiple message items with their individual rects.
    ///
    /// # Arguments
    ///
    /// * `rects` - Slice of (message_index, rect) pairs
    pub fn register_messages(&mut self, rects: &[(usize, Rect)]) {
        for (idx, rect) in rects {
            self.register(ClickZoneId::MessageItem(*idx), *rect);
        }
    }

    /// Registers a modal overlay that blocks clicks to content behind it.
    ///
    /// The modal is registered with high z-order to ensure it captures clicks.
    ///
    /// # Arguments
    ///
    /// * `rect` - The modal's bounding rectangle
    pub fn register_modal(&mut self, rect: Rect) {
        self.register_with_z_order(ClickZoneId::Modal, rect, 100);
    }

    /// Returns the number of registered zones.
    pub fn len(&self) -> usize {
        self.zones.len()
    }

    /// Returns true if no zones are registered.
    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }

    /// Returns an iterator over all registered zones.
    pub fn iter(&self) -> impl Iterator<Item = &ClickZone> {
        self.zones.iter()
    }

    /// Enables or disables a zone by ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The zone ID to modify
    /// * `enabled` - Whether the zone should be enabled
    pub fn set_enabled(&mut self, id: ClickZoneId, enabled: bool) {
        if let Some(zone) = self.get_mut(id) {
            zone.set_enabled(enabled);
        }
    }
}

impl Default for ClickZoneRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ClickZoneRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClickZoneRegistry")
            .field("zone_count", &self.zones.len())
            .field("needs_sort", &self.needs_sort)
            .finish()
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_click_zone_id_display() {
        assert_eq!(format!("{}", ClickZoneId::Sidebar), "Sidebar");
        assert_eq!(format!("{}", ClickZoneId::SessionItem(5)), "SessionItem(5)");
        assert_eq!(format!("{}", ClickZoneId::Custom(42)), "Custom(42)");
    }

    #[test]
    fn test_click_zone_id_is_methods() {
        assert!(ClickZoneId::Sidebar.is_session_zone());
        assert!(ClickZoneId::SessionItem(0).is_session_zone());
        assert!(ClickZoneId::NewSessionButton.is_session_zone());
        assert!(!ClickZoneId::ChatArea.is_session_zone());

        assert!(ClickZoneId::ChatArea.is_chat_zone());
        assert!(ClickZoneId::MessageItem(0).is_chat_zone());
        assert!(!ClickZoneId::Sidebar.is_chat_zone());

        assert!(ClickZoneId::Modal.is_modal_zone());
        assert!(ClickZoneId::ApproveButton.is_modal_zone());
        assert!(!ClickZoneId::Sidebar.is_modal_zone());
    }

    #[test]
    fn test_click_zone_id_index_methods() {
        assert_eq!(ClickZoneId::SessionItem(5).session_index(), Some(5));
        assert_eq!(ClickZoneId::Sidebar.session_index(), None);

        assert_eq!(ClickZoneId::MessageItem(3).message_index(), Some(3));
        assert_eq!(ClickZoneId::ChatArea.message_index(), None);
    }

    #[test]
    fn test_click_zone_new() {
        let zone = ClickZone::new(ClickZoneId::Sidebar, Rect::new(0, 0, 30, 24));

        assert_eq!(zone.id, ClickZoneId::Sidebar);
        assert_eq!(zone.rect, Rect::new(0, 0, 30, 24));
        assert!(zone.enabled);
        assert_eq!(zone.z_order, 0);
    }

    #[test]
    fn test_click_zone_contains() {
        let zone = ClickZone::new(ClickZoneId::Sidebar, Rect::new(10, 10, 20, 15));

        // Inside
        assert!(zone.contains(10, 10)); // Top-left corner
        assert!(zone.contains(29, 24)); // Bottom-right corner (exclusive)
        assert!(zone.contains(20, 17)); // Center

        // Outside
        assert!(!zone.contains(9, 10)); // Left of zone
        assert!(!zone.contains(30, 10)); // Right of zone
        assert!(!zone.contains(10, 9)); // Above zone
        assert!(!zone.contains(10, 25)); // Below zone
    }

    #[test]
    fn test_click_zone_disabled() {
        let mut zone = ClickZone::new(ClickZoneId::Sidebar, Rect::new(0, 0, 30, 24));

        assert!(zone.contains(15, 12));

        zone.set_enabled(false);
        assert!(!zone.contains(15, 12));
    }

    #[test]
    fn test_click_zone_area() {
        let zone = ClickZone::new(ClickZoneId::Sidebar, Rect::new(0, 0, 10, 5));
        assert_eq!(zone.area(), 50);
    }

    #[test]
    fn test_registry_new() {
        let registry = ClickZoneRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_registry_register() {
        let mut registry = ClickZoneRegistry::new();

        registry.register(ClickZoneId::Sidebar, Rect::new(0, 0, 30, 24));
        registry.register(ClickZoneId::ChatArea, Rect::new(30, 0, 50, 20));

        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_registry_register_empty_rect() {
        let mut registry = ClickZoneRegistry::new();

        // Empty rects should not be registered
        registry.register(ClickZoneId::Sidebar, Rect::new(0, 0, 0, 24));
        registry.register(ClickZoneId::ChatArea, Rect::new(0, 0, 30, 0));

        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_clear() {
        let mut registry = ClickZoneRegistry::new();

        registry.register(ClickZoneId::Sidebar, Rect::new(0, 0, 30, 24));
        assert_eq!(registry.len(), 1);

        registry.clear();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_find() {
        let mut registry = ClickZoneRegistry::new();

        registry.register(ClickZoneId::Sidebar, Rect::new(0, 0, 30, 24));
        registry.register(ClickZoneId::ChatArea, Rect::new(30, 0, 50, 24));

        assert_eq!(registry.find(15, 12), Some(ClickZoneId::Sidebar));
        assert_eq!(registry.find(50, 12), Some(ClickZoneId::ChatArea));
        assert_eq!(registry.find(100, 100), None);
    }

    #[test]
    fn test_registry_z_order() {
        let mut registry = ClickZoneRegistry::new();

        // Register overlapping zones with different z-orders
        registry.register(ClickZoneId::ChatArea, Rect::new(0, 0, 100, 100));
        registry.register_with_z_order(ClickZoneId::Modal, Rect::new(20, 20, 60, 60), 100);

        // Modal should be found due to higher z-order
        assert_eq!(registry.find(50, 50), Some(ClickZoneId::Modal));

        // Outside modal but inside chat area
        assert_eq!(registry.find(10, 10), Some(ClickZoneId::ChatArea));
    }

    #[test]
    fn test_registry_find_all() {
        let mut registry = ClickZoneRegistry::new();

        registry.register(ClickZoneId::ChatArea, Rect::new(0, 0, 100, 100));
        registry.register_with_z_order(ClickZoneId::Modal, Rect::new(20, 20, 60, 60), 100);
        registry.register_with_z_order(ClickZoneId::ApproveButton, Rect::new(30, 30, 20, 10), 101);

        let zones = registry.find_all(35, 35);

        assert_eq!(zones.len(), 3);
        // Should be sorted by z-order descending
        assert_eq!(zones[0], ClickZoneId::ApproveButton);
        assert_eq!(zones[1], ClickZoneId::Modal);
        assert_eq!(zones[2], ClickZoneId::ChatArea);
    }

    #[test]
    fn test_registry_get() {
        let mut registry = ClickZoneRegistry::new();

        registry.register(ClickZoneId::Sidebar, Rect::new(0, 0, 30, 24));

        assert!(registry.get(ClickZoneId::Sidebar).is_some());
        assert!(registry.get(ClickZoneId::ChatArea).is_none());
    }

    #[test]
    fn test_registry_register_sessions() {
        let mut registry = ClickZoneRegistry::new();

        // Register 5 sessions with height 4
        registry.register_sessions(Rect::new(0, 0, 30, 20), 5, 4);

        assert_eq!(registry.len(), 5);

        // First session
        assert_eq!(registry.find(15, 0), Some(ClickZoneId::SessionItem(0)));
        assert_eq!(registry.find(15, 3), Some(ClickZoneId::SessionItem(0)));

        // Second session
        assert_eq!(registry.find(15, 4), Some(ClickZoneId::SessionItem(1)));
        assert_eq!(registry.find(15, 7), Some(ClickZoneId::SessionItem(1)));

        // Fifth session (last one that fits)
        assert_eq!(registry.find(15, 16), Some(ClickZoneId::SessionItem(4)));
    }

    #[test]
    fn test_registry_register_messages() {
        let mut registry = ClickZoneRegistry::new();

        let message_rects = vec![
            (0, Rect::new(0, 0, 80, 5)),
            (1, Rect::new(0, 5, 80, 3)),
            (2, Rect::new(0, 8, 80, 10)),
        ];

        registry.register_messages(&message_rects);

        assert_eq!(registry.len(), 3);
        assert_eq!(registry.find(40, 2), Some(ClickZoneId::MessageItem(0)));
        assert_eq!(registry.find(40, 6), Some(ClickZoneId::MessageItem(1)));
        assert_eq!(registry.find(40, 12), Some(ClickZoneId::MessageItem(2)));
    }

    #[test]
    fn test_registry_register_modal() {
        let mut registry = ClickZoneRegistry::new();

        registry.register(ClickZoneId::ChatArea, Rect::new(0, 0, 100, 100));
        registry.register_modal(Rect::new(20, 20, 60, 60));

        // Modal should capture clicks inside it
        assert_eq!(registry.find(50, 50), Some(ClickZoneId::Modal));
    }

    #[test]
    fn test_registry_set_enabled() {
        let mut registry = ClickZoneRegistry::new();

        registry.register(ClickZoneId::Sidebar, Rect::new(0, 0, 30, 24));

        // Initially enabled
        assert_eq!(registry.find(15, 12), Some(ClickZoneId::Sidebar));

        // Disable it
        registry.set_enabled(ClickZoneId::Sidebar, false);
        assert_eq!(registry.find(15, 12), None);

        // Re-enable it
        registry.set_enabled(ClickZoneId::Sidebar, true);
        assert_eq!(registry.find(15, 12), Some(ClickZoneId::Sidebar));
    }

    #[test]
    fn test_registry_iter() {
        let mut registry = ClickZoneRegistry::new();

        registry.register(ClickZoneId::Sidebar, Rect::new(0, 0, 30, 24));
        registry.register(ClickZoneId::ChatArea, Rect::new(30, 0, 50, 24));

        let ids: Vec<_> = registry.iter().map(|z| z.id).collect();

        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&ClickZoneId::Sidebar));
        assert!(ids.contains(&ClickZoneId::ChatArea));
    }
}
