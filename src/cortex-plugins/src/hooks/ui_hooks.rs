//! Advanced TUI UI rendering hooks for plugin customization.
//!
//! This module provides comprehensive UI hooks that allow plugins to:
//! - Register and render custom widgets
//! - Modify existing UI components (header, footer, sidebar, chat)
//! - Override color schemes and themes dynamically
//! - Add custom keyboard shortcuts
//! - Inject modals, sidebars, and overlays
//! - Control widget positioning and sizing

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::types::{HookPriority, HookResult};
use crate::Result;

// ============================================================================
// UI REGION TYPES
// ============================================================================

/// UI regions where plugins can inject content
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRegion {
    /// Top header area
    Header,
    /// Bottom footer/status bar area
    Footer,
    /// Left sidebar area
    SidebarLeft,
    /// Right sidebar area
    SidebarRight,
    /// Main chat/content area
    MainContent,
    /// Input area at the bottom
    InputArea,
    /// Overlay layer (modals, popups)
    Overlay,
    /// Status indicators area
    StatusBar,
    /// Tool output area
    ToolOutput,
    /// Message display area
    MessageArea,
}

impl std::fmt::Display for UiRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header => write!(f, "header"),
            Self::Footer => write!(f, "footer"),
            Self::SidebarLeft => write!(f, "sidebar_left"),
            Self::SidebarRight => write!(f, "sidebar_right"),
            Self::MainContent => write!(f, "main_content"),
            Self::InputArea => write!(f, "input_area"),
            Self::Overlay => write!(f, "overlay"),
            Self::StatusBar => write!(f, "status_bar"),
            Self::ToolOutput => write!(f, "tool_output"),
            Self::MessageArea => write!(f, "message_area"),
        }
    }
}

// ============================================================================
// UI COMPONENT TYPES
// ============================================================================

/// UI components that can be customized
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiComponent {
    /// Chat message display
    ChatMessage { role: String, message_id: String },
    /// Tool output display
    ToolOutput { tool: String, call_id: String },
    /// Status bar
    StatusBar,
    /// Input area
    InputArea,
    /// Header
    Header,
    /// Sidebar
    Sidebar,
    /// Modal/dialog
    Modal { modal_type: String },
    /// Notification toast
    Toast { level: String },
    /// Progress indicator
    Progress { operation: String },
    /// Custom component
    Custom {
        component_type: String,
        data: serde_json::Value,
    },
}

// ============================================================================
// STYLE TYPES
// ============================================================================

/// Color specification (supports multiple formats)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Color {
    /// Named color (e.g., "red", "cyan", "green")
    Named(String),
    /// RGB color
    Rgb { r: u8, g: u8, b: u8 },
    /// Hex color (e.g., "#FF5733")
    Hex(String),
    /// ANSI 256 color index
    Ansi256(u8),
}

impl Default for Color {
    fn default() -> Self {
        Self::Named("white".to_string())
    }
}

/// Border style for widgets
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BorderStyle {
    /// No border
    None,
    /// Plain single line border
    Plain,
    /// Rounded corners
    Rounded,
    /// Double line border
    Double,
    /// Thick line border
    Thick,
    /// Quadrant inside style
    QuadrantInside,
    /// Quadrant outside style
    QuadrantOutside,
}

impl Default for BorderStyle {
    fn default() -> Self {
        Self::Plain
    }
}

/// Text style modifiers
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextStyle {
    /// Foreground color
    #[serde(default)]
    pub fg: Option<Color>,
    /// Background color
    #[serde(default)]
    pub bg: Option<Color>,
    /// Bold text
    #[serde(default)]
    pub bold: bool,
    /// Italic text
    #[serde(default)]
    pub italic: bool,
    /// Underlined text
    #[serde(default)]
    pub underline: bool,
    /// Strikethrough text
    #[serde(default)]
    pub strikethrough: bool,
    /// Dim/faded text
    #[serde(default)]
    pub dim: bool,
}

/// Widget styling options
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WidgetStyle {
    /// Border style
    #[serde(default)]
    pub border: BorderStyle,
    /// Border color
    #[serde(default)]
    pub border_color: Option<Color>,
    /// Background color
    #[serde(default)]
    pub background: Option<Color>,
    /// Padding (top, right, bottom, left)
    #[serde(default)]
    pub padding: [u16; 4],
    /// Margin (top, right, bottom, left)
    #[serde(default)]
    pub margin: [u16; 4],
    /// Title style
    #[serde(default)]
    pub title_style: Option<TextStyle>,
    /// Content style
    #[serde(default)]
    pub content_style: Option<TextStyle>,
}

// ============================================================================
// WIDGET TYPES
// ============================================================================

/// Widget sizing constraints
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetSize {
    /// Fixed size in cells
    Fixed(u16),
    /// Percentage of parent
    Percent(u16),
    /// Minimum size
    Min(u16),
    /// Maximum size
    Max(u16),
    /// Fill available space
    Fill,
    /// Auto-size based on content
    Auto,
}

impl Default for WidgetSize {
    fn default() -> Self {
        Self::Auto
    }
}

/// Widget layout constraints
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WidgetConstraints {
    /// Width constraint
    #[serde(default)]
    pub width: WidgetSize,
    /// Height constraint
    #[serde(default)]
    pub height: WidgetSize,
    /// Minimum width
    #[serde(default)]
    pub min_width: Option<u16>,
    /// Maximum width
    #[serde(default)]
    pub max_width: Option<u16>,
    /// Minimum height
    #[serde(default)]
    pub min_height: Option<u16>,
    /// Maximum height
    #[serde(default)]
    pub max_height: Option<u16>,
}

/// Custom UI widget that plugins can register
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiWidget {
    /// Text block
    Text {
        content: String,
        #[serde(default)]
        style: TextStyle,
        #[serde(default)]
        wrap: bool,
    },
    /// Progress bar
    ProgressBar {
        value: f32,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        style: WidgetStyle,
    },
    /// Status indicator (colored dot with label)
    StatusIndicator {
        status: String,
        #[serde(default)]
        color: Option<Color>,
        #[serde(default)]
        label: Option<String>,
    },
    /// Clickable button
    Button {
        label: String,
        action: String,
        #[serde(default)]
        style: WidgetStyle,
        #[serde(default)]
        disabled: bool,
    },
    /// Badge/tag
    Badge {
        text: String,
        #[serde(default)]
        color: Option<Color>,
        #[serde(default)]
        bg_color: Option<Color>,
    },
    /// List of items
    List {
        items: Vec<String>,
        #[serde(default)]
        selected: Option<usize>,
        #[serde(default)]
        style: WidgetStyle,
    },
    /// Table
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        #[serde(default)]
        style: WidgetStyle,
        #[serde(default)]
        widths: Vec<WidgetSize>,
    },
    /// Horizontal separator
    Separator {
        #[serde(default)]
        style: Option<TextStyle>,
    },
    /// Gauge/meter
    Gauge {
        value: f32,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        color: Option<Color>,
    },
    /// Sparkline chart
    Sparkline {
        data: Vec<u64>,
        #[serde(default)]
        color: Option<Color>,
    },
    /// Horizontal layout container
    HorizontalLayout {
        children: Vec<UiWidget>,
        #[serde(default)]
        spacing: u16,
    },
    /// Vertical layout container
    VerticalLayout {
        children: Vec<UiWidget>,
        #[serde(default)]
        spacing: u16,
    },
    /// Block container with border and title
    Block {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        content: Option<Box<UiWidget>>,
        #[serde(default)]
        style: WidgetStyle,
    },
    /// Custom widget (plugin-specific rendering)
    Custom {
        widget_type: String,
        data: serde_json::Value,
        #[serde(default)]
        constraints: WidgetConstraints,
    },
}

// ============================================================================
// KEYBOARD SHORTCUT TYPES
// ============================================================================

/// Keyboard modifier keys
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyModifier {
    /// Control key
    Ctrl,
    /// Alt/Option key
    Alt,
    /// Shift key
    Shift,
    /// Super/Meta/Windows key
    Super,
}

/// Key binding definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBinding {
    /// The key code (e.g., "a", "Enter", "F1", "Escape")
    pub key: String,
    /// Modifier keys required
    #[serde(default)]
    pub modifiers: Vec<KeyModifier>,
    /// Action to trigger
    pub action: String,
    /// Description for help
    #[serde(default)]
    pub description: Option<String>,
    /// Context where this binding is active (None = global)
    #[serde(default)]
    pub context: Option<String>,
}

/// Key binding registration result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBindingResult {
    /// Whether the registration succeeded
    pub success: bool,
    /// Error message if failed
    #[serde(default)]
    pub error: Option<String>,
    /// ID of the registered binding
    #[serde(default)]
    pub binding_id: Option<String>,
}

// ============================================================================
// THEME TYPES
// ============================================================================

/// Theme color palette
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeColors {
    /// Primary accent color
    #[serde(default)]
    pub primary: Option<Color>,
    /// Secondary accent color
    #[serde(default)]
    pub secondary: Option<Color>,
    /// Background color
    #[serde(default)]
    pub background: Option<Color>,
    /// Foreground/text color
    #[serde(default)]
    pub foreground: Option<Color>,
    /// Success/positive color
    #[serde(default)]
    pub success: Option<Color>,
    /// Warning color
    #[serde(default)]
    pub warning: Option<Color>,
    /// Error/danger color
    #[serde(default)]
    pub error: Option<Color>,
    /// Info/informational color
    #[serde(default)]
    pub info: Option<Color>,
    /// Border color
    #[serde(default)]
    pub border: Option<Color>,
    /// Selection highlight color
    #[serde(default)]
    pub selection: Option<Color>,
    /// Muted/dimmed text color
    #[serde(default)]
    pub muted: Option<Color>,
    /// Custom colors by name
    #[serde(default)]
    pub custom: HashMap<String, Color>,
}

/// Theme override that plugins can apply
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeOverride {
    /// Color palette overrides
    #[serde(default)]
    pub colors: ThemeColors,
    /// Component-specific style overrides
    #[serde(default)]
    pub components: HashMap<String, WidgetStyle>,
}

// ============================================================================
// UI RENDER HOOK
// ============================================================================

/// Input for ui.render hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiRenderInput {
    /// Session ID
    pub session_id: String,
    /// Component being rendered
    pub component: UiComponent,
    /// Current theme name
    pub theme: String,
    /// Current terminal dimensions (width, height)
    #[serde(default)]
    pub dimensions: (u16, u16),
    /// Whether the component has focus
    #[serde(default)]
    pub has_focus: bool,
    /// Current frame number (for animations)
    #[serde(default)]
    pub frame: u64,
}

/// Output for ui.render hook
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiRenderOutput {
    /// Custom styles to apply to the component
    #[serde(default)]
    pub styles: HashMap<String, String>,
    /// Additional content to render after the component
    #[serde(default)]
    pub extra_content: Option<String>,
    /// Widgets to inject into the component
    #[serde(default)]
    pub widgets: Vec<UiWidget>,
    /// Style overrides for the component
    #[serde(default)]
    pub style_override: Option<WidgetStyle>,
    /// Theme overrides to apply temporarily
    #[serde(default)]
    pub theme_override: Option<ThemeOverride>,
    /// Hook result (continue, skip, abort)
    #[serde(default)]
    pub result: HookResult,
}

impl UiRenderOutput {
    /// Create a new empty output
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a widget to inject
    pub fn add_widget(&mut self, widget: UiWidget) {
        self.widgets.push(widget);
    }

    /// Set a style value
    pub fn set_style(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.styles.insert(key.into(), value.into());
    }

    /// Set the style override
    pub fn with_style(mut self, style: WidgetStyle) -> Self {
        self.style_override = Some(style);
        self
    }

    /// Set extra content
    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.extra_content = Some(content.into());
        self
    }
}

/// Handler for ui.render hook
#[async_trait]
pub trait UiRenderHook: Send + Sync {
    /// Get the priority of this hook
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Get components this hook applies to (None = all)
    fn components(&self) -> Option<Vec<String>> {
        None
    }

    /// Execute the hook
    async fn execute(&self, input: &UiRenderInput, output: &mut UiRenderOutput) -> Result<()>;
}

// ============================================================================
// WIDGET REGISTRATION HOOK
// ============================================================================

/// Input for widget registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetRegisterInput {
    /// Plugin ID registering the widget
    pub plugin_id: String,
    /// Widget type identifier
    pub widget_type: String,
    /// Region where widget should appear
    pub region: UiRegion,
    /// Widget constraints
    #[serde(default)]
    pub constraints: WidgetConstraints,
    /// Initial widget data
    #[serde(default)]
    pub initial_data: Option<serde_json::Value>,
}

/// Output for widget registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetRegisterOutput {
    /// Whether registration succeeded
    pub success: bool,
    /// Widget ID assigned (for updates/removal)
    #[serde(default)]
    pub widget_id: Option<String>,
    /// Error message if failed
    #[serde(default)]
    pub error: Option<String>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl WidgetRegisterOutput {
    /// Create a successful registration output
    pub fn success(widget_id: impl Into<String>) -> Self {
        Self {
            success: true,
            widget_id: Some(widget_id.into()),
            error: None,
            result: HookResult::Continue,
        }
    }

    /// Create an error registration output
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            widget_id: None,
            error: Some(message.into()),
            result: HookResult::Continue,
        }
    }
}

impl Default for WidgetRegisterOutput {
    fn default() -> Self {
        Self {
            success: false,
            widget_id: None,
            error: None,
            result: HookResult::Continue,
        }
    }
}

/// Handler for widget registration
#[async_trait]
pub trait WidgetRegisterHook: Send + Sync {
    /// Get the priority of this hook
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook
    async fn execute(
        &self,
        input: &WidgetRegisterInput,
        output: &mut WidgetRegisterOutput,
    ) -> Result<()>;
}

// ============================================================================
// KEYBOARD BINDING HOOK
// ============================================================================

/// Input for keyboard binding registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBindingInput {
    /// Plugin ID registering the binding
    pub plugin_id: String,
    /// Key binding definition
    pub binding: KeyBinding,
}

/// Output for keyboard binding registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBindingOutput {
    /// Registration result
    pub result: KeyBindingResult,
    /// Hook result
    #[serde(default)]
    pub hook_result: HookResult,
}

impl KeyBindingOutput {
    /// Create a successful binding output
    pub fn success(binding_id: impl Into<String>) -> Self {
        Self {
            result: KeyBindingResult {
                success: true,
                error: None,
                binding_id: Some(binding_id.into()),
            },
            hook_result: HookResult::Continue,
        }
    }

    /// Create an error binding output
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            result: KeyBindingResult {
                success: false,
                error: Some(message.into()),
                binding_id: None,
            },
            hook_result: HookResult::Continue,
        }
    }
}

impl Default for KeyBindingOutput {
    fn default() -> Self {
        Self {
            result: KeyBindingResult {
                success: false,
                error: None,
                binding_id: None,
            },
            hook_result: HookResult::Continue,
        }
    }
}

/// Handler for keyboard binding registration
#[async_trait]
pub trait KeyBindingHook: Send + Sync {
    /// Get the priority of this hook
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook
    async fn execute(&self, input: &KeyBindingInput, output: &mut KeyBindingOutput) -> Result<()>;
}

// ============================================================================
// THEME OVERRIDE HOOK
// ============================================================================

/// Input for theme override
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeOverrideInput {
    /// Session ID
    pub session_id: String,
    /// Current theme name
    pub current_theme: String,
    /// Current theme colors
    pub current_colors: ThemeColors,
}

/// Output for theme override
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeOverrideOutput {
    /// Theme overrides to apply
    #[serde(default)]
    pub overrides: Option<ThemeOverride>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl ThemeOverrideOutput {
    /// Create a new empty output
    pub fn new() -> Self {
        Self::default()
    }

    /// Set theme overrides
    pub fn with_override(mut self, override_: ThemeOverride) -> Self {
        self.overrides = Some(override_);
        self
    }
}

/// Handler for theme override hook
#[async_trait]
pub trait ThemeOverrideHook: Send + Sync {
    /// Get the priority of this hook
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook
    async fn execute(
        &self,
        input: &ThemeOverrideInput,
        output: &mut ThemeOverrideOutput,
    ) -> Result<()>;
}

// ============================================================================
// LAYOUT CUSTOMIZATION HOOK
// ============================================================================

/// Layout direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutDirection {
    /// Horizontal layout
    Horizontal,
    /// Vertical layout
    Vertical,
}

impl Default for LayoutDirection {
    fn default() -> Self {
        Self::Vertical
    }
}

/// Panel definition for layout
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutPanel {
    /// Panel identifier
    pub id: String,
    /// Panel region
    pub region: UiRegion,
    /// Size constraint
    #[serde(default)]
    pub size: WidgetSize,
    /// Whether panel is visible
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Whether panel is collapsible
    #[serde(default)]
    pub collapsible: bool,
    /// Whether panel is currently collapsed
    #[serde(default)]
    pub collapsed: bool,
    /// Panel title
    #[serde(default)]
    pub title: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Layout configuration from plugin
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayoutConfig {
    /// Main layout direction
    #[serde(default)]
    pub direction: LayoutDirection,
    /// Panels to add/modify
    #[serde(default)]
    pub panels: Vec<LayoutPanel>,
    /// Panels to hide
    #[serde(default)]
    pub hidden_panels: Vec<String>,
}

/// Input for layout customization hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutCustomizeInput {
    /// Session ID
    pub session_id: String,
    /// Current terminal dimensions
    pub dimensions: (u16, u16),
    /// Current layout config
    pub current_layout: LayoutConfig,
}

/// Output for layout customization hook
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayoutCustomizeOutput {
    /// Layout modifications
    #[serde(default)]
    pub layout_changes: Option<LayoutConfig>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl LayoutCustomizeOutput {
    /// Create a new empty output
    pub fn new() -> Self {
        Self::default()
    }

    /// Set layout changes
    pub fn with_layout(mut self, layout: LayoutConfig) -> Self {
        self.layout_changes = Some(layout);
        self
    }
}

/// Handler for layout customization hook
#[async_trait]
pub trait LayoutCustomizeHook: Send + Sync {
    /// Get the priority of this hook
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook
    async fn execute(
        &self,
        input: &LayoutCustomizeInput,
        output: &mut LayoutCustomizeOutput,
    ) -> Result<()>;
}

// ============================================================================
// MODAL INJECTION HOOK
// ============================================================================

/// Modal priority/layer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModalLayer {
    /// Background layer (behind other modals)
    Background = 0,
    /// Normal layer
    Normal = 1,
    /// High priority layer
    High = 2,
    /// Urgent/critical layer (topmost)
    Urgent = 3,
}

impl Default for ModalLayer {
    fn default() -> Self {
        Self::Normal
    }
}

/// Modal definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModalDefinition {
    /// Modal identifier
    pub id: String,
    /// Modal title
    pub title: String,
    /// Modal content widget
    pub content: UiWidget,
    /// Modal style
    #[serde(default)]
    pub style: WidgetStyle,
    /// Modal layer
    #[serde(default)]
    pub layer: ModalLayer,
    /// Whether modal is dismissible (Escape key)
    #[serde(default = "default_true")]
    pub dismissible: bool,
    /// Width constraint
    #[serde(default)]
    pub width: WidgetSize,
    /// Height constraint
    #[serde(default)]
    pub height: WidgetSize,
    /// Action buttons
    #[serde(default)]
    pub buttons: Vec<UiWidget>,
}

/// Input for modal injection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModalInjectInput {
    /// Plugin ID requesting modal
    pub plugin_id: String,
    /// Modal definition
    pub modal: ModalDefinition,
}

/// Output for modal injection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModalInjectOutput {
    /// Whether modal was shown
    pub shown: bool,
    /// Modal ID for later dismissal
    #[serde(default)]
    pub modal_id: Option<String>,
    /// Error if failed
    #[serde(default)]
    pub error: Option<String>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl ModalInjectOutput {
    /// Create a successful modal output
    pub fn success(modal_id: impl Into<String>) -> Self {
        Self {
            shown: true,
            modal_id: Some(modal_id.into()),
            error: None,
            result: HookResult::Continue,
        }
    }

    /// Create an error modal output
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            shown: false,
            modal_id: None,
            error: Some(message.into()),
            result: HookResult::Continue,
        }
    }
}

impl Default for ModalInjectOutput {
    fn default() -> Self {
        Self {
            shown: false,
            modal_id: None,
            error: None,
            result: HookResult::Continue,
        }
    }
}

/// Handler for modal injection
#[async_trait]
pub trait ModalInjectHook: Send + Sync {
    /// Get the priority of this hook
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook
    async fn execute(&self, input: &ModalInjectInput, output: &mut ModalInjectOutput)
    -> Result<()>;
}

// ============================================================================
// TOAST/NOTIFICATION HOOK
// ============================================================================

/// Toast notification level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastLevel {
    /// Informational toast
    Info,
    /// Success toast
    Success,
    /// Warning toast
    Warning,
    /// Error toast
    Error,
}

impl Default for ToastLevel {
    fn default() -> Self {
        Self::Info
    }
}

/// Toast notification definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToastDefinition {
    /// Toast message
    pub message: String,
    /// Toast level
    #[serde(default)]
    pub level: ToastLevel,
    /// Duration in milliseconds (0 = persistent)
    #[serde(default = "default_toast_duration")]
    pub duration_ms: u64,
    /// Optional title
    #[serde(default)]
    pub title: Option<String>,
    /// Optional action button (label, action_id)
    #[serde(default)]
    pub action: Option<(String, String)>,
}

fn default_toast_duration() -> u64 {
    3000
}

/// Input for toast notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToastShowInput {
    /// Plugin ID showing toast
    pub plugin_id: String,
    /// Toast definition
    pub toast: ToastDefinition,
}

/// Output for toast notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToastShowOutput {
    /// Whether toast was shown
    pub shown: bool,
    /// Toast ID for later dismissal
    #[serde(default)]
    pub toast_id: Option<String>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl ToastShowOutput {
    /// Create a successful toast output
    pub fn success(toast_id: impl Into<String>) -> Self {
        Self {
            shown: true,
            toast_id: Some(toast_id.into()),
            result: HookResult::Continue,
        }
    }
}

impl Default for ToastShowOutput {
    fn default() -> Self {
        Self {
            shown: false,
            toast_id: None,
            result: HookResult::Continue,
        }
    }
}

/// Handler for toast notifications
#[async_trait]
pub trait ToastShowHook: Send + Sync {
    /// Get the priority of this hook
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook
    async fn execute(&self, input: &ToastShowInput, output: &mut ToastShowOutput) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_region_display() {
        assert_eq!(UiRegion::Header.to_string(), "header");
        assert_eq!(UiRegion::SidebarLeft.to_string(), "sidebar_left");
    }

    #[test]
    fn test_widget_style_default() {
        let style = WidgetStyle::default();
        assert_eq!(style.border, BorderStyle::Plain);
        assert!(style.background.is_none());
    }

    #[test]
    fn test_ui_render_output() {
        let mut output = UiRenderOutput::new();
        output.set_style("color", "cyan");
        output.add_widget(UiWidget::Badge {
            text: "Test".to_string(),
            color: Some(Color::Named("green".to_string())),
            bg_color: None,
        });

        assert_eq!(output.styles.get("color"), Some(&"cyan".to_string()));
        assert_eq!(output.widgets.len(), 1);
    }

    #[test]
    fn test_theme_colors() {
        let colors = ThemeColors {
            primary: Some(Color::Hex("#00CED1".to_string())),
            ..Default::default()
        };
        assert!(colors.primary.is_some());
        assert!(colors.secondary.is_none());
    }

    #[test]
    fn test_key_binding() {
        let binding = KeyBinding {
            key: "k".to_string(),
            modifiers: vec![KeyModifier::Ctrl],
            action: "kill_line".to_string(),
            description: Some("Kill line".to_string()),
            context: None,
        };
        assert_eq!(binding.modifiers.len(), 1);
    }

    #[test]
    fn test_widget_register_output() {
        let output = WidgetRegisterOutput::success("widget-123");
        assert!(output.success);
        assert_eq!(output.widget_id, Some("widget-123".to_string()));
    }

    #[test]
    fn test_modal_definition() {
        let modal = ModalDefinition {
            id: "test-modal".to_string(),
            title: "Test".to_string(),
            content: UiWidget::Text {
                content: "Hello".to_string(),
                style: TextStyle::default(),
                wrap: false,
            },
            style: WidgetStyle::default(),
            layer: ModalLayer::Normal,
            dismissible: true,
            width: WidgetSize::Percent(50),
            height: WidgetSize::Auto,
            buttons: vec![],
        };
        assert!(modal.dismissible);
    }

    #[test]
    fn test_toast_definition() {
        let toast = ToastDefinition {
            message: "Test message".to_string(),
            level: ToastLevel::Success,
            duration_ms: 5000,
            title: Some("Success!".to_string()),
            action: None,
        };
        assert_eq!(toast.level, ToastLevel::Success);
        assert_eq!(toast.duration_ms, 5000);
    }

    #[test]
    fn test_layout_panel() {
        let panel = LayoutPanel {
            id: "sidebar".to_string(),
            region: UiRegion::SidebarLeft,
            size: WidgetSize::Percent(20),
            visible: true,
            collapsible: true,
            collapsed: false,
            title: Some("Navigation".to_string()),
        };
        assert!(panel.visible);
        assert!(panel.collapsible);
    }
}
