//! TUI-level event hooks for plugin integration.
//!
//! This module provides hooks that allow plugins to:
//! - Subscribe to TUI events (render, resize, focus, scroll)
//! - Emit custom events
//! - Intercept and modify event propagation
//!
//! These hooks bridge the gap between the TUI event system and plugins,
//! allowing rich interaction while maintaining sandboxed execution.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::types::{HookPriority, HookResult};
use crate::Result;

// ============================================================================
// TUI EVENT TYPES
// ============================================================================

/// TUI-level events that plugins can subscribe to
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TuiEvent {
    /// Frame rendered (called every frame, ~120 FPS)
    FrameRendered {
        frame_number: u64,
        render_time_us: u64,
    },
    /// Terminal resized
    Resized {
        width: u16,
        height: u16,
        previous_width: u16,
        previous_height: u16,
    },
    /// Focus changed between components
    FocusChanged {
        previous_focus: Option<String>,
        new_focus: String,
    },
    /// Scroll position changed
    ScrollChanged {
        component: String,
        position: usize,
        max_position: usize,
        direction: ScrollDirection,
    },
    /// View/screen changed
    ViewChanged {
        previous_view: Option<String>,
        new_view: String,
    },
    /// Modal opened
    ModalOpened {
        modal_id: String,
        modal_type: String,
    },
    /// Modal closed
    ModalClosed {
        modal_id: String,
        result: Option<String>,
    },
    /// Input focus gained
    InputFocused { input_id: String },
    /// Input focus lost
    InputBlurred { input_id: String, value: String },
    /// Selection changed in a list/tree
    SelectionChanged {
        component: String,
        selected_index: Option<usize>,
        selected_id: Option<String>,
    },
    /// Theme changed
    ThemeChanged {
        previous_theme: String,
        new_theme: String,
    },
    /// Sidebar toggled
    SidebarToggled { visible: bool },
    /// Panel collapsed/expanded
    PanelToggled { panel_id: String, collapsed: bool },
    /// Key pressed (after action mapping)
    KeyPressed {
        key: String,
        modifiers: Vec<String>,
        action: Option<String>,
    },
    /// Mouse event
    MouseEvent {
        event_type: MouseEventType,
        x: u16,
        y: u16,
        button: Option<MouseButton>,
    },
    /// Custom plugin event
    Custom {
        plugin_id: String,
        event_name: String,
        data: serde_json::Value,
    },
}

/// Scroll direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
}

/// Mouse event type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseEventType {
    Click,
    DoubleClick,
    RightClick,
    MiddleClick,
    Scroll,
    Move,
    Drag,
    DragEnd,
}

/// Mouse button
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

// ============================================================================
// TUI EVENT SUBSCRIPTION HOOK
// ============================================================================

/// Event filter for subscriptions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TuiEventFilter {
    /// Event types to subscribe to (empty = all)
    #[serde(default)]
    pub event_types: Vec<String>,
    /// Components to filter by (empty = all)
    #[serde(default)]
    pub components: Vec<String>,
    /// Whether to include frame events (high frequency)
    #[serde(default)]
    pub include_frame_events: bool,
    /// Whether to include mouse move events (high frequency)
    #[serde(default)]
    pub include_mouse_move: bool,
    /// Minimum interval between events (for throttling)
    #[serde(default)]
    pub throttle_ms: Option<u64>,
}

/// Input for TUI event subscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiEventSubscribeInput {
    /// Plugin ID subscribing
    pub plugin_id: String,
    /// Session ID
    pub session_id: String,
    /// Event filter
    #[serde(default)]
    pub filter: TuiEventFilter,
}

/// Output for TUI event subscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiEventSubscribeOutput {
    /// Whether subscription succeeded
    pub success: bool,
    /// Subscription ID
    #[serde(default)]
    pub subscription_id: Option<String>,
    /// Error if failed
    #[serde(default)]
    pub error: Option<String>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl TuiEventSubscribeOutput {
    pub fn success(subscription_id: impl Into<String>) -> Self {
        Self {
            success: true,
            subscription_id: Some(subscription_id.into()),
            error: None,
            result: HookResult::Continue,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            subscription_id: None,
            error: Some(message.into()),
            result: HookResult::Continue,
        }
    }
}

/// Handler for TUI event subscription
#[async_trait]
pub trait TuiEventSubscribeHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &TuiEventSubscribeInput,
        output: &mut TuiEventSubscribeOutput,
    ) -> Result<()>;
}

// ============================================================================
// TUI EVENT DISPATCH HOOK
// ============================================================================

/// Input for TUI event dispatch (to plugins)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiEventDispatchInput {
    /// Session ID
    pub session_id: String,
    /// The event being dispatched
    pub event: TuiEvent,
    /// Target plugin (None = broadcast to all subscribers)
    #[serde(default)]
    pub target_plugin: Option<String>,
}

/// Output for TUI event dispatch
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TuiEventDispatchOutput {
    /// Whether event should continue propagating
    #[serde(default)]
    pub propagate: bool,
    /// Modifications to the event
    #[serde(default)]
    pub modifications: HashMap<String, serde_json::Value>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl TuiEventDispatchOutput {
    pub fn new() -> Self {
        Self {
            propagate: true,
            modifications: HashMap::new(),
            result: HookResult::Continue,
        }
    }

    /// Stop event propagation
    pub fn stop_propagation(mut self) -> Self {
        self.propagate = false;
        self
    }

    /// Add event modification
    pub fn modify(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.modifications.insert(key.into(), value);
        self
    }
}

/// Handler for TUI event dispatch
#[async_trait]
pub trait TuiEventDispatchHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &TuiEventDispatchInput,
        output: &mut TuiEventDispatchOutput,
    ) -> Result<()>;
}

// ============================================================================
// CUSTOM EVENT EMIT HOOK
// ============================================================================

/// Input for emitting custom events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomEventEmitInput {
    /// Plugin ID emitting the event
    pub plugin_id: String,
    /// Session ID
    pub session_id: String,
    /// Event name
    pub event_name: String,
    /// Event data
    pub data: serde_json::Value,
    /// Target plugin (None = broadcast)
    #[serde(default)]
    pub target_plugin: Option<String>,
}

/// Output for custom event emit
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomEventEmitOutput {
    /// Whether event was emitted
    pub emitted: bool,
    /// Event ID
    #[serde(default)]
    pub event_id: Option<String>,
    /// Number of subscribers notified
    #[serde(default)]
    pub subscribers_notified: usize,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl CustomEventEmitOutput {
    pub fn success(event_id: impl Into<String>, subscribers: usize) -> Self {
        Self {
            emitted: true,
            event_id: Some(event_id.into()),
            subscribers_notified: subscribers,
            result: HookResult::Continue,
        }
    }
}

/// Handler for custom event emission
#[async_trait]
pub trait CustomEventEmitHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &CustomEventEmitInput,
        output: &mut CustomEventEmitOutput,
    ) -> Result<()>;
}

// ============================================================================
// EVENT INTERCEPT HOOK
// ============================================================================

/// Event interception mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterceptMode {
    /// Observe only, cannot modify
    Observe,
    /// Can modify the event
    Modify,
    /// Can block the event
    Block,
}

impl Default for InterceptMode {
    fn default() -> Self {
        Self::Observe
    }
}

/// Input for event interception
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInterceptInput {
    /// Plugin ID intercepting
    pub plugin_id: String,
    /// Session ID
    pub session_id: String,
    /// Event being intercepted
    pub event: TuiEvent,
    /// Interception mode requested
    pub mode: InterceptMode,
}

/// Output for event interception
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInterceptOutput {
    /// Whether to block the event
    #[serde(default)]
    pub blocked: bool,
    /// Modified event (if mode allows)
    #[serde(default)]
    pub modified_event: Option<TuiEvent>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl Default for EventInterceptOutput {
    fn default() -> Self {
        Self {
            blocked: false,
            modified_event: None,
            result: HookResult::Continue,
        }
    }
}

impl EventInterceptOutput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Block the event
    pub fn block(mut self) -> Self {
        self.blocked = true;
        self
    }

    /// Modify the event
    pub fn modify(mut self, event: TuiEvent) -> Self {
        self.modified_event = Some(event);
        self
    }
}

/// Handler for event interception
#[async_trait]
pub trait EventInterceptHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// What interception mode this hook supports
    fn intercept_mode(&self) -> InterceptMode {
        InterceptMode::Observe
    }

    /// Event types this hook intercepts (empty = all)
    fn event_types(&self) -> Vec<String> {
        vec![]
    }

    async fn execute(
        &self,
        input: &EventInterceptInput,
        output: &mut EventInterceptOutput,
    ) -> Result<()>;
}

// ============================================================================
// ANIMATION FRAME HOOK
// ============================================================================

/// Input for animation frame hook (called every frame)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationFrameInput {
    /// Session ID
    pub session_id: String,
    /// Current frame number
    pub frame: u64,
    /// Time since last frame (microseconds)
    pub delta_us: u64,
    /// Total elapsed time (microseconds)
    pub elapsed_us: u64,
}

/// Output for animation frame hook
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnimationFrameOutput {
    /// Widgets to update
    #[serde(default)]
    pub widget_updates: HashMap<String, serde_json::Value>,
    /// Whether to request another frame
    #[serde(default)]
    pub request_frame: bool,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl AnimationFrameOutput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update a widget
    pub fn update_widget(mut self, widget_id: impl Into<String>, data: serde_json::Value) -> Self {
        self.widget_updates.insert(widget_id.into(), data);
        self
    }

    /// Request animation continue
    pub fn continue_animation(mut self) -> Self {
        self.request_frame = true;
        self
    }
}

/// Handler for animation frames
#[async_trait]
pub trait AnimationFrameHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Whether this hook needs frame callbacks
    fn needs_frames(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        input: &AnimationFrameInput,
        output: &mut AnimationFrameOutput,
    ) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tui_event_serialization() {
        let event = TuiEvent::Resized {
            width: 120,
            height: 40,
            previous_width: 100,
            previous_height: 30,
        };
        let json = serde_json::to_string(&event).expect("serialize failed");
        assert!(json.contains("resized"));
        assert!(json.contains("120"));
    }

    #[test]
    fn test_event_filter_default() {
        let filter = TuiEventFilter::default();
        assert!(filter.event_types.is_empty());
        assert!(!filter.include_frame_events);
    }

    #[test]
    fn test_subscribe_output_success() {
        let output = TuiEventSubscribeOutput::success("sub-123");
        assert!(output.success);
        assert_eq!(output.subscription_id, Some("sub-123".to_string()));
    }

    #[test]
    fn test_dispatch_output() {
        let output = TuiEventDispatchOutput::new()
            .stop_propagation()
            .modify("key", serde_json::json!("value"));

        assert!(!output.propagate);
        assert!(output.modifications.contains_key("key"));
    }

    #[test]
    fn test_intercept_output_block() {
        let output = EventInterceptOutput::new().block();
        assert!(output.blocked);
    }

    #[test]
    fn test_animation_frame_output() {
        let output = AnimationFrameOutput::new()
            .update_widget("widget-1", serde_json::json!({"value": 50}))
            .continue_animation();

        assert!(output.request_frame);
        assert!(output.widget_updates.contains_key("widget-1"));
    }

    #[test]
    fn test_custom_event() {
        let event = TuiEvent::Custom {
            plugin_id: "my-plugin".to_string(),
            event_name: "my-event".to_string(),
            data: serde_json::json!({"foo": "bar"}),
        };

        if let TuiEvent::Custom { event_name, .. } = event {
            assert_eq!(event_name, "my-event");
        } else {
            panic!("Expected Custom event");
        }
    }
}
