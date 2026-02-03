//! Plugin management command for Cortex CLI.
//!
//! Provides plugin management functionality:
//! - List installed plugins
//! - Install plugins
//! - Remove plugins
//! - Enable/disable plugins
//! - Show plugin info
//! - Create new plugin projects
//! - Development mode with hot-reload
//! - Build plugin WASM files
//! - Validate plugin manifests
//! - Publish plugins (dry-run)

use anyhow::{Context, Result, bail};
use clap::Parser;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

// =============================================================================
// Plugin SDK Templates (embedded for standalone CLI operation)
// =============================================================================

/// Manifest template for new plugins.
const MANIFEST_TEMPLATE: &str = r#"[plugin]
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

/// Basic Rust template for plugins.
const RUST_TEMPLATE: &str = r#"//! {{plugin_name}} - A Cortex plugin
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

fn log_info(msg: &str) { log_message(2, msg); }

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

/// Advanced Rust template with TUI hooks.
const RUST_ADVANCED_TEMPLATE: &str = r#"//! {{plugin_name}} - Advanced Cortex Plugin
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
    // SAFETY: FFI call to host-provided `log` function.
    // The host reads the message immediately and does not retain the pointer.
    unsafe {
        log(level, msg.as_ptr() as i32, msg.len() as i32);
    }
}

fn log_info(msg: &str) { log_message(2, msg); }
fn log_debug(msg: &str) { log_message(1, msg); }

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
    // Arguments are passed by value (region) and by pointer+len (widget_type string).
    // The host copies the string data before this call returns.
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
    // Both string arguments are passed as (ptr, len) pairs and copied by the host.
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
    // The message string is copied by the host before this call returns.
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
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// ============================================================================
// Global allocator
// ============================================================================

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
"#;

/// Cargo.toml template for plugins.
const CARGO_TEMPLATE: &str = r#"[package]
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

/// TypeScript template for plugins.
const TYPESCRIPT_TEMPLATE: &str = r#"/**
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

// ============================================================================
// Type Definitions
// ============================================================================

interface ToolExecuteBeforeInput {
    tool: string;
    session_id: string;
    call_id: string;
    args: Record<string, unknown>;
}
"#;

/// tsconfig.json template.
const TSCONFIG_TEMPLATE: &str = r#"{
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

/// Plugin CLI command.
#[derive(Debug, Parser)]
pub struct PluginCli {
    #[command(subcommand)]
    pub subcommand: PluginSubcommand,
}

/// Plugin subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum PluginSubcommand {
    /// List installed plugins
    #[command(visible_alias = "ls")]
    List(PluginListArgs),

    /// Install a plugin
    #[command(visible_alias = "add")]
    Install(PluginInstallArgs),

    /// Remove a plugin
    #[command(visible_aliases = ["rm", "uninstall"])]
    Remove(PluginRemoveArgs),

    /// Enable a plugin
    Enable(PluginEnableArgs),

    /// Disable a plugin
    Disable(PluginDisableArgs),

    /// Show plugin information
    #[command(visible_alias = "info")]
    Show(PluginShowArgs),

    /// Create a new plugin project
    #[command(visible_alias = "create")]
    New(PluginNewArgs),

    /// Start development mode with hot-reload
    Dev(PluginDevArgs),

    /// Build the plugin WASM file
    Build(PluginBuildArgs),

    /// Validate plugin manifest and structure
    #[command(visible_alias = "check")]
    Validate(PluginValidateArgs),

    /// Prepare plugin for publication (dry-run)
    Publish(PluginPublishArgs),
}

/// Arguments for plugin list command.
#[derive(Debug, Parser)]
pub struct PluginListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show only enabled plugins
    #[arg(long)]
    pub enabled: bool,

    /// Show only disabled plugins
    #[arg(long)]
    pub disabled: bool,
}

/// Arguments for plugin install command.
#[derive(Debug, Parser)]
pub struct PluginInstallArgs {
    /// Plugin name or URL to install
    pub name: String,

    /// Plugin version (defaults to latest)
    #[arg(long, short = 'v')]
    pub version: Option<String>,

    /// Force reinstall if already installed
    #[arg(long, short = 'f')]
    pub force: bool,
}

/// Arguments for plugin remove command.
#[derive(Debug, Parser)]
pub struct PluginRemoveArgs {
    /// Plugin name to remove
    pub name: String,

    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for plugin enable command.
#[derive(Debug, Parser)]
pub struct PluginEnableArgs {
    /// Plugin name to enable
    pub name: String,
}

/// Arguments for plugin disable command.
#[derive(Debug, Parser)]
pub struct PluginDisableArgs {
    /// Plugin name to disable
    pub name: String,
}

/// Arguments for plugin show command.
#[derive(Debug, Parser)]
pub struct PluginShowArgs {
    /// Plugin name to show
    pub name: String,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for plugin new command.
#[derive(Debug, Parser)]
pub struct PluginNewArgs {
    /// Plugin name (will be used as directory name and ID)
    pub name: String,

    /// Plugin description
    #[arg(long, short = 'd', default_value = "A Cortex plugin")]
    pub description: String,

    /// Plugin author
    #[arg(long, short = 'a')]
    pub author: Option<String>,

    /// Output directory (defaults to current directory)
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,

    /// Use advanced template with TUI hooks
    #[arg(long)]
    pub advanced: bool,

    /// Use TypeScript template instead of Rust
    #[arg(long)]
    pub typescript: bool,
}

/// Arguments for plugin dev command.
#[derive(Debug, Parser)]
pub struct PluginDevArgs {
    /// Plugin directory (defaults to current directory)
    #[arg(long, short = 'p')]
    pub path: Option<PathBuf>,

    /// Watch for file changes and auto-rebuild
    #[arg(long, short = 'w')]
    pub watch: bool,

    /// Debounce time in milliseconds for file change events
    #[arg(long, default_value = "500")]
    pub debounce_ms: u64,
}

/// Arguments for plugin build command.
#[derive(Debug, Parser)]
pub struct PluginBuildArgs {
    /// Plugin directory (defaults to current directory)
    #[arg(long, short = 'p')]
    pub path: Option<PathBuf>,

    /// Build in debug mode (faster, larger output)
    #[arg(long)]
    pub debug: bool,

    /// Output directory for the compiled WASM file
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
}

/// Arguments for plugin validate command.
#[derive(Debug, Parser)]
pub struct PluginValidateArgs {
    /// Plugin directory (defaults to current directory)
    #[arg(long, short = 'p')]
    pub path: Option<PathBuf>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show verbose output with all checks
    #[arg(long, short = 'v')]
    pub verbose: bool,
}

/// Arguments for plugin publish command.
#[derive(Debug, Parser)]
pub struct PluginPublishArgs {
    /// Plugin directory (defaults to current directory)
    #[arg(long, short = 'p')]
    pub path: Option<PathBuf>,

    /// Dry-run mode (default, no actual publishing)
    #[arg(long, default_value = "true")]
    pub dry_run: bool,

    /// Output tarball path (defaults to plugin-name-version.tar.gz)
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
}

/// Plugin information for display.
#[derive(Debug, Serialize)]
struct PluginInfo {
    name: String,
    version: String,
    description: String,
    enabled: bool,
    path: PathBuf,
}

/// Get the plugins directory.
fn get_plugins_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".cortex").join("plugins"))
        .unwrap_or_else(|| PathBuf::from(".cortex/plugins"))
}

// =============================================================================
// Plugin Scaffolding Functions
// =============================================================================

/// Generate a manifest from the template.
fn generate_manifest(
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

/// Generate basic Rust plugin code.
fn generate_rust_code(plugin_name: &str, command_name: &str) -> String {
    let command_name_snake = command_name.replace('-', "_");
    RUST_TEMPLATE
        .replace("{{plugin_name}}", plugin_name)
        .replace("{{command_name}}", command_name)
        .replace("{{command_name_snake}}", &command_name_snake)
}

/// Generate advanced Rust plugin code with TUI hooks.
fn generate_advanced_rust_code(plugin_id: &str, plugin_name: &str, command_name: &str) -> String {
    let command_name_snake = command_name.replace('-', "_");
    let plugin_id_snake = plugin_id.replace('-', "_");
    RUST_ADVANCED_TEMPLATE
        .replace("{{plugin_id}}", plugin_id)
        .replace("{{plugin_id_snake}}", &plugin_id_snake)
        .replace("{{plugin_name}}", plugin_name)
        .replace("{{command_name}}", command_name)
        .replace("{{command_name_snake}}", &command_name_snake)
}

/// Generate Cargo.toml for a plugin.
fn generate_cargo_toml(plugin_id: &str) -> String {
    CARGO_TEMPLATE.replace("{{plugin_id}}", plugin_id)
}

/// Generate TypeScript plugin code.
fn generate_typescript_code(plugin_id: &str, plugin_name: &str, command_name: &str) -> String {
    let command_name_snake = command_name.replace('-', "_");
    TYPESCRIPT_TEMPLATE
        .replace("{{plugin_id}}", plugin_id)
        .replace("{{plugin_name}}", plugin_name)
        .replace("{{command_name}}", command_name)
        .replace("{{command_name_snake}}", &command_name_snake)
}

/// Scaffold a basic plugin project.
fn scaffold_basic_plugin(
    output_dir: &Path,
    plugin_id: &str,
    plugin_name: &str,
    description: &str,
    author: &str,
) -> std::io::Result<()> {
    use std::fs;

    let plugin_dir = output_dir.join(plugin_id);
    let src_dir = plugin_dir.join("src");

    fs::create_dir_all(&src_dir)?;

    // Generate manifest
    let manifest = generate_manifest(
        plugin_id,
        plugin_name,
        description,
        author,
        "example",
        "An example command",
    );

    // Generate Rust code
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

/// Scaffold an advanced plugin project with optional TypeScript support.
fn scaffold_advanced_plugin(
    output_dir: &Path,
    plugin_id: &str,
    plugin_name: &str,
    description: &str,
    author: &str,
    use_typescript: bool,
) -> std::io::Result<()> {
    use std::fs;

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
    fs::write(plugin_dir.join("plugin.toml"), manifest)?;

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

        // gitignore for TypeScript
        fs::write(
            plugin_dir.join(".gitignore"),
            "node_modules/\ndist/\n*.wasm\n",
        )?;
    } else {
        // Rust project with advanced template
        let rust_code = generate_advanced_rust_code(plugin_id, plugin_name, "example");
        fs::write(src_dir.join("lib.rs"), rust_code)?;

        // Cargo.toml
        let cargo_toml = generate_cargo_toml(plugin_id);
        fs::write(plugin_dir.join("Cargo.toml"), cargo_toml)?;

        // gitignore for Rust
        fs::write(plugin_dir.join(".gitignore"), "target/\n*.wasm\n")?;
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

    Ok(())
}

impl PluginCli {
    /// Run the plugin command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            PluginSubcommand::List(args) => run_list(args).await,
            PluginSubcommand::Install(args) => run_install(args).await,
            PluginSubcommand::Remove(args) => run_remove(args).await,
            PluginSubcommand::Enable(args) => run_enable(args).await,
            PluginSubcommand::Disable(args) => run_disable(args).await,
            PluginSubcommand::Show(args) => run_show(args).await,
            PluginSubcommand::New(args) => run_new(args).await,
            PluginSubcommand::Dev(args) => run_dev(args).await,
            PluginSubcommand::Build(args) => run_build(args).await,
            PluginSubcommand::Validate(args) => run_validate(args).await,
            PluginSubcommand::Publish(args) => run_publish(args).await,
        }
    }
}

async fn run_list(args: PluginListArgs) -> Result<()> {
    // Validate mutually exclusive flags
    if args.enabled && args.disabled {
        bail!(
            "Cannot specify both --enabled and --disabled. Choose one filter or use neither for all plugins."
        );
    }

    let plugins_dir = get_plugins_dir();

    if !plugins_dir.exists() {
        if args.json {
            println!("[]");
        } else {
            println!("No plugins installed.");
            println!("\nPlugin directory: {}", plugins_dir.display());
            println!("Use 'cortex plugin install <name>' to install a plugin.");
        }
        return Ok(());
    }

    let mut plugins = Vec::new();

    // Scan plugins directory
    if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("plugin.toml");
                if manifest_path.exists()
                    && let Ok(content) = std::fs::read_to_string(&manifest_path)
                    && let Ok(manifest) = toml::from_str::<toml::Value>(&content)
                {
                    let name = manifest
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_else(|| {
                            path.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("unknown")
                        })
                        .to_string();

                    let version = manifest
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0.0.0")
                        .to_string();

                    let description = manifest
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let enabled = manifest
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);

                    // Apply filters
                    if args.enabled && !enabled {
                        continue;
                    }
                    if args.disabled && enabled {
                        continue;
                    }

                    plugins.push(PluginInfo {
                        name,
                        version,
                        description,
                        enabled,
                        path,
                    });
                }
            }
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&plugins)?);
    } else if plugins.is_empty() {
        println!("No plugins installed.");
        println!("\nPlugin directory: {}", plugins_dir.display());
        println!("Use 'cortex plugin install <name>' to install a plugin.");
    } else {
        println!("Installed Plugins:");
        println!("{}", "-".repeat(60));
        for plugin in &plugins {
            let status = if plugin.enabled {
                "enabled"
            } else {
                "disabled"
            };
            println!("  {} v{} [{}]", plugin.name, plugin.version, status);
            if !plugin.description.is_empty() {
                println!("    {}", plugin.description);
            }
        }
        println!("\nTotal: {} plugin(s)", plugins.len());
    }

    Ok(())
}

async fn run_install(args: PluginInstallArgs) -> Result<()> {
    // Validate plugin name is not empty (Issue #3700)
    if args.name.trim().is_empty() {
        bail!("Plugin name cannot be empty. Please provide a valid plugin name.");
    }

    let plugins_dir = get_plugins_dir();

    // Create plugins directory if it doesn't exist
    if !plugins_dir.exists() {
        std::fs::create_dir_all(&plugins_dir)?;
    }

    let plugin_path = plugins_dir.join(&args.name);

    if plugin_path.exists() && !args.force {
        bail!(
            "Plugin '{}' is already installed. Use --force to reinstall.",
            args.name
        );
    }

    println!("Installing plugin: {}", args.name);
    if let Some(ref version) = args.version {
        println!("  Version: {}", version);
    }

    // For now, we support local directory installation or create a placeholder
    // In a full implementation, this would fetch from a plugin registry
    if std::path::Path::new(&args.name).exists() {
        // Install from local path
        let src_path = std::path::Path::new(&args.name);
        if src_path.is_dir() {
            // Copy directory
            copy_dir_recursive(src_path, &plugin_path)?;
            println!("Plugin installed from local directory.");
        } else {
            bail!("Source path is not a directory: {}", args.name);
        }
    } else {
        // Create placeholder plugin structure
        std::fs::create_dir_all(&plugin_path)?;

        let version = args.version.as_deref().unwrap_or("1.0.0");
        let manifest = format!(
            r#"# Plugin manifest
name = "{}"
version = "{}"
description = "Placeholder plugin - replace with actual implementation"
enabled = true
"#,
            args.name, version
        );

        std::fs::write(plugin_path.join("plugin.toml"), manifest)?;
        println!("Created placeholder plugin structure.");
        println!("Edit {} to configure your plugin.", plugin_path.display());
    }

    println!("Plugin '{}' installed successfully.", args.name);
    Ok(())
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

async fn run_remove(args: PluginRemoveArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();
    let plugin_path = plugins_dir.join(&args.name);

    if !plugin_path.exists() {
        bail!("Plugin '{}' is not installed.", args.name);
    }

    if !args.yes {
        println!(
            "Are you sure you want to remove plugin '{}'? (y/N)",
            args.name
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    std::fs::remove_dir_all(&plugin_path)?;
    println!("Plugin '{}' removed successfully.", args.name);
    Ok(())
}

async fn run_enable(args: PluginEnableArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();
    let plugin_path = plugins_dir.join(&args.name);
    let manifest_path = plugin_path.join("plugin.toml");

    if !manifest_path.exists() {
        bail!("Plugin '{}' is not installed.", args.name);
    }

    let content = std::fs::read_to_string(&manifest_path)?;
    let mut manifest: toml::Value = toml::from_str(&content)?;

    if let Some(table) = manifest.as_table_mut() {
        table.insert("enabled".to_string(), toml::Value::Boolean(true));
    }

    std::fs::write(&manifest_path, toml::to_string_pretty(&manifest)?)?;
    println!("Plugin '{}' enabled.", args.name);
    Ok(())
}

async fn run_disable(args: PluginDisableArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();
    let plugin_path = plugins_dir.join(&args.name);
    let manifest_path = plugin_path.join("plugin.toml");

    if !manifest_path.exists() {
        bail!("Plugin '{}' is not installed.", args.name);
    }

    let content = std::fs::read_to_string(&manifest_path)?;
    let mut manifest: toml::Value = toml::from_str(&content)?;

    if let Some(table) = manifest.as_table_mut() {
        table.insert("enabled".to_string(), toml::Value::Boolean(false));
    }

    std::fs::write(&manifest_path, toml::to_string_pretty(&manifest)?)?;
    println!("Plugin '{}' disabled.", args.name);
    Ok(())
}

async fn run_show(args: PluginShowArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();
    let plugin_path = plugins_dir.join(&args.name);
    let manifest_path = plugin_path.join("plugin.toml");

    if !manifest_path.exists() {
        if args.json {
            let error = serde_json::json!({
                "error": format!("Plugin '{}' is not installed", args.name)
            });
            println!("{}", serde_json::to_string_pretty(&error)?);
            // Exit with error code but don't duplicate error message via bail!()
            std::process::exit(1);
        }
        bail!("Plugin '{}' is not installed.", args.name);
    }

    let content = std::fs::read_to_string(&manifest_path)?;
    let manifest: toml::Value = toml::from_str(&content)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
    } else {
        println!("Plugin: {}", args.name);
        println!("{}", "-".repeat(40));

        if let Some(version) = manifest.get("version").and_then(|v| v.as_str()) {
            println!("  Version:     {}", version);
        }

        if let Some(description) = manifest.get("description").and_then(|v| v.as_str()) {
            println!("  Description: {}", description);
        }

        if let Some(enabled) = manifest.get("enabled").and_then(|v| v.as_bool()) {
            println!("  Enabled:     {}", enabled);
        }

        if let Some(author) = manifest.get("author").and_then(|v| v.as_str()) {
            println!("  Author:      {}", author);
        }

        println!("  Path:        {}", plugin_path.display());
    }

    Ok(())
}

// =============================================================================
// New Plugin Command Implementation
// =============================================================================

async fn run_new(args: PluginNewArgs) -> Result<()> {
    let output_dir = args
        .output
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Validate plugin name
    if args.name.is_empty() {
        bail!("Plugin name cannot be empty");
    }

    if !args
        .name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        bail!("Plugin name can only contain alphanumeric characters, hyphens, and underscores");
    }

    let plugin_dir = output_dir.join(&args.name);

    if plugin_dir.exists() {
        bail!(
            "Directory '{}' already exists. Choose a different name or remove the existing directory.",
            plugin_dir.display()
        );
    }

    // Determine author
    let author = args.author.unwrap_or_else(|| {
        // Try to get from git config
        Command::new("git")
            .args(["config", "user.name"])
            .output()
            .ok()
            .and_then(|output| {
                if output.status.success() {
                    String::from_utf8(output.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "Plugin Author".to_string())
    });

    // Create human-readable name from plugin ID
    let plugin_name = args
        .name
        .split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    println!("Creating new plugin: {}", plugin_name);
    println!("  Directory: {}", plugin_dir.display());
    println!("  Author: {}", author);

    if args.advanced {
        println!("  Template: Advanced (with TUI hooks)");
        scaffold_advanced_plugin(
            &output_dir,
            &args.name,
            &plugin_name,
            &args.description,
            &author,
            args.typescript,
        )
        .context("Failed to scaffold advanced plugin")?;
    } else if args.typescript {
        println!("  Template: TypeScript");
        scaffold_advanced_plugin(
            &output_dir,
            &args.name,
            &plugin_name,
            &args.description,
            &author,
            true,
        )
        .context("Failed to scaffold TypeScript plugin")?;
    } else {
        println!("  Template: Basic Rust");
        scaffold_basic_plugin(
            &output_dir,
            &args.name,
            &plugin_name,
            &args.description,
            &author,
        )
        .context("Failed to scaffold plugin")?;
    }

    println!("\nPlugin created successfully!");
    println!("\nNext steps:");
    println!("  cd {}", args.name);
    if args.typescript {
        println!("  npm install");
        println!("  npm run build");
    } else {
        println!("  cargo build --target wasm32-wasi --release");
    }
    println!("  cortex plugin validate");
    println!("  cortex plugin dev --watch");

    Ok(())
}

// =============================================================================
// Dev Command Implementation
// =============================================================================

async fn run_dev(args: PluginDevArgs) -> Result<()> {
    let plugin_dir = args
        .path
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Verify this is a plugin directory
    let manifest_path = plugin_dir.join("plugin.toml");
    if !manifest_path.exists() {
        bail!(
            "No plugin.toml found in {}. Are you in a plugin directory?",
            plugin_dir.display()
        );
    }

    // Read plugin info
    let manifest_content =
        std::fs::read_to_string(&manifest_path).context("Failed to read plugin.toml")?;
    let manifest: toml::Value =
        toml::from_str(&manifest_content).context("Failed to parse plugin.toml")?;

    let plugin_name = manifest
        .get("plugin")
        .and_then(|p| p.get("name"))
        .or_else(|| manifest.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    println!("Starting development mode for plugin: {}", plugin_name);
    println!("  Directory: {}", plugin_dir.display());

    // Initial build
    println!("\nRunning initial build...");
    let build_result = run_plugin_build(&plugin_dir, false, None);
    match build_result {
        Ok(wasm_path) => println!("Build successful: {}", wasm_path.display()),
        Err(e) => println!("Build failed: {}", e),
    }

    if !args.watch {
        println!("\nDevelopment mode started (non-watch).");
        println!("Run with --watch to auto-rebuild on file changes.");
        return Ok(());
    }

    println!("\nWatching for file changes (press Ctrl+C to stop)...");

    let (tx, rx) = mpsc::channel();
    let debounce_duration = Duration::from_millis(args.debounce_ms);

    let mut watcher: RecommendedWatcher = Watcher::new(
        move |result: Result<Event, notify::Error>| {
            if let Ok(event) = result {
                let _ = tx.send(event);
            }
        },
        notify::Config::default().with_poll_interval(Duration::from_millis(100)),
    )
    .context("Failed to create file watcher")?;

    // Watch src directory if it exists, otherwise watch the plugin directory
    let src_dir = plugin_dir.join("src");
    let watch_path = if src_dir.exists() {
        &src_dir
    } else {
        &plugin_dir
    };

    watcher
        .watch(watch_path, RecursiveMode::Recursive)
        .context("Failed to start watching directory")?;

    // Also watch plugin.toml
    watcher
        .watch(&manifest_path, RecursiveMode::NonRecursive)
        .context("Failed to watch plugin.toml")?;

    println!("  Watching: {}", watch_path.display());

    let mut last_rebuild = std::time::Instant::now();

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                // Filter for relevant file changes
                let is_relevant = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) && event.paths.iter().any(|p| {
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    matches!(ext, "rs" | "ts" | "toml" | "js")
                });

                if is_relevant && last_rebuild.elapsed() >= debounce_duration {
                    let changed_files: Vec<_> = event
                        .paths
                        .iter()
                        .filter_map(|p| p.file_name())
                        .filter_map(|n| n.to_str())
                        .collect();

                    println!(
                        "\n[{}] File changed: {:?}",
                        chrono::Local::now().format("%H:%M:%S"),
                        changed_files
                    );
                    println!("Rebuilding...");

                    match run_plugin_build(&plugin_dir, false, None) {
                        Ok(wasm_path) => {
                            println!("Build successful: {}", wasm_path.display());
                        }
                        Err(e) => {
                            println!("Build failed: {}", e);
                        }
                    }

                    last_rebuild = std::time::Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Continue waiting
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    Ok(())
}

// =============================================================================
// Build Command Implementation
// =============================================================================

/// Internal function to build a plugin.
fn run_plugin_build(plugin_dir: &Path, debug: bool, output: Option<PathBuf>) -> Result<PathBuf> {
    let manifest_path = plugin_dir.join("plugin.toml");
    let cargo_toml_path = plugin_dir.join("Cargo.toml");

    if !manifest_path.exists() {
        bail!("No plugin.toml found in {}", plugin_dir.display());
    }

    // Check if this is a Rust or TypeScript plugin
    let is_rust = cargo_toml_path.exists();
    let package_json_path = plugin_dir.join("package.json");
    let is_typescript = package_json_path.exists();

    if !is_rust && !is_typescript {
        bail!("No Cargo.toml or package.json found. Cannot determine build system.");
    }

    // Read plugin ID from plugin.toml for output naming
    let manifest_content = std::fs::read_to_string(&manifest_path)?;
    let manifest: toml::Value = toml::from_str(&manifest_content)?;

    let plugin_id = manifest
        .get("plugin")
        .and_then(|p| p.get("id"))
        .or_else(|| manifest.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("plugin");

    let wasm_filename = format!("{}.wasm", plugin_id.replace('-', "_"));

    if is_rust {
        // Build with cargo
        let mut cmd = Command::new("cargo");
        cmd.arg("build")
            .arg("--target")
            .arg("wasm32-wasi")
            .current_dir(plugin_dir);

        if !debug {
            cmd.arg("--release");
        }

        println!(
            "  Running: cargo build --target wasm32-wasi {}",
            if debug { "" } else { "--release" }
        );

        let output_result = cmd
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("Failed to execute cargo build")?;

        if !output_result.success() {
            bail!(
                "Cargo build failed with exit code: {:?}",
                output_result.code()
            );
        }

        // Locate the built WASM file
        let profile_dir = if debug { "debug" } else { "release" };
        let wasm_source = plugin_dir
            .join("target")
            .join("wasm32-wasi")
            .join(profile_dir)
            .join(&wasm_filename);

        if !wasm_source.exists() {
            // Try to find any .wasm file
            let target_dir = plugin_dir
                .join("target")
                .join("wasm32-wasi")
                .join(profile_dir);
            if let Ok(entries) = std::fs::read_dir(&target_dir) {
                for entry in entries.flatten() {
                    if entry
                        .path()
                        .extension()
                        .map(|e| e == "wasm")
                        .unwrap_or(false)
                    {
                        let found_wasm = entry.path();
                        let output_path = output
                            .clone()
                            .unwrap_or_else(|| plugin_dir.join("plugin.wasm"));
                        std::fs::copy(&found_wasm, &output_path)
                            .context("Failed to copy WASM file")?;
                        return Ok(output_path);
                    }
                }
            }
            bail!(
                "WASM file not found at expected path: {}",
                wasm_source.display()
            );
        }

        // Copy to output location
        let output_path = output.unwrap_or_else(|| plugin_dir.join("plugin.wasm"));
        std::fs::copy(&wasm_source, &output_path).context("Failed to copy WASM file to output")?;

        Ok(output_path)
    } else {
        // TypeScript build
        println!("  Running: npm run build");

        let npm_result = Command::new("npm")
            .arg("run")
            .arg("build")
            .current_dir(plugin_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("Failed to execute npm build")?;

        if !npm_result.success() {
            bail!("npm build failed with exit code: {:?}", npm_result.code());
        }

        // For TypeScript plugins, check dist directory
        let dist_path = plugin_dir.join("dist");
        if dist_path.exists() {
            println!("Build output: {}", dist_path.display());
            Ok(dist_path)
        } else {
            Ok(plugin_dir.join("dist"))
        }
    }
}

async fn run_build(args: PluginBuildArgs) -> Result<()> {
    let plugin_dir = args
        .path
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    println!("Building plugin in: {}", plugin_dir.display());

    let wasm_path = run_plugin_build(&plugin_dir, args.debug, args.output)?;

    println!("\nBuild complete!");
    println!("  Output: {}", wasm_path.display());

    // Show file size
    if let Ok(metadata) = std::fs::metadata(&wasm_path) {
        let size = metadata.len();
        if size >= 1024 * 1024 {
            println!("  Size: {:.2} MB", size as f64 / (1024.0 * 1024.0));
        } else if size >= 1024 {
            println!("  Size: {:.2} KB", size as f64 / 1024.0);
        } else {
            println!("  Size: {} bytes", size);
        }
    }

    Ok(())
}

// =============================================================================
// Validate Command Implementation
// =============================================================================

/// Validation issue severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ValidationSeverity {
    Error,
    Warning,
    Info,
}

/// A validation issue found in the plugin.
#[derive(Debug, Serialize)]
struct ValidationIssue {
    severity: ValidationSeverity,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
}

/// Plugin validation result.
#[derive(Debug, Serialize)]
struct ValidationResult {
    valid: bool,
    plugin_id: Option<String>,
    issues: Vec<ValidationIssue>,
}

async fn run_validate(args: PluginValidateArgs) -> Result<()> {
    let plugin_dir = args
        .path
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let manifest_path = plugin_dir.join("plugin.toml");

    let mut result = ValidationResult {
        valid: true,
        plugin_id: None,
        issues: Vec::new(),
    };

    // Check plugin.toml exists
    if !manifest_path.exists() {
        result.valid = false;
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Error,
            message: format!("plugin.toml not found in {}", plugin_dir.display()),
            field: None,
        });

        return output_validation_result(result, args.json);
    }

    // Parse plugin.toml
    let manifest_content = match std::fs::read_to_string(&manifest_path) {
        Ok(content) => content,
        Err(e) => {
            result.valid = false;
            result.issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                message: format!("Failed to read plugin.toml: {}", e),
                field: None,
            });
            return output_validation_result(result, args.json);
        }
    };

    let manifest: toml::Value = match toml::from_str(&manifest_content) {
        Ok(m) => m,
        Err(e) => {
            result.valid = false;
            result.issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                message: format!("Invalid TOML syntax: {}", e),
                field: None,
            });
            return output_validation_result(result, args.json);
        }
    };

    // Get plugin section (may be at root or under [plugin])
    let plugin_section = manifest.get("plugin").unwrap_or(&manifest);

    // Validate required fields
    let plugin_id = plugin_section.get("id").or_else(|| manifest.get("name"));
    if let Some(id) = plugin_id.and_then(|v| v.as_str()) {
        result.plugin_id = Some(id.to_string());

        if id.is_empty() {
            result.valid = false;
            result.issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                message: "Plugin ID cannot be empty".to_string(),
                field: Some("id".to_string()),
            });
        }
    } else {
        result.valid = false;
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Error,
            message: "Missing required field: id or name".to_string(),
            field: Some("id".to_string()),
        });
    }

    // Validate version
    if plugin_section
        .get("version")
        .and_then(|v| v.as_str())
        .is_none()
    {
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Warning,
            message: "Missing version field (defaults to 0.0.0)".to_string(),
            field: Some("version".to_string()),
        });
    }

    // Validate description
    if plugin_section
        .get("description")
        .and_then(|v| v.as_str())
        .is_none()
        && args.verbose
    {
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Info,
            message: "Consider adding a description".to_string(),
            field: Some("description".to_string()),
        });
    }

    // Check for WASM file
    let wasm_path = plugin_dir.join("plugin.wasm");
    let has_wasm = wasm_path.exists();

    // Also check in target directory
    let target_wasm = plugin_dir
        .join("target")
        .join("wasm32-wasi")
        .join("release");

    let has_built_wasm = if target_wasm.exists() {
        std::fs::read_dir(&target_wasm)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "wasm")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    } else {
        false
    };

    if !has_wasm && !has_built_wasm {
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Warning,
            message: "No WASM file found. Run 'cortex plugin build' to compile.".to_string(),
            field: None,
        });
    } else if args.verbose {
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Info,
            message: format!(
                "WASM file found: {}",
                if has_wasm {
                    "plugin.wasm"
                } else {
                    "target/wasm32-wasi/release/*.wasm"
                }
            ),
            field: None,
        });
    }

    // Validate permissions if present
    if let Some(perms_array) = manifest
        .get("permissions")
        .or_else(|| plugin_section.get("permissions"))
        .and_then(|p| p.as_array())
    {
        validate_permissions(perms_array, &mut result, args.verbose);
    }

    // Validate capabilities if present
    if let Some(caps_array) = manifest
        .get("capabilities")
        .or_else(|| plugin_section.get("capabilities"))
        .and_then(|c| c.as_array())
    {
        validate_capabilities(caps_array, &mut result, args.verbose);
    }

    // Check for source files
    let src_dir = plugin_dir.join("src");
    let cargo_toml = plugin_dir.join("Cargo.toml");
    let package_json = plugin_dir.join("package.json");

    if !src_dir.exists() && cargo_toml.exists() {
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Warning,
            message: "src/ directory not found but Cargo.toml exists".to_string(),
            field: None,
        });
    }

    if !cargo_toml.exists() && !package_json.exists() {
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Warning,
            message: "No Cargo.toml or package.json found. Build configuration missing."
                .to_string(),
            field: None,
        });
    }

    output_validation_result(result, args.json)
}

fn validate_permissions(permissions: &[toml::Value], result: &mut ValidationResult, verbose: bool) {
    const KNOWN_PERMISSION_TYPES: &[&str] = &[
        "read_file",
        "write_file",
        "execute",
        "network",
        "filesystem",
        "shell",
        "env",
    ];

    let mut permission_count = 0;

    for perm in permissions {
        if let Some(table) = perm.as_table() {
            for key in table.keys() {
                permission_count += 1;

                if !KNOWN_PERMISSION_TYPES.contains(&key.as_str()) {
                    result.issues.push(ValidationIssue {
                        severity: ValidationSeverity::Warning,
                        message: format!("Unknown permission type: '{}'", key),
                        field: Some("permissions".to_string()),
                    });
                }

                // Check for overly broad permissions
                if let Some(paths) = table
                    .get(key)
                    .and_then(|v| v.get("paths"))
                    .and_then(|p| p.as_array())
                {
                    for path in paths {
                        let is_overly_broad = path
                            .as_str()
                            .map(|p| p == "**/*" || p == "**" || p == "*")
                            .unwrap_or(false);
                        if is_overly_broad {
                            result.issues.push(ValidationIssue {
                                severity: ValidationSeverity::Warning,
                                message: format!(
                                    "Overly broad permission pattern '{}' for {}. Consider restricting to specific paths.",
                                    path.as_str().unwrap_or(""), key
                                ),
                                field: Some("permissions".to_string()),
                            });
                        }
                    }
                }
            }
        }
    }

    if verbose && permission_count > 0 {
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Info,
            message: format!("Plugin requests {} permission(s)", permission_count),
            field: Some("permissions".to_string()),
        });
    }
}

fn validate_capabilities(
    capabilities: &[toml::Value],
    result: &mut ValidationResult,
    verbose: bool,
) {
    const KNOWN_CAPABILITIES: &[&str] = &[
        "commands",
        "hooks",
        "events",
        "tools",
        "formatters",
        "themes",
        "config",
        "filesystem",
        "shell",
        "network",
    ];

    let unknown_caps: Vec<String> = capabilities
        .iter()
        .filter_map(|cap| cap.as_str())
        .filter(|cap_str| !KNOWN_CAPABILITIES.contains(cap_str))
        .map(String::from)
        .collect();

    if !unknown_caps.is_empty() {
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Warning,
            message: format!("Unknown capabilities: {:?}", unknown_caps),
            field: Some("capabilities".to_string()),
        });
    }

    if verbose {
        result.issues.push(ValidationIssue {
            severity: ValidationSeverity::Info,
            message: format!("Plugin declares {} capability(ies)", capabilities.len()),
            field: Some("capabilities".to_string()),
        });
    }
}

fn output_validation_result(result: ValidationResult, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    let error_count = result
        .issues
        .iter()
        .filter(|i| i.severity == ValidationSeverity::Error)
        .count();
    let warning_count = result
        .issues
        .iter()
        .filter(|i| i.severity == ValidationSeverity::Warning)
        .count();

    if let Some(ref id) = result.plugin_id {
        println!("Validating plugin: {}", id);
    } else {
        println!("Validating plugin...");
    }
    println!("{}", "-".repeat(50));

    if result.issues.is_empty() {
        println!("✓ All checks passed");
    } else {
        for issue in &result.issues {
            let prefix = match issue.severity {
                ValidationSeverity::Error => "✗",
                ValidationSeverity::Warning => "⚠",
                ValidationSeverity::Info => "ℹ",
            };

            let field_info = issue
                .field
                .as_ref()
                .map(|f| format!(" [{}]", f))
                .unwrap_or_default();

            println!("{} {}{}", prefix, issue.message, field_info);
        }
    }

    println!();
    if result.valid {
        println!("Validation: PASSED");
        if warning_count > 0 {
            println!("  ({} warning(s))", warning_count);
        }
    } else {
        println!("Validation: FAILED");
        println!("  {} error(s), {} warning(s)", error_count, warning_count);
    }

    if !result.valid {
        bail!("Plugin validation failed");
    }

    Ok(())
}

// =============================================================================
// Publish Command Implementation
// =============================================================================

async fn run_publish(args: PluginPublishArgs) -> Result<()> {
    let plugin_dir = args
        .path
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    println!("Preparing plugin for publication...");
    println!("  Directory: {}", plugin_dir.display());

    // First, validate the plugin
    println!("\nStep 1: Validating plugin...");
    let validate_args = PluginValidateArgs {
        path: Some(plugin_dir.clone()),
        json: false,
        verbose: false,
    };

    if let Err(e) = run_validate(validate_args).await {
        bail!(
            "Plugin validation failed: {}. Fix issues before publishing.",
            e
        );
    }

    // Read plugin info
    let manifest_path = plugin_dir.join("plugin.toml");
    let manifest_content = std::fs::read_to_string(&manifest_path)?;
    let manifest: toml::Value = toml::from_str(&manifest_content)?;

    let plugin_section = manifest.get("plugin").unwrap_or(&manifest);

    let plugin_id = plugin_section
        .get("id")
        .or_else(|| manifest.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("plugin");

    let plugin_version = plugin_section
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0");

    // Check for WASM file
    let wasm_path = plugin_dir.join("plugin.wasm");
    if !wasm_path.exists() {
        println!("\nStep 2: Building plugin...");
        run_plugin_build(&plugin_dir, false, None)?;
    }

    // Verify WASM exists after build, try to copy from target dir if needed
    if !wasm_path.exists() {
        let target_wasm = plugin_dir
            .join("target")
            .join("wasm32-wasi")
            .join("release");

        if let Some(entries) = target_wasm
            .exists()
            .then(|| std::fs::read_dir(&target_wasm).ok())
            .flatten()
        {
            for entry in entries.flatten() {
                let is_wasm = entry
                    .path()
                    .extension()
                    .map(|e| e == "wasm")
                    .unwrap_or(false);
                if is_wasm {
                    std::fs::copy(entry.path(), &wasm_path)?;
                    break;
                }
            }
        }
    }

    if !wasm_path.exists() {
        bail!("No plugin.wasm file found. Build the plugin first with 'cortex plugin build'.");
    }

    // Create tarball
    println!("\nStep 3: Creating distribution package...");

    let tarball_name = format!("{}-{}.tar.gz", plugin_id, plugin_version);
    let tarball_path = args
        .output
        .unwrap_or_else(|| plugin_dir.join(&tarball_name));

    let tarball_file =
        std::fs::File::create(&tarball_path).context("Failed to create tarball file")?;

    let encoder = flate2::write::GzEncoder::new(tarball_file, flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);

    // Add essential files
    let files_to_include = [
        ("plugin.toml", true),
        ("plugin.wasm", true),
        ("README.md", false),
        ("LICENSE", false),
    ];

    let mut included_files = Vec::new();

    for (filename, required) in files_to_include {
        let file_path = plugin_dir.join(filename);
        if file_path.exists() {
            let archive_path = format!("{}/{}", plugin_id, filename);
            archive
                .append_path_with_name(&file_path, &archive_path)
                .with_context(|| format!("Failed to add {} to archive", filename))?;
            included_files.push(filename.to_string());
        } else if required {
            bail!("Required file '{}' not found", filename);
        }
    }

    archive.finish().context("Failed to finalize archive")?;

    // Calculate tarball size
    let tarball_metadata = std::fs::metadata(&tarball_path)?;
    let tarball_size = tarball_metadata.len();

    println!("\nPackage created: {}", tarball_path.display());
    println!("  Size: {:.2} KB", tarball_size as f64 / 1024.0);
    println!("  Contents:");
    for file in &included_files {
        println!("    - {}", file);
    }

    if args.dry_run {
        println!("\n[DRY RUN] Would publish:");
        println!("  Plugin: {}", plugin_id);
        println!("  Version: {}", plugin_version);
        println!("  Package: {}", tarball_path.display());
        println!("\nNote: Actual publishing is not yet implemented.");
        println!("The tarball has been created and can be manually distributed.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    // ==========================================================================
    // PluginInfo serialization tests
    // ==========================================================================

    #[test]
    fn test_plugin_info_serialization_json() {
        let info = PluginInfo {
            name: "test-plugin".to_string(),
            version: "1.2.3".to_string(),
            description: "A test plugin".to_string(),
            enabled: true,
            path: PathBuf::from("/home/user/.cortex/plugins/test-plugin"),
        };

        let json = serde_json::to_string(&info).expect("should serialize to JSON");

        assert!(json.contains("test-plugin"), "JSON should contain name");
        assert!(json.contains("1.2.3"), "JSON should contain version");
        assert!(
            json.contains("A test plugin"),
            "JSON should contain description"
        );
        assert!(json.contains("true"), "JSON should contain enabled status");
    }

    #[test]
    fn test_plugin_info_serialization_with_empty_description() {
        let info = PluginInfo {
            name: "minimal-plugin".to_string(),
            version: "0.1.0".to_string(),
            description: "".to_string(),
            enabled: false,
            path: PathBuf::from("/plugins/minimal"),
        };

        let json = serde_json::to_string(&info).expect("should serialize to JSON");

        assert!(json.contains("minimal-plugin"), "JSON should contain name");
        assert!(json.contains("0.1.0"), "JSON should contain version");
        assert!(
            json.contains("false"),
            "JSON should contain disabled status"
        );
    }

    #[test]
    fn test_plugin_info_serialization_pretty_json() {
        let info = PluginInfo {
            name: "pretty-plugin".to_string(),
            version: "2.0.0".to_string(),
            description: "Plugin for pretty output".to_string(),
            enabled: true,
            path: PathBuf::from("/path/to/plugin"),
        };

        let json = serde_json::to_string_pretty(&info).expect("should serialize to pretty JSON");

        assert!(json.contains('\n'), "Pretty JSON should have newlines");
        assert!(json.contains("pretty-plugin"), "JSON should contain name");
    }

    #[test]
    fn test_plugin_info_array_serialization() {
        let plugins = vec![
            PluginInfo {
                name: "plugin-a".to_string(),
                version: "1.0.0".to_string(),
                description: "First plugin".to_string(),
                enabled: true,
                path: PathBuf::from("/plugins/a"),
            },
            PluginInfo {
                name: "plugin-b".to_string(),
                version: "2.0.0".to_string(),
                description: "Second plugin".to_string(),
                enabled: false,
                path: PathBuf::from("/plugins/b"),
            },
        ];

        let json = serde_json::to_string(&plugins).expect("should serialize array to JSON");

        assert!(
            json.contains("plugin-a"),
            "JSON should contain first plugin name"
        );
        assert!(
            json.contains("plugin-b"),
            "JSON should contain second plugin name"
        );
        assert!(
            json.contains("1.0.0"),
            "JSON should contain first plugin version"
        );
        assert!(
            json.contains("2.0.0"),
            "JSON should contain second plugin version"
        );
    }

    #[test]
    fn test_plugin_info_empty_array_serialization() {
        let plugins: Vec<PluginInfo> = vec![];
        let json = serde_json::to_string(&plugins).expect("should serialize empty array to JSON");
        assert_eq!(json, "[]", "Empty array should serialize to []");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginListArgs
    // ==========================================================================

    #[test]
    fn test_plugin_list_args_default() {
        let args = PluginListArgs {
            json: false,
            enabled: false,
            disabled: false,
        };

        assert!(!args.json, "json should be false by default");
        assert!(!args.enabled, "enabled filter should be false by default");
        assert!(!args.disabled, "disabled filter should be false by default");
    }

    #[test]
    fn test_plugin_list_args_json_flag() {
        let args = PluginListArgs {
            json: true,
            enabled: false,
            disabled: false,
        };

        assert!(args.json, "json flag should be true when set");
    }

    #[test]
    fn test_plugin_list_args_enabled_filter() {
        let args = PluginListArgs {
            json: false,
            enabled: true,
            disabled: false,
        };

        assert!(args.enabled, "enabled filter should be true when set");
        assert!(!args.disabled, "disabled filter should be false");
    }

    #[test]
    fn test_plugin_list_args_disabled_filter() {
        let args = PluginListArgs {
            json: false,
            enabled: false,
            disabled: true,
        };

        assert!(!args.enabled, "enabled filter should be false");
        assert!(args.disabled, "disabled filter should be true when set");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginInstallArgs
    // ==========================================================================

    #[test]
    fn test_plugin_install_args_minimal() {
        let args = PluginInstallArgs {
            name: "my-plugin".to_string(),
            version: None,
            force: false,
        };

        assert_eq!(args.name, "my-plugin", "name should match");
        assert!(args.version.is_none(), "version should be None by default");
        assert!(!args.force, "force should be false by default");
    }

    #[test]
    fn test_plugin_install_args_with_version() {
        let args = PluginInstallArgs {
            name: "versioned-plugin".to_string(),
            version: Some("1.2.3".to_string()),
            force: false,
        };

        assert_eq!(args.name, "versioned-plugin", "name should match");
        assert_eq!(
            args.version,
            Some("1.2.3".to_string()),
            "version should be set"
        );
    }

    #[test]
    fn test_plugin_install_args_with_force() {
        let args = PluginInstallArgs {
            name: "forced-plugin".to_string(),
            version: None,
            force: true,
        };

        assert_eq!(args.name, "forced-plugin", "name should match");
        assert!(args.force, "force should be true when set");
    }

    #[test]
    fn test_plugin_install_args_full() {
        let args = PluginInstallArgs {
            name: "full-plugin".to_string(),
            version: Some("2.0.0-beta".to_string()),
            force: true,
        };

        assert_eq!(args.name, "full-plugin", "name should match");
        assert_eq!(
            args.version,
            Some("2.0.0-beta".to_string()),
            "version should match"
        );
        assert!(args.force, "force should be true");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginRemoveArgs
    // ==========================================================================

    #[test]
    fn test_plugin_remove_args_minimal() {
        let args = PluginRemoveArgs {
            name: "remove-me".to_string(),
            yes: false,
        };

        assert_eq!(args.name, "remove-me", "name should match");
        assert!(!args.yes, "yes should be false by default");
    }

    #[test]
    fn test_plugin_remove_args_with_yes() {
        let args = PluginRemoveArgs {
            name: "remove-confirmed".to_string(),
            yes: true,
        };

        assert_eq!(args.name, "remove-confirmed", "name should match");
        assert!(args.yes, "yes should be true when set");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginEnableArgs / PluginDisableArgs
    // ==========================================================================

    #[test]
    fn test_plugin_enable_args() {
        let args = PluginEnableArgs {
            name: "enable-me".to_string(),
        };

        assert_eq!(args.name, "enable-me", "name should match");
    }

    #[test]
    fn test_plugin_disable_args() {
        let args = PluginDisableArgs {
            name: "disable-me".to_string(),
        };

        assert_eq!(args.name, "disable-me", "name should match");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginShowArgs
    // ==========================================================================

    #[test]
    fn test_plugin_show_args_minimal() {
        let args = PluginShowArgs {
            name: "show-me".to_string(),
            json: false,
        };

        assert_eq!(args.name, "show-me", "name should match");
        assert!(!args.json, "json should be false by default");
    }

    #[test]
    fn test_plugin_show_args_with_json() {
        let args = PluginShowArgs {
            name: "show-json".to_string(),
            json: true,
        };

        assert_eq!(args.name, "show-json", "name should match");
        assert!(args.json, "json should be true when set");
    }

    // ==========================================================================
    // PluginCli command structure tests
    // ==========================================================================

    #[test]
    fn test_plugin_cli_command_exists() {
        let cmd = PluginCli::command();
        assert!(
            cmd.get_subcommands().count() > 0,
            "PluginCli should have subcommands"
        );
    }

    #[test]
    fn test_plugin_cli_has_list_subcommand() {
        let cmd = PluginCli::command();
        let list_cmd = cmd.get_subcommands().find(|c| c.get_name() == "list");
        assert!(
            list_cmd.is_some(),
            "PluginCli should have 'list' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_install_subcommand() {
        let cmd = PluginCli::command();
        let install_cmd = cmd.get_subcommands().find(|c| c.get_name() == "install");
        assert!(
            install_cmd.is_some(),
            "PluginCli should have 'install' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_remove_subcommand() {
        let cmd = PluginCli::command();
        let remove_cmd = cmd.get_subcommands().find(|c| c.get_name() == "remove");
        assert!(
            remove_cmd.is_some(),
            "PluginCli should have 'remove' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_enable_subcommand() {
        let cmd = PluginCli::command();
        let enable_cmd = cmd.get_subcommands().find(|c| c.get_name() == "enable");
        assert!(
            enable_cmd.is_some(),
            "PluginCli should have 'enable' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_disable_subcommand() {
        let cmd = PluginCli::command();
        let disable_cmd = cmd.get_subcommands().find(|c| c.get_name() == "disable");
        assert!(
            disable_cmd.is_some(),
            "PluginCli should have 'disable' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_show_subcommand() {
        let cmd = PluginCli::command();
        let show_cmd = cmd.get_subcommands().find(|c| c.get_name() == "show");
        assert!(
            show_cmd.is_some(),
            "PluginCli should have 'show' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_list_has_ls_alias() {
        let cmd = PluginCli::command();
        let list_cmd = cmd.get_subcommands().find(|c| c.get_name() == "list");
        if let Some(list) = list_cmd {
            let aliases: Vec<_> = list.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"ls"),
                "list command should have 'ls' alias"
            );
        }
    }

    #[test]
    fn test_plugin_cli_install_has_add_alias() {
        let cmd = PluginCli::command();
        let install_cmd = cmd.get_subcommands().find(|c| c.get_name() == "install");
        if let Some(install) = install_cmd {
            let aliases: Vec<_> = install.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"add"),
                "install command should have 'add' alias"
            );
        }
    }

    #[test]
    fn test_plugin_cli_remove_has_rm_alias() {
        let cmd = PluginCli::command();
        let remove_cmd = cmd.get_subcommands().find(|c| c.get_name() == "remove");
        if let Some(remove) = remove_cmd {
            let aliases: Vec<_> = remove.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"rm"),
                "remove command should have 'rm' alias"
            );
        }
    }

    #[test]
    fn test_plugin_cli_remove_has_uninstall_alias() {
        let cmd = PluginCli::command();
        let remove_cmd = cmd.get_subcommands().find(|c| c.get_name() == "remove");
        if let Some(remove) = remove_cmd {
            let aliases: Vec<_> = remove.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"uninstall"),
                "remove command should have 'uninstall' alias"
            );
        }
    }

    #[test]
    fn test_plugin_cli_show_has_info_alias() {
        let cmd = PluginCli::command();
        let show_cmd = cmd.get_subcommands().find(|c| c.get_name() == "show");
        if let Some(show) = show_cmd {
            let aliases: Vec<_> = show.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"info"),
                "show command should have 'info' alias"
            );
        }
    }

    // ==========================================================================
    // PluginSubcommand variant tests
    // ==========================================================================

    #[test]
    fn test_plugin_subcommand_list_variant() {
        let args = PluginListArgs {
            json: true,
            enabled: false,
            disabled: false,
        };
        let subcmd = PluginSubcommand::List(args);

        match subcmd {
            PluginSubcommand::List(list_args) => {
                assert!(list_args.json, "List variant should contain correct args");
            }
            _ => panic!("Expected List variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_install_variant() {
        let args = PluginInstallArgs {
            name: "test".to_string(),
            version: Some("1.0.0".to_string()),
            force: true,
        };
        let subcmd = PluginSubcommand::Install(args);

        match subcmd {
            PluginSubcommand::Install(install_args) => {
                assert_eq!(
                    install_args.name, "test",
                    "Install variant should contain correct args"
                );
                assert_eq!(install_args.version, Some("1.0.0".to_string()));
                assert!(install_args.force);
            }
            _ => panic!("Expected Install variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_remove_variant() {
        let args = PluginRemoveArgs {
            name: "remove-test".to_string(),
            yes: true,
        };
        let subcmd = PluginSubcommand::Remove(args);

        match subcmd {
            PluginSubcommand::Remove(remove_args) => {
                assert_eq!(
                    remove_args.name, "remove-test",
                    "Remove variant should contain correct args"
                );
                assert!(remove_args.yes);
            }
            _ => panic!("Expected Remove variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_enable_variant() {
        let args = PluginEnableArgs {
            name: "enable-test".to_string(),
        };
        let subcmd = PluginSubcommand::Enable(args);

        match subcmd {
            PluginSubcommand::Enable(enable_args) => {
                assert_eq!(
                    enable_args.name, "enable-test",
                    "Enable variant should contain correct args"
                );
            }
            _ => panic!("Expected Enable variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_disable_variant() {
        let args = PluginDisableArgs {
            name: "disable-test".to_string(),
        };
        let subcmd = PluginSubcommand::Disable(args);

        match subcmd {
            PluginSubcommand::Disable(disable_args) => {
                assert_eq!(
                    disable_args.name, "disable-test",
                    "Disable variant should contain correct args"
                );
            }
            _ => panic!("Expected Disable variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_show_variant() {
        let args = PluginShowArgs {
            name: "show-test".to_string(),
            json: true,
        };
        let subcmd = PluginSubcommand::Show(args);

        match subcmd {
            PluginSubcommand::Show(show_args) => {
                assert_eq!(
                    show_args.name, "show-test",
                    "Show variant should contain correct args"
                );
                assert!(show_args.json);
            }
            _ => panic!("Expected Show variant"),
        }
    }

    // ==========================================================================
    // Debug trait tests
    // ==========================================================================

    #[test]
    fn test_plugin_list_args_debug() {
        let args = PluginListArgs {
            json: true,
            enabled: false,
            disabled: true,
        };
        let debug_output = format!("{:?}", args);

        assert!(
            debug_output.contains("PluginListArgs"),
            "Debug should include type name"
        );
        assert!(
            debug_output.contains("json"),
            "Debug should include json field"
        );
        assert!(
            debug_output.contains("enabled"),
            "Debug should include enabled field"
        );
        assert!(
            debug_output.contains("disabled"),
            "Debug should include disabled field"
        );
    }

    #[test]
    fn test_plugin_install_args_debug() {
        let args = PluginInstallArgs {
            name: "test-plugin".to_string(),
            version: Some("1.0.0".to_string()),
            force: true,
        };
        let debug_output = format!("{:?}", args);

        assert!(
            debug_output.contains("PluginInstallArgs"),
            "Debug should include type name"
        );
        assert!(
            debug_output.contains("test-plugin"),
            "Debug should include name"
        );
        assert!(
            debug_output.contains("1.0.0"),
            "Debug should include version"
        );
    }

    #[test]
    fn test_plugin_subcommand_debug() {
        let subcmd = PluginSubcommand::Enable(PluginEnableArgs {
            name: "test".to_string(),
        });
        let debug_output = format!("{:?}", subcmd);

        assert!(
            debug_output.contains("Enable"),
            "Debug should include variant name"
        );
        assert!(
            debug_output.contains("test"),
            "Debug should include contained name"
        );
    }

    #[test]
    fn test_plugin_cli_debug() {
        let cli = PluginCli {
            subcommand: PluginSubcommand::List(PluginListArgs {
                json: false,
                enabled: false,
                disabled: false,
            }),
        };
        let debug_output = format!("{:?}", cli);

        assert!(
            debug_output.contains("PluginCli"),
            "Debug should include type name"
        );
        assert!(
            debug_output.contains("List"),
            "Debug should include subcommand variant"
        );
    }
}
