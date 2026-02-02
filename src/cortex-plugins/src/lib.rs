//! # Cortex Plugin System
//!
//! A complete WASM-based plugin system for Cortex CLI that allows users to extend
//! functionality through custom plugins.
//!
//! ## Features
//!
//! - **WASM Runtime**: Plugins run in isolated WebAssembly sandboxes for security
//! - **Hook System**: Intercept and modify tool execution, chat messages, permissions
//! - **Custom Commands**: Add new slash commands to the CLI
//! - **Event Bus**: Subscribe to system events (session start/end, tool execution, etc.)
//! - **Configuration**: Plugins can define and access their own configuration
//! - **Hot Reload**: Development mode supports automatic plugin reloading
//!
//! ## Plugin Structure
//!
//! Plugins are distributed as WASM modules with a manifest file:
//!
//! ```text
//! my-plugin/
//! ├── plugin.toml      # Plugin manifest
//! ├── plugin.wasm      # Compiled WASM module
//! └── README.md        # Optional documentation
//! ```
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_plugins::{PluginManager, PluginConfig};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create plugin manager
//!     let config = PluginConfig::default();
//!     let manager = PluginManager::new(config).await?;
//!
//!     // Discover and load plugins
//!     manager.discover_and_load().await?;
//!
//!     // List installed plugins
//!     for plugin in manager.list_plugins().await {
//!         println!("{}: {}", plugin.info.name, plugin.info.version);
//!     }
//!
//!     Ok(())
//! }
//! ```

pub mod api;
pub mod commands;
pub mod config;
pub mod error;
pub mod events;
pub mod hooks;
pub mod loader;
pub mod manager;
pub mod manifest;
pub mod plugin;
pub mod registry;
pub mod runtime;
pub mod sdk;

// Re-exports for convenience
pub use api::{PluginApi, PluginContext, PluginHostFunctions};
pub use commands::{PluginCommand, PluginCommandArg, PluginCommandRegistry};
pub use config::PluginConfig;
pub use error::{PluginError, Result};
pub use events::{Event, EventBus, EventHandler, EventSubscription};

// Hook system re-exports
pub use hooks::{
    // Prompt/AI hooks
    AiResponseAfterHook,
    AiResponseAfterInput,
    AiResponseAfterOutput,
    AiResponseBeforeHook,
    AiResponseBeforeInput,
    AiResponseBeforeOutput,
    AiResponseStreamHook,
    AiResponseStreamInput,
    AiResponseStreamOutput,
    AnimationFrameHook,
    AnimationFrameInput,
    AnimationFrameOutput,
    ArgumentCompletionHook,
    ArgumentCompletionInput,
    ArgumentCompletionOutput,
    ArgumentDefinition,
    BorderStyle,
    // Chat hooks
    ChatMessageHook,
    ChatMessageInput,
    ChatMessageOutput,
    // Clipboard hooks
    ClipboardCopyHook,
    ClipboardCopyInput,
    ClipboardCopyOutput,
    ClipboardPasteHook,
    ClipboardPasteInput,
    ClipboardPasteOutput,
    ClipboardSource,
    Color,
    // Command hooks
    CommandExecuteAfterHook,
    CommandExecuteAfterInput,
    CommandExecuteAfterOutput,
    CommandExecuteBeforeHook,
    CommandExecuteBeforeInput,
    CommandExecuteBeforeOutput,
    CompletionContext,
    CompletionItem,
    // Completion hooks
    CompletionKind,
    CompletionProvider,
    CompletionProviderRegisterHook,
    CompletionProviderRegisterInput,
    CompletionProviderRegisterOutput,
    CompletionRequestHook,
    CompletionRequestInput,
    CompletionRequestOutput,
    CompletionResolveHook,
    CompletionResolveInput,
    CompletionResolveOutput,
    // Config hooks
    ConfigChangeAction,
    ConfigChangeSource,
    ConfigChangedHook,
    ConfigChangedInput,
    ConfigChangedOutput,
    ContextDocument,
    ContextDocumentType,
    CustomEventEmitHook,
    CustomEventEmitInput,
    CustomEventEmitOutput,
    // Error hooks
    ErrorHandleHook,
    ErrorHandleInput,
    ErrorHandleOutput,
    ErrorRecovery,
    ErrorSource,
    EventInterceptHook,
    EventInterceptInput,
    EventInterceptOutput,
    // File operation hooks
    FileOperation,
    FileOperationAfterHook,
    FileOperationAfterInput,
    FileOperationAfterOutput,
    FileOperationBeforeHook,
    FileOperationBeforeInput,
    FileOperationBeforeOutput,
    FilePostAction,
    // Focus hooks
    FocusAction,
    FocusChangeHook,
    FocusChangeInput,
    FocusChangeOutput,
    // Core hook types
    HookDispatcher,
    HookPriority,
    HookRegistry,
    HookResult,
    // Input hooks
    InputAction,
    InputInterceptHook,
    InputInterceptInput,
    InputInterceptOutput,
    InputSuggestion,
    InterceptMode,
    KeyBinding,
    KeyBindingHook,
    KeyBindingInput,
    KeyBindingOutput,
    KeyBindingResult,
    KeyModifier,
    LayoutConfig,
    LayoutCustomizeHook,
    LayoutCustomizeInput,
    LayoutCustomizeOutput,
    LayoutDirection,
    LayoutPanel,
    MessagePart,
    ModalDefinition,
    ModalInjectHook,
    ModalInjectInput,
    ModalInjectOutput,
    ModalLayer,
    MouseButton,
    MouseEventType,
    // Permission hooks
    PermissionAskHook,
    PermissionAskInput,
    PermissionAskOutput,
    PermissionDecision,
    // Workspace hooks
    ProjectType,
    PromptInjectHook,
    PromptInjectInput,
    PromptInjectOutput,
    QuickPickItem,
    ScrollDirection,
    // Session hooks
    SessionEndAction,
    SessionEndHook,
    SessionEndInput,
    SessionEndOutput,
    SessionStartHook,
    SessionStartInput,
    SessionStartOutput,
    SuggestionKind,
    TextStyle,
    ThemeColors,
    ThemeOverride,
    ThemeOverrideHook,
    ThemeOverrideInput,
    ThemeOverrideOutput,
    ToastDefinition,
    ToastLevel,
    ToastShowHook,
    ToastShowInput,
    ToastShowOutput,
    TokenUsage,
    // Tool hooks
    ToolExecuteAfterHook,
    ToolExecuteAfterInput,
    ToolExecuteAfterOutput,
    ToolExecuteBeforeHook,
    ToolExecuteBeforeInput,
    ToolExecuteBeforeOutput,
    // TUI event hooks
    TuiEvent,
    TuiEventDispatchHook,
    TuiEventDispatchInput,
    TuiEventDispatchOutput,
    TuiEventFilter,
    TuiEventSubscribeHook,
    TuiEventSubscribeInput,
    TuiEventSubscribeOutput,
    // UI hooks - Basic
    UiComponent,
    // UI hooks - Advanced
    UiRegion,
    UiRenderHook,
    UiRenderInput,
    UiRenderOutput,
    UiWidget,
    WidgetConstraints,
    WidgetRegisterHook,
    WidgetRegisterInput,
    WidgetRegisterOutput,
    WidgetSize,
    WidgetStyle,
    // Workspace hooks
    WorkspaceChangedHook,
    WorkspaceChangedInput,
    WorkspaceChangedOutput,
};

// SDK re-exports
pub use sdk::{
    CARGO_TEMPLATE, HOT_RELOAD_CONFIG, HotReloadConfig, MANIFEST_TEMPLATE, PluginDev,
    RUST_ADVANCED_TEMPLATE, RUST_TEMPLATE, TEST_UTILS_TEMPLATE, TSCONFIG_TEMPLATE,
    TYPESCRIPT_TEMPLATE, generate_advanced_rust_code, generate_cargo_toml,
    generate_hot_reload_config, generate_manifest, generate_rust_code, generate_test_utils,
    generate_typescript_code,
};

pub use loader::PluginLoader;
pub use manager::PluginManager;
pub use manifest::{
    HookType, PluginCapability, PluginCommandManifest, PluginDependency, PluginHookManifest,
    PluginManifest, PluginPermission,
};
pub use plugin::{Plugin, PluginInfo, PluginState, PluginStatus};
pub use registry::PluginRegistry;
pub use runtime::{WasmPlugin, WasmRuntime};

/// Plugin system version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default plugin directory name
pub const PLUGIN_DIR: &str = "plugins";

/// Plugin manifest filename
pub const MANIFEST_FILE: &str = "plugin.toml";

/// Compiled WASM module filename
pub const WASM_FILE: &str = "plugin.wasm";
