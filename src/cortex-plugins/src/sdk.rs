//! Plugin SDK for developing Cortex plugins.
//!
//! This module provides utilities and documentation for plugin developers.
//!
//! # Creating a Plugin
//!
//! ## 1. Create a plugin manifest (`plugin.toml`)
//!
//! ```toml
//! [plugin]
//! id = "my-awesome-plugin"
//! name = "My Awesome Plugin"
//! version = "1.0.0"
//! description = "An awesome plugin that does awesome things"
//! authors = ["Your Name <your@email.com>"]
//!
//! # Capabilities your plugin needs
//! capabilities = ["commands", "hooks"]
//!
//! # Permissions your plugin requires
//! permissions = [
//!     { read_file = { paths = ["**/*.rs"] } },
//! ]
//!
//! # Commands provided by your plugin
//! [[commands]]
//! name = "awesome"
//! aliases = ["aw"]
//! description = "Do something awesome"
//! usage = "/awesome [arg]"
//!
//! [[commands.args]]
//! name = "arg"
//! description = "An optional argument"
//! required = false
//!
//! # Hooks your plugin registers
//! [[hooks]]
//! hook_type = "tool_execute_before"
//! priority = 50
//!
//! # Plugin configuration schema
//! [config]
//! api_key = { description = "API key for the service", type = "string", required = true }
//! max_items = { description = "Maximum items to process", type = "number", default = 10 }
//!
//! # WASM settings
//! [wasm]
//! memory_pages = 128
//! timeout_ms = 5000
//! ```
//!
//! ## 2. Write your plugin in Rust (compiles to WASM)
//!
//! ```rust,ignore
//! use cortex_plugin_sdk::*;
//!
//! // Plugin initialization
//! #[no_mangle]
//! pub extern "C" fn init() -> i32 {
//!     log_info("Plugin initialized!");
//!     0 // Return 0 for success
//! }
//!
//! // Plugin shutdown
//! #[no_mangle]
//! pub extern "C" fn shutdown() -> i32 {
//!     log_info("Plugin shutting down");
//!     0
//! }
//!
//! // Command handler
//! #[no_mangle]
//! pub extern "C" fn cmd_awesome() -> i32 {
//!     log_info("Awesome command executed!");
//!     0
//! }
//! ```
//!
//! ## 3. Build your plugin
//!
//! ```bash
//! # Add wasm32 target
//! rustup target add wasm32-wasi
//!
//! # Build
//! cargo build --target wasm32-wasi --release
//!
//! # Copy to plugin directory
//! cp target/wasm32-wasi/release/my_plugin.wasm ~/.cortex/plugins/my-awesome-plugin/plugin.wasm
//! ```
//!
//! ## 4. Install your plugin
//!
//! ```text
//! ~/.cortex/plugins/my-awesome-plugin/
//! ├── plugin.toml
//! └── plugin.wasm
//! ```

/// Example plugin manifest template.
pub const MANIFEST_TEMPLATE: &str = r#"[plugin]
id = "{{plugin_id}}"
name = "{{plugin_name}}"
version = "0.1.0"
description = "{{description}}"
authors = ["{{author}}"]

# Capabilities your plugin needs
# Available: commands, hooks, events, tools, formatters, themes, config, filesystem, shell, network
capabilities = ["commands"]

# Permissions your plugin requires (optional)
# permissions = [
#     { read_file = { paths = ["**/*"] } },
#     { execute = { commands = ["ls", "cat"] } },
#     { network = { domains = ["api.example.com"] } },
# ]

# Commands provided by your plugin
[[commands]]
name = "{{command_name}}"
description = "{{command_description}}"
usage = "/{{command_name}} [args]"

# Command arguments (optional)
# [[commands.args]]
# name = "arg"
# description = "An argument"
# required = false
# default = "default_value"

# Hooks your plugin registers (optional)
# [[hooks]]
# hook_type = "tool_execute_before"  # or: tool_execute_after, chat_message, permission_ask, etc.
# priority = 100                     # Lower runs first
# pattern = "*"                      # Tool pattern filter

# Plugin configuration schema (optional)
# [config]
# setting_name = { description = "Description", type = "string", default = "value" }

# WASM settings (optional)
[wasm]
memory_pages = 256    # 64KB per page, 256 = 16MB
timeout_ms = 30000    # 30 seconds
"#;

/// Example Rust code template for a plugin.
pub const RUST_TEMPLATE: &str = r#"//! {{plugin_name}} - A Cortex plugin
//!
//! Build with: cargo build --target wasm32-wasi --release

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ============================================================================
// Host function imports
// ============================================================================

#[link(wasm_import_module = "cortex")]
extern "C" {
    /// Log a message at the specified level.
    /// level: 0=trace, 1=debug, 2=info, 3=warn, 4=error
    fn log(level: i32, msg_ptr: i32, msg_len: i32);

    /// Get context JSON (returns length)
    fn get_context() -> i64;
}

// ============================================================================
// Logging helpers
// ============================================================================

fn log_message(level: i32, msg: &str) {
    // SAFETY: FFI call to host-provided `log` function.
    // Contract with the host runtime:
    // 1. `log` is a valid function pointer provided by the WASM runtime during instantiation
    // 2. The host reads the message from WASM linear memory using (ptr, len) immediately
    // 3. The host does not retain the pointer past the call boundary
    // 4. The host handles all memory management on its side (copies data if needed)
    // 5. Invalid level values are handled gracefully by the host (treated as info)
    // 6. The pointer is valid for the duration of this call (Rust string guarantee)
    unsafe {
        log(level, msg.as_ptr() as i32, msg.len() as i32);
    }
}

fn log_trace(msg: &str) { log_message(0, msg); }
fn log_debug(msg: &str) { log_message(1, msg); }
fn log_info(msg: &str) { log_message(2, msg); }
fn log_warn(msg: &str) { log_message(3, msg); }
fn log_error(msg: &str) { log_message(4, msg); }

// ============================================================================
// Plugin lifecycle
// ============================================================================

/// Called when the plugin is initialized.
#[no_mangle]
pub extern "C" fn init() -> i32 {
    log_info("{{plugin_name}} initialized");
    0 // Return 0 for success
}

/// Called when the plugin is shutting down.
#[no_mangle]
pub extern "C" fn shutdown() -> i32 {
    log_info("{{plugin_name}} shutting down");
    0
}

// ============================================================================
// Command handlers
// ============================================================================

/// Handler for the /{{command_name}} command.
#[no_mangle]
pub extern "C" fn cmd_{{command_name_snake}}() -> i32 {
    log_info("{{command_name}} command executed");
    0
}

// ============================================================================
// Hook handlers (if using hooks)
// ============================================================================

// /// Called before a tool is executed.
// #[no_mangle]
// pub extern "C" fn hook_tool_execute_before() -> i32 {
//     log_debug("Tool execute before hook triggered");
//     0 // 0 = continue, 1 = skip, 2 = abort
// }

// ============================================================================
// Panic handler (required for no_std)
// ============================================================================

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// ============================================================================
// Global allocator (required for alloc)
// ============================================================================

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
"#;

/// Cargo.toml template for a plugin.
pub const CARGO_TEMPLATE: &str = r#"[package]
name = "{{plugin_id}}"
version = "0.1.0"
edition = "2021"

# Build for WASM target: cargo build --target wasm32-wasi --release

[lib]
crate-type = ["cdylib"]

[dependencies]
wee_alloc = "0.4"

[profile.release]
opt-level = "s"
lto = true
"#;

/// Generate a plugin manifest from a template.
pub fn generate_manifest(
    plugin_id: &str,
    plugin_name: &str,
    description: &str,
    author: &str,
    command_name: &str,
    command_description: &str,
) -> String {
    MANIFEST_TEMPLATE
        .replace("{{plugin_id}}", plugin_id)
        .replace("{{plugin_name}}", plugin_name)
        .replace("{{description}}", description)
        .replace("{{author}}", author)
        .replace("{{command_name}}", command_name)
        .replace("{{command_description}}", command_description)
}

/// Generate plugin Rust code from a template.
pub fn generate_rust_code(plugin_name: &str, command_name: &str) -> String {
    let command_name_snake = command_name.replace('-', "_");

    RUST_TEMPLATE
        .replace("{{plugin_name}}", plugin_name)
        .replace("{{command_name}}", command_name)
        .replace("{{command_name_snake}}", &command_name_snake)
}

/// Generate Cargo.toml from a template.
pub fn generate_cargo_toml(plugin_id: &str) -> String {
    CARGO_TEMPLATE.replace("{{plugin_id}}", plugin_id)
}

/// Plugin development commands and utilities.
pub struct PluginDev;

impl PluginDev {
    /// Scaffold a new plugin project.
    pub fn scaffold(
        output_dir: &std::path::Path,
        plugin_id: &str,
        plugin_name: &str,
        description: &str,
        author: &str,
    ) -> std::io::Result<()> {
        use std::fs;

        // Create directory structure
        let plugin_dir = output_dir.join(plugin_id);
        let src_dir = plugin_dir.join("src");

        fs::create_dir_all(&src_dir)?;

        // Generate files
        let manifest = generate_manifest(
            plugin_id,
            plugin_name,
            description,
            author,
            "example",
            "An example command",
        );

        let rust_code = generate_rust_code(plugin_name, "example");
        let cargo_toml = generate_cargo_toml(plugin_id);

        // Write files
        fs::write(plugin_dir.join("plugin.toml"), manifest)?;
        fs::write(src_dir.join("lib.rs"), rust_code)?;
        fs::write(plugin_dir.join("Cargo.toml"), cargo_toml)?;

        // Write README
        let readme = format!(
            "# {}\n\n{}\n\n## Building\n\n```bash\ncargo build --target wasm32-wasi --release\n```\n\n## Installing\n\nCopy the compiled WASM and manifest to your Cortex plugins directory:\n\n```bash\nmkdir -p ~/.cortex/plugins/{}\ncp target/wasm32-wasi/release/{}.wasm ~/.cortex/plugins/{}/plugin.wasm\ncp plugin.toml ~/.cortex/plugins/{}/\n```\n",
            plugin_name,
            description,
            plugin_id,
            plugin_id.replace('-', "_"),
            plugin_id,
            plugin_id,
        );
        fs::write(plugin_dir.join("README.md"), readme)?;

        // Write .gitignore
        fs::write(plugin_dir.join(".gitignore"), "target/\n")?;

        Ok(())
    }
}

// ============================================================================
// TypeScript Template
// ============================================================================

/// TypeScript template for plugin development (for JavaScript/TypeScript plugins).
pub const TYPESCRIPT_TEMPLATE: &str = r#"/**
 * {{plugin_name}} - A Cortex Plugin
 * 
 * This template provides a TypeScript-based plugin structure.
 * Compile with: npx tsc && npx wasm-pack build
 */

// Plugin metadata
export const PLUGIN_ID = "{{plugin_id}}";
export const PLUGIN_VERSION = "0.1.0";

// ============================================================================
// Plugin Lifecycle
// ============================================================================

/**
 * Called when the plugin is initialized.
 */
export function init(): number {
    console.log(`${PLUGIN_ID} initialized`);
    return 0;
}

/**
 * Called when the plugin is shutting down.
 */
export function shutdown(): number {
    console.log(`${PLUGIN_ID} shutting down`);
    return 0;
}

// ============================================================================
// Command Handlers
// ============================================================================

/**
 * Handler for the /{{command_name}} command.
 */
export function cmd_{{command_name_snake}}(args: string[]): number {
    console.log("{{command_name}} command executed with args:", args);
    return 0;
}

// ============================================================================
// Hook Handlers
// ============================================================================

/**
 * Called before a tool is executed.
 * Return: 0 = continue, 1 = skip, 2 = abort
 */
export function hook_tool_execute_before(input: ToolExecuteBeforeInput): number {
    console.log(`Tool ${input.tool} about to execute`);
    return 0;
}

/**
 * Called when UI is being rendered.
 */
export function hook_ui_render(input: UiRenderInput): UiRenderOutput {
    return {
        styles: {},
        widgets: [],
        result: "continue"
    };
}

// ============================================================================
// Type Definitions
// ============================================================================

interface ToolExecuteBeforeInput {
    tool: string;
    session_id: string;
    call_id: string;
    args: Record<string, unknown>;
}

interface UiRenderInput {
    session_id: string;
    component: string;
    theme: string;
    dimensions: [number, number];
}

interface UiRenderOutput {
    styles: Record<string, string>;
    widgets: Widget[];
    result: "continue" | "skip" | "abort";
}

interface Widget {
    type: string;
    [key: string]: unknown;
}
"#;

/// tsconfig.json template for TypeScript plugins.
pub const TSCONFIG_TEMPLATE: &str = r#"{
    "compilerOptions": {
        "target": "ES2020",
        "module": "ESNext",
        "moduleResolution": "node",
        "strict": true,
        "esModuleInterop": true,
        "skipLibCheck": true,
        "forceConsistentCasingInFileNames": true,
        "outDir": "./dist",
        "declaration": true
    },
    "include": ["src/**/*"],
    "exclude": ["node_modules", "dist"]
}
"#;

// ============================================================================
// Hot-Reload Configuration
// ============================================================================

/// Hot-reload configuration for plugin development.
pub const HOT_RELOAD_CONFIG: &str = r#"# Hot Reload Configuration
# This file configures hot-reload behavior during plugin development.

[hot_reload]
# Enable hot-reload (set to false in production)
enabled = true

# Watch patterns for file changes
watch_patterns = [
    "src/**/*.rs",
    "src/**/*.ts",
    "plugin.toml"
]

# Debounce time in milliseconds (prevents rapid reloads)
debounce_ms = 500

# Auto-rebuild on change
auto_rebuild = true

# Preserve plugin state on reload (if supported)
preserve_state = false

# Log reload events
log_reloads = true

[build]
# Build command for the plugin
command = "cargo build --target wasm32-wasi --release"

# Output path for the compiled WASM
output = "target/wasm32-wasi/release/{{plugin_id}}.wasm"

# Pre-build commands (optional)
pre_build = []

# Post-build commands (optional)
post_build = []
"#;

// ============================================================================
// Testing Utilities Template
// ============================================================================

/// Testing utilities template for plugin authors.
pub const TEST_UTILS_TEMPLATE: &str = r#"//! Testing utilities for {{plugin_name}}
//!
//! This module provides utilities for testing your plugin.

#![cfg(test)]

use std::collections::HashMap;

/// Mock context for testing plugin functions.
pub struct MockContext {
    pub session_id: String,
    pub config: HashMap<String, String>,
}

impl MockContext {
    pub fn new() -> Self {
        Self {
            session_id: "test-session".to_string(),
            config: HashMap::new(),
        }
    }

    pub fn with_config(mut self, key: &str, value: &str) -> Self {
        self.config.insert(key.to_string(), value.to_string());
        self
    }
}

impl Default for MockContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Mock tool execution input for testing.
pub struct MockToolInput {
    pub tool: String,
    pub args: serde_json::Value,
}

impl MockToolInput {
    pub fn new(tool: &str) -> Self {
        Self {
            tool: tool.to_string(),
            args: serde_json::json!({}),
        }
    }

    pub fn with_arg(mut self, key: &str, value: serde_json::Value) -> Self {
        if let Some(obj) = self.args.as_object_mut() {
            obj.insert(key.to_string(), value);
        }
        self
    }
}

/// Assert that a hook returned the expected result.
#[macro_export]
macro_rules! assert_hook_result {
    ($result:expr, continue) => {
        assert_eq!($result, 0, "Expected hook to continue");
    };
    ($result:expr, skip) => {
        assert_eq!($result, 1, "Expected hook to skip");
    };
    ($result:expr, abort) => {
        assert_eq!($result, 2, "Expected hook to abort");
    };
}

/// Test fixture for widget rendering.
pub struct WidgetTestFixture {
    pub width: u16,
    pub height: u16,
    pub theme: String,
}

impl WidgetTestFixture {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            theme: "ocean".to_string(),
        }
    }

    pub fn with_theme(mut self, theme: &str) -> Self {
        self.theme = theme.to_string();
        self
    }
}

impl Default for WidgetTestFixture {
    fn default() -> Self {
        Self::new(120, 40)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_context() {
        let ctx = MockContext::new()
            .with_config("api_key", "test-key");
        
        assert_eq!(ctx.config.get("api_key"), Some(&"test-key".to_string()));
    }

    #[test]
    fn test_mock_tool_input() {
        let input = MockToolInput::new("read")
            .with_arg("path", serde_json::json!("/test/file.txt"));
        
        assert_eq!(input.tool, "read");
        assert_eq!(input.args["path"], "/test/file.txt");
    }
}
"#;

// ============================================================================
// Advanced Rust Template
// ============================================================================

/// Advanced Rust plugin template with TUI hooks.
pub const RUST_ADVANCED_TEMPLATE: &str = r#"//! {{plugin_name}} - Advanced Cortex Plugin
//!
//! This template demonstrates advanced plugin features including:
//! - TUI customization hooks
//! - Custom widgets
//! - Keyboard bindings
//! - Event handling
//!
//! Build with: cargo build --target wasm32-wasi --release

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;

// ============================================================================
// Host function imports
// ============================================================================

#[link(wasm_import_module = "cortex")]
extern "C" {
    fn log(level: i32, msg_ptr: i32, msg_len: i32);
    fn get_context() -> i64;
    fn register_widget(region: i32, widget_type_ptr: i32, widget_type_len: i32) -> i32;
    fn register_keybinding(key_ptr: i32, key_len: i32, action_ptr: i32, action_len: i32) -> i32;
    fn show_toast(level: i32, msg_ptr: i32, msg_len: i32, duration_ms: i32) -> i32;
    fn emit_event(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
}

// ============================================================================
// Logging helpers
// ============================================================================

fn log_message(level: i32, msg: &str) {
    unsafe {
        log(level, msg.as_ptr() as i32, msg.len() as i32);
    }
}

fn log_info(msg: &str) { log_message(2, msg); }
fn log_debug(msg: &str) { log_message(1, msg); }
fn log_warn(msg: &str) { log_message(3, msg); }
fn log_error(msg: &str) { log_message(4, msg); }

// ============================================================================
// Widget helpers
// ============================================================================

/// UI regions for widget placement
#[repr(i32)]
enum UiRegion {
    Header = 0,
    Footer = 1,
    SidebarLeft = 2,
    SidebarRight = 3,
    StatusBar = 7,
}

fn register_widget_in_region(region: UiRegion, widget_type: &str) -> bool {
    // SAFETY: FFI call to host-provided `register_widget` function.
    // Contract with the host runtime:
    // 1. `register_widget` is a valid function pointer provided by the WASM runtime
    // 2. Arguments are passed by value (region) and by pointer+len (widget_type string)
    // 3. The host copies the string data before this call returns
    // 4. The host validates the region value and handles invalid values gracefully
    // 5. Return value 0 indicates success, non-zero indicates failure
    // 6. The widget_type pointer remains valid for the duration of this call
    unsafe {
        register_widget(
            region as i32,
            widget_type.as_ptr() as i32,
            widget_type.len() as i32,
        ) == 0
    }
}

fn register_key(key: &str, action: &str) -> bool {
    // SAFETY: FFI call to host-provided `register_keybinding` function.
    // Contract with the host runtime:
    // 1. `register_keybinding` is a valid function pointer provided by the WASM runtime
    // 2. Both string arguments are passed as (ptr, len) pairs
    // 3. The host copies both strings before this call returns
    // 4. The host validates the key combination and action name
    // 5. Return value 0 indicates success, non-zero indicates failure
    // 6. Both pointers remain valid for the duration of this call (Rust string guarantee)
    unsafe {
        register_keybinding(
            key.as_ptr() as i32,
            key.len() as i32,
            action.as_ptr() as i32,
            action.len() as i32,
        ) == 0
    }
}

/// Toast notification levels
#[repr(i32)]
enum ToastLevel {
    Info = 0,
    Success = 1,
    Warning = 2,
    Error = 3,
}

fn show_notification(level: ToastLevel, message: &str, duration_ms: i32) {
    // SAFETY: FFI call to host-provided `show_toast` function.
    // Contract with the host runtime:
    // 1. `show_toast` is a valid function pointer provided by the WASM runtime
    // 2. The level is passed by value and validated by the host (invalid = Info)
    // 3. The message string is passed as (ptr, len) and copied by the host
    // 4. duration_ms is passed by value; invalid values are clamped by host
    // 5. The host does not retain the message pointer past this call
    // 6. The function has no return value; failures are logged on the host side
    unsafe {
        show_toast(
            level as i32,
            message.as_ptr() as i32,
            message.len() as i32,
            duration_ms,
        );
    }
}

// ============================================================================
// Plugin lifecycle
// ============================================================================

#[no_mangle]
pub extern "C" fn init() -> i32 {
    log_info("{{plugin_name}} initializing...");
    
    // Register custom widgets
    if register_widget_in_region(UiRegion::StatusBar, "{{plugin_id}}_status") {
        log_debug("Status widget registered");
    }
    
    // Register keyboard bindings
    if register_key("ctrl+shift+p", "{{plugin_id}}_action") {
        log_debug("Keybinding registered: Ctrl+Shift+P");
    }
    
    log_info("{{plugin_name}} initialized successfully");
    0
}

#[no_mangle]
pub extern "C" fn shutdown() -> i32 {
    log_info("{{plugin_name}} shutting down");
    0
}

// ============================================================================
// Command handlers
// ============================================================================

#[no_mangle]
pub extern "C" fn cmd_{{command_name_snake}}() -> i32 {
    log_info("{{command_name}} command executed");
    show_notification(ToastLevel::Info, "Command executed!", 2000);
    0
}

// ============================================================================
// Hook handlers
// ============================================================================

/// UI render hook - customize component rendering
#[no_mangle]
pub extern "C" fn hook_ui_render() -> i32 {
    // Return 0 to continue with normal rendering
    // Modifications are passed through the output buffer
    0
}

/// Animation frame hook - called every frame for animations
#[no_mangle]
pub extern "C" fn hook_animation_frame(_frame: u64, _delta_us: u64) -> i32 {
    // Return 1 to request another frame, 0 to stop
    0
}

/// Focus change hook
#[no_mangle]
pub extern "C" fn hook_focus_change(_focused: i32) -> i32 {
    0
}

/// TUI event handler
#[no_mangle]
pub extern "C" fn hook_tui_event() -> i32 {
    0
}

// ============================================================================
// Custom action handlers
// ============================================================================

#[no_mangle]
pub extern "C" fn action_{{plugin_id_snake}}_action() -> i32 {
    log_info("Custom action triggered via keybinding");
    show_notification(ToastLevel::Success, "Action executed!", 1500);
    0
}

// ============================================================================
// Panic handler
// ============================================================================

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Try to log panic info
    if let Some(location) = info.location() {
        let file = location.file();
        let line = location.line();
        log_error("PANIC occurred");
        let _ = file;
        let _ = line;
    }
    loop {}
}

// ============================================================================
// Global allocator
// ============================================================================

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
"#;

// ============================================================================
// Generator Functions
// ============================================================================

/// Generate hot-reload configuration from a template.
pub fn generate_hot_reload_config(plugin_id: &str) -> String {
    HOT_RELOAD_CONFIG.replace("{{plugin_id}}", plugin_id)
}

/// Generate TypeScript plugin code from a template.
pub fn generate_typescript_code(plugin_id: &str, plugin_name: &str, command_name: &str) -> String {
    let command_name_snake = command_name.replace('-', "_");

    TYPESCRIPT_TEMPLATE
        .replace("{{plugin_id}}", plugin_id)
        .replace("{{plugin_name}}", plugin_name)
        .replace("{{command_name}}", command_name)
        .replace("{{command_name_snake}}", &command_name_snake)
}

/// Generate test utilities template.
pub fn generate_test_utils(plugin_name: &str) -> String {
    TEST_UTILS_TEMPLATE.replace("{{plugin_name}}", plugin_name)
}

/// Generate advanced Rust code with TUI hooks.
pub fn generate_advanced_rust_code(
    plugin_id: &str,
    plugin_name: &str,
    command_name: &str,
) -> String {
    let command_name_snake = command_name.replace('-', "_");
    let plugin_id_snake = plugin_id.replace('-', "_");

    RUST_ADVANCED_TEMPLATE
        .replace("{{plugin_id}}", plugin_id)
        .replace("{{plugin_id_snake}}", &plugin_id_snake)
        .replace("{{plugin_name}}", plugin_name)
        .replace("{{command_name}}", command_name)
        .replace("{{command_name_snake}}", &command_name_snake)
}

// ============================================================================
// Hot-Reload Configuration Struct
// ============================================================================

/// Hot-reload watcher configuration.
#[derive(Debug, Clone)]
pub struct HotReloadConfig {
    /// Whether hot-reload is enabled.
    pub enabled: bool,
    /// File patterns to watch.
    pub watch_patterns: Vec<String>,
    /// Debounce time in milliseconds.
    pub debounce_ms: u64,
    /// Auto-rebuild on change.
    pub auto_rebuild: bool,
    /// Preserve plugin state on reload.
    pub preserve_state: bool,
}

impl Default for HotReloadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            watch_patterns: vec![
                "src/**/*.rs".to_string(),
                "src/**/*.ts".to_string(),
                "plugin.toml".to_string(),
            ],
            debounce_ms: 500,
            auto_rebuild: true,
            preserve_state: false,
        }
    }
}

impl HotReloadConfig {
    /// Create a new hot-reload configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable hot-reload.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set watch patterns.
    pub fn with_patterns(mut self, patterns: Vec<String>) -> Self {
        self.watch_patterns = patterns;
        self
    }

    /// Set debounce time.
    pub fn with_debounce(mut self, ms: u64) -> Self {
        self.debounce_ms = ms;
        self
    }
}

// ============================================================================
// PluginDev Advanced Scaffold
// ============================================================================

impl PluginDev {
    /// Scaffold a new plugin project with advanced features.
    pub fn scaffold_advanced(
        output_dir: &std::path::Path,
        plugin_id: &str,
        plugin_name: &str,
        description: &str,
        author: &str,
        use_typescript: bool,
    ) -> std::io::Result<()> {
        use std::fs;

        // Create directory structure
        let plugin_dir = output_dir.join(plugin_id);
        let src_dir = plugin_dir.join("src");
        let tests_dir = plugin_dir.join("tests");

        fs::create_dir_all(&src_dir)?;
        fs::create_dir_all(&tests_dir)?;

        // Generate manifest
        let manifest = generate_manifest(
            plugin_id,
            plugin_name,
            description,
            author,
            "example",
            "An example command",
        );

        // Write manifest
        fs::write(plugin_dir.join("plugin.toml"), manifest)?;

        // Generate hot-reload config
        let hot_reload = generate_hot_reload_config(plugin_id);
        fs::write(plugin_dir.join("hot-reload.toml"), hot_reload)?;

        if use_typescript {
            // TypeScript project
            let ts_code = generate_typescript_code(plugin_id, plugin_name, "example");
            fs::write(src_dir.join("index.ts"), ts_code)?;
            fs::write(plugin_dir.join("tsconfig.json"), TSCONFIG_TEMPLATE)?;

            // package.json
            let package_json = format!(
                r#"{{
    "name": "{}",
    "version": "0.1.0",
    "description": "{}",
    "main": "dist/index.js",
    "scripts": {{
        "build": "tsc",
        "watch": "tsc --watch"
    }},
    "devDependencies": {{
        "typescript": "^5.0.0"
    }}
}}"#,
                plugin_id, description
            );
            fs::write(plugin_dir.join("package.json"), package_json)?;
        } else {
            // Rust project with advanced template
            let rust_code = generate_advanced_rust_code(plugin_id, plugin_name, "example");
            fs::write(src_dir.join("lib.rs"), rust_code)?;

            // Cargo.toml
            let cargo_toml = generate_cargo_toml(plugin_id);
            fs::write(plugin_dir.join("Cargo.toml"), cargo_toml)?;

            // Test utilities
            let test_utils = generate_test_utils(plugin_name);
            fs::write(tests_dir.join("utils.rs"), test_utils)?;
        }

        // Write README
        let readme = format!(
            r#"# {}

{}

## Features

- Custom widgets and UI customization
- Keyboard bindings
- Event handling
- Hot-reload support for development

## Building

{}

## Development

Enable hot-reload during development:

```bash
cortex plugin dev --watch
```

## Testing

```bash
cargo test
```

## Installing

Copy the compiled WASM and manifest to your Cortex plugins directory:

```bash
mkdir -p ~/.cortex/plugins/{}
cp target/wasm32-wasi/release/{}.wasm ~/.cortex/plugins/{}/plugin.wasm
cp plugin.toml ~/.cortex/plugins/{}/
```
"#,
            plugin_name,
            description,
            if use_typescript {
                "```bash\nnpm install\nnpm run build\n```"
            } else {
                "```bash\ncargo build --target wasm32-wasi --release\n```"
            },
            plugin_id,
            plugin_id.replace('-', "_"),
            plugin_id,
            plugin_id,
        );
        fs::write(plugin_dir.join("README.md"), readme)?;

        // Write .gitignore
        let gitignore = if use_typescript {
            "node_modules/\ndist/\n*.wasm\n"
        } else {
            "target/\n*.wasm\n"
        };
        fs::write(plugin_dir.join(".gitignore"), gitignore)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_manifest() {
        let manifest = generate_manifest(
            "test-plugin",
            "Test Plugin",
            "A test plugin",
            "Test Author",
            "test",
            "A test command",
        );

        assert!(manifest.contains("test-plugin"));
        assert!(manifest.contains("Test Plugin"));
        assert!(manifest.contains("A test plugin"));
    }

    #[test]
    fn test_generate_rust_code() {
        let code = generate_rust_code("Test Plugin", "my-command");

        assert!(code.contains("Test Plugin"));
        assert!(code.contains("my-command"));
        assert!(code.contains("cmd_my_command"));
    }

    #[test]
    fn test_generate_cargo_toml() {
        let cargo = generate_cargo_toml("my-plugin");

        assert!(cargo.contains("my-plugin"));
        assert!(cargo.contains("wasm32-wasi"));
    }

    #[test]
    fn test_generate_hot_reload_config() {
        let config = generate_hot_reload_config("my-plugin");
        assert!(config.contains("my-plugin"));
        assert!(config.contains("enabled = true"));
    }

    #[test]
    fn test_generate_typescript_code() {
        let code = generate_typescript_code("my-plugin", "My Plugin", "my-command");
        assert!(code.contains("My Plugin"));
        assert!(code.contains("my-command"));
        assert!(code.contains("cmd_my_command"));
    }

    #[test]
    fn test_generate_advanced_rust_code() {
        let code = generate_advanced_rust_code("my-plugin", "My Plugin", "example");
        assert!(code.contains("My Plugin"));
        assert!(code.contains("register_widget"));
        assert!(code.contains("register_keybinding"));
    }

    #[test]
    fn test_hot_reload_config() {
        let config = HotReloadConfig::new().with_enabled(true).with_debounce(300);

        assert!(config.enabled);
        assert_eq!(config.debounce_ms, 300);
    }
}
