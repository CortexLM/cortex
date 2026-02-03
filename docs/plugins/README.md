# Cortex Plugin Development Guide

Welcome to the Cortex Plugin System! This guide provides comprehensive documentation for developing plugins that extend Cortex functionality.

## Table of Contents

- [Introduction](#introduction)
- [Plugin Architecture](#plugin-architecture)
- [Plugin Manifest](#plugin-manifest-plugintoml)
- [Capabilities and Permissions](#capabilities-and-permissions)
- [WASM Runtime](#wasm-runtime)
- [Host Functions](#host-functions)
- [Development Workflow](#development-workflow)
- [Related Documentation](#related-documentation)

## Introduction

The Cortex plugin system allows developers to extend the CLI with custom functionality including:

- **Custom Commands**: Add new slash commands
- **Hook Handlers**: Intercept and modify tool execution, chat messages, UI rendering, and more
- **Custom Widgets**: Add UI elements to the TUI
- **Keyboard Bindings**: Register custom key combinations
- **Event Handling**: React to system events

Plugins are written in Rust and compiled to WebAssembly (WASM), providing cross-platform compatibility and sandboxed execution.

## Plugin Architecture

### Plugin Types

| Type | Description | Location |
|------|-------------|----------|
| WASM | WebAssembly-based plugins for cross-platform compatibility | `~/.cortex/plugins/` or `.cortex/plugins/` |
| Native | Dynamic libraries (.so, .dylib, .dll) for maximum performance | Same as above |
| Script | Directory-based plugins with hooks.json and scripts | Same as above |

### Plugin Locations

Plugins are discovered from two locations:

1. **User Plugins**: `~/.cortex/plugins/` - Available across all projects
2. **Project Plugins**: `.cortex/plugins/` - Project-specific plugins

### Plugin Lifecycle

```
┌─────────────┐     ┌─────────────┐     ┌──────────────┐     ┌────────────┐
│  Discovery  │ ──▶ │   Loading   │ ──▶ │ Registration │ ──▶ │ Initialize │
└─────────────┘     └─────────────┘     └──────────────┘     └────────────┘
                                                                    │
                                                                    ▼
┌─────────────┐     ┌─────────────┐                          ┌────────────┐
│  Shutdown   │ ◀── │  Unloading  │ ◀────────────────────────│   Active   │
└─────────────┘     └─────────────┘                          └────────────┘
```

1. **Discovery**: Plugins are found in plugin directories
2. **Loading**: Manifest is loaded and validated
3. **Registration**: Plugin is registered with the manager
4. **Initialize**: Plugin's `init()` function is called
5. **Active**: Plugin receives hook calls
6. **Shutdown**: Plugin's `shutdown()` function is called on exit

## Plugin Manifest (plugin.toml)

Every plugin requires a `plugin.toml` manifest file. Here's a complete reference:

```toml
# ============================================================================
# Plugin Metadata
# ============================================================================
[plugin]
id = "my-awesome-plugin"           # Unique identifier (alphanumeric, hyphens, underscores)
name = "My Awesome Plugin"         # Human-readable name
version = "1.0.0"                  # Semantic version (required)
description = "Plugin description" # What your plugin does
authors = ["Your Name <email>"]    # List of authors
homepage = "https://github.com/..." # Plugin homepage/repository
license = "MIT"                    # License identifier
min_cortex_version = "1.0.0"       # Minimum Cortex version required
keywords = ["utility", "tools"]    # Keywords for discovery

# ============================================================================
# Capabilities (what your plugin can do)
# ============================================================================
capabilities = [
    "commands",    # Provide custom commands
    "hooks",       # Register hooks
    "events",      # Handle events
    "tools",       # Provide tools (MCP-style)
    "formatters",  # Provide formatters
    "themes",      # Provide custom themes
    "config",      # Access configuration
    "filesystem",  # Access file system (with permissions)
    "shell",       # Execute shell commands (with permissions)
    "network"      # Make network requests (with permissions)
]

# ============================================================================
# Permissions (what resources your plugin needs)
# ============================================================================
permissions = [
    { read_file = { paths = ["**/*.rs", "**/*.toml"] } },
    { write_file = { paths = [".cortex/plugins/my-plugin/**"] } },
    { execute = { commands = ["cargo", "rustc"] } },
    { network = { domains = ["api.example.com", "registry.npmjs.org"] } },
    { environment = { vars = ["HOME", "PATH"] } },
    { config = { keys = ["theme", "model"] } },
    "clipboard",       # Access clipboard
    "notifications"    # Show notifications
]

# ============================================================================
# Commands
# ============================================================================
[[commands]]
name = "my-command"               # Command name (without leading /)
aliases = ["mc", "myc"]           # Alternative names
description = "Does something"    # Command description
usage = "/my-command [options]"   # Usage example
category = "utility"              # Category for grouping
hidden = false                    # Hide from help listing

[[commands.args]]
name = "option"                   # Argument name
description = "An option"         # Argument description
required = false                  # Is it required?
default = "default_value"         # Default value
arg_type = "string"               # Type: string, number, boolean

# ============================================================================
# Hooks
# ============================================================================
[[hooks]]
hook_type = "tool_execute_before" # Hook type (see HOOKS.md)
priority = 50                     # Lower = runs first (default: 100)
pattern = "*.rs"                  # Optional pattern filter
function = "my_hook_handler"      # WASM function name (optional)

[[hooks]]
hook_type = "session_start"
priority = 100

# ============================================================================
# Configuration Schema
# ============================================================================
[config]
api_key = { description = "API key", type = "string", required = true }
max_items = { description = "Max items", type = "number", default = 10 }
enabled = { description = "Enable feature", type = "boolean", default = true }

[config.advanced]
description = "Advanced settings"
type = "object"
required = false

# ============================================================================
# WASM Settings
# ============================================================================
[wasm]
memory_pages = 256      # Memory limit in 64KB pages (256 = 16MB)
timeout_ms = 30000      # Execution timeout (30 seconds default)
wasi_enabled = true     # Enable WASI preview1
wasi_caps = [           # WASI capabilities
    "stdin",
    "stdout",
    "stderr",
    "env",
    "random",
    "clocks"
]
```

## Capabilities and Permissions

### Capabilities

Capabilities declare what your plugin can do:

| Capability | Description |
|------------|-------------|
| `commands` | Provide custom slash commands |
| `hooks` | Register hook handlers |
| `events` | Handle system events |
| `tools` | Provide MCP-style tools |
| `formatters` | Provide code formatters |
| `themes` | Provide custom themes |
| `config` | Access Cortex configuration |
| `filesystem` | Access file system (requires permissions) |
| `shell` | Execute shell commands (requires permissions) |
| `network` | Make network requests (requires permissions) |

### Permissions

Permissions request access to specific resources:

| Permission | Description | Risk Level |
|------------|-------------|------------|
| `read_file` | Read files from specified paths | Medium |
| `write_file` | Write files to specified paths | High |
| `execute` | Execute specified shell commands | High |
| `network` | Access specified network domains | Medium |
| `environment` | Access environment variables | Low |
| `config` | Access configuration keys | Low |
| `clipboard` | Access clipboard | Medium |
| `notifications` | Show notifications | Low |

## WASM Runtime

### Resource Limits

The WASM runtime enforces strict resource limits for security:

| Resource | Limit | Description |
|----------|-------|-------------|
| Memory | 16 MB | Maximum memory per plugin instance |
| CPU | 10M operations | Fuel-based CPU limiting |
| Timeout | 30 seconds | Default execution timeout |
| Tables | 10,000 elements | Maximum table size |
| Instances | 10 | Maximum instances per plugin |
| Memories | 1 | One linear memory per instance |

### Memory Configuration

Memory is allocated in 64KB pages:

```toml
[wasm]
memory_pages = 256  # 256 × 64KB = 16MB (maximum)
```

### Fuel-Based CPU Limiting

The runtime uses wasmtime's fuel mechanism to limit CPU usage. Each WASM operation consumes fuel. When fuel is exhausted, execution stops with an error.

Default fuel limit: 10,000,000 operations (approximately 10 million basic operations).

## Host Functions

Plugins can call host functions to interact with Cortex. These are exposed through the `cortex` WASM import module.

### Available Host Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `log` | `(level: i32, msg_ptr: i32, msg_len: i32)` | Log a message |
| `get_context` | `() -> i64` | Get execution context |
| `register_widget` | `(region: i32, type_ptr: i32, type_len: i32) -> i32` | Register a UI widget |
| `register_keybinding` | `(key_ptr: i32, key_len: i32, action_ptr: i32, action_len: i32) -> i32` | Register a keyboard binding |
| `show_toast` | `(level: i32, msg_ptr: i32, msg_len: i32, duration_ms: i32) -> i32` | Show a toast notification |
| `emit_event` | `(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32` | Emit a custom event |

### Log Levels

| Level | Value | Description |
|-------|-------|-------------|
| Trace | 0 | Detailed debugging |
| Debug | 1 | Debug information |
| Info | 2 | General information |
| Warn | 3 | Warning messages |
| Error | 4 | Error messages |

### UI Regions for Widgets

| Region | Value | Description |
|--------|-------|-------------|
| Header | 0 | Top header area |
| Footer | 1 | Bottom footer area |
| SidebarLeft | 2 | Left sidebar |
| SidebarRight | 3 | Right sidebar |
| MainContent | 4 | Main content area |
| InputArea | 5 | User input area |
| Overlay | 6 | Overlay/modal area |
| StatusBar | 7 | Status bar |
| ToolOutput | 8 | Tool output area |
| MessageArea | 9 | Chat message area |

### Toast Notification Levels

| Level | Value | Description |
|-------|-------|-------------|
| Info | 0 | Informational |
| Success | 1 | Success message |
| Warning | 2 | Warning message |
| Error | 3 | Error message |

### Example: Using Host Functions in Rust

```rust
#[link(wasm_import_module = "cortex")]
extern "C" {
    fn log(level: i32, msg_ptr: i32, msg_len: i32);
    fn register_widget(region: i32, type_ptr: i32, type_len: i32) -> i32;
    fn show_toast(level: i32, msg_ptr: i32, msg_len: i32, duration_ms: i32) -> i32;
}

fn log_info(msg: &str) {
    unsafe {
        log(2, msg.as_ptr() as i32, msg.len() as i32);
    }
}

fn register_status_widget(widget_type: &str) -> bool {
    unsafe {
        register_widget(7, widget_type.as_ptr() as i32, widget_type.len() as i32) == 0
    }
}

fn show_success_toast(msg: &str, duration_ms: i32) {
    unsafe {
        show_toast(1, msg.as_ptr() as i32, msg.len() as i32, duration_ms);
    }
}
```

## Development Workflow

### 1. Create Your Plugin

```bash
# Create plugin directory
mkdir -p ~/.cortex/plugins/my-plugin
cd ~/.cortex/plugins/my-plugin

# Initialize Rust project
cargo init --lib
```

### 2. Configure Cargo.toml

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wee_alloc = "0.4"

[profile.release]
opt-level = "s"
lto = true
```

### 3. Create plugin.toml

See the [Plugin Manifest](#plugin-manifest-plugintoml) section above.

### 4. Build Your Plugin

```bash
# Add WASM target
rustup target add wasm32-wasi

# Build
cargo build --target wasm32-wasi --release

# Copy WASM file
cp target/wasm32-wasi/release/my_plugin.wasm plugin.wasm
```

### 5. Test Your Plugin

```bash
# Validate your plugin
cortex plugin validate .

# Test in development mode
cortex plugin dev --watch
```

### 6. Install Your Plugin

```bash
# Plugin is automatically discovered from ~/.cortex/plugins/my-plugin/
cortex plugin list
```

## Related Documentation

- [Getting Started](./GETTING_STARTED.md) - Step-by-step tutorial for creating your first plugin
- [Hooks Reference](./HOOKS.md) - Complete reference for all hook types
- [Security Model](./SECURITY.md) - Security features and best practices

## Configuration in Cortex Config

Plugins can be configured in your main Cortex config file:

```toml
[[plugins]]
name = "my-plugin"
path = "~/.cortex/plugins/my-plugin"
enabled = true
priority = 0
granted_permissions = ["read_files", "network"]

[plugins.config]
api_key = "your-api-key"
max_items = 20
```
