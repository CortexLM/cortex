# Getting Started with Cortex Plugin Development

This guide walks you through creating your first Cortex plugin from scratch.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Creating Your First Plugin](#creating-your-first-plugin)
- [Building and Installing](#building-and-installing)
- [Testing Your Plugin](#testing-your-plugin)
- [Debugging Tips](#debugging-tips)
- [Common Issues and Solutions](#common-issues-and-solutions)

## Prerequisites

### Required Tools

1. **Rust toolchain** (1.70.0 or later)
   ```bash
   # Install Rust
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   
   # Verify installation
   rustc --version
   cargo --version
   ```

2. **wasm32-wasi target**
   ```bash
   rustup target add wasm32-wasi
   ```

3. **Cortex CLI** (for testing)
   ```bash
   # Verify Cortex is installed
   cortex --version
   ```

### Recommended Tools

- **wasm-opt** (from binaryen) for optimizing WASM files
- **wasmtime** for testing WASM modules outside Cortex

## Creating Your First Plugin

We'll create a simple "hello" plugin that adds a `/hello` command.

### Step 1: Create the Plugin Directory

```bash
mkdir -p ~/.cortex/plugins/hello-plugin
cd ~/.cortex/plugins/hello-plugin
```

### Step 2: Initialize Cargo Project

```bash
cargo init --lib
```

### Step 3: Configure Cargo.toml

Replace the contents of `Cargo.toml`:

```toml
[package]
name = "hello-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wee_alloc = "0.4"

[profile.release]
opt-level = "s"      # Optimize for size
lto = true           # Link-time optimization
strip = true         # Strip symbols
```

### Step 4: Create the Plugin Manifest

Create `plugin.toml`:

```toml
[plugin]
id = "hello-plugin"
name = "Hello Plugin"
version = "0.1.0"
description = "A simple hello world plugin"
authors = ["Your Name <your@email.com>"]

capabilities = ["commands"]

[[commands]]
name = "hello"
aliases = ["hi", "hey"]
description = "Say hello"
usage = "/hello [name]"

[[commands.args]]
name = "name"
description = "Name to greet"
required = false
default = "World"

[wasm]
memory_pages = 64
timeout_ms = 5000
```

### Step 5: Write the Plugin Code

Replace `src/lib.rs` with:

```rust
//! Hello Plugin - A simple Cortex plugin example

#![no_std]

extern crate alloc;

// Host function imports
#[link(wasm_import_module = "cortex")]
extern "C" {
    /// Log a message at the specified level
    /// level: 0=trace, 1=debug, 2=info, 3=warn, 4=error
    fn log(level: i32, msg_ptr: i32, msg_len: i32);
}

// Logging helpers
fn log_info(msg: &str) {
    // SAFETY: FFI call to host-provided `log` function.
    // The host copies the string data from WASM memory immediately and does not
    // retain the pointer. The pointer is valid for the duration of the call.
    unsafe {
        log(2, msg.as_ptr() as i32, msg.len() as i32);
    }
}

fn log_debug(msg: &str) {
    unsafe {
        log(1, msg.as_ptr() as i32, msg.len() as i32);
    }
}

// ============================================================================
// Plugin Lifecycle
// ============================================================================

/// Called when the plugin is initialized
#[no_mangle]
pub extern "C" fn init() -> i32 {
    log_info("Hello Plugin initialized!");
    0 // Return 0 for success
}

/// Called when the plugin is shutting down
#[no_mangle]
pub extern "C" fn shutdown() -> i32 {
    log_info("Hello Plugin shutting down");
    0
}

// ============================================================================
// Command Handlers
// ============================================================================

/// Handler for the /hello command
/// Function name format: cmd_<command_name_with_underscores>
#[no_mangle]
pub extern "C" fn cmd_hello() -> i32 {
    log_info("Hello, World!");
    0
}

// ============================================================================
// Required: Panic Handler
// ============================================================================

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// ============================================================================
// Required: Global Allocator
// ============================================================================

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
```

### Step 6: Create .gitignore

```bash
echo "target/" > .gitignore
```

## Building and Installing

### Build the Plugin

```bash
# Build in release mode for the WASM target
cargo build --target wasm32-wasi --release
```

### Install the Plugin

```bash
# Copy the WASM file to the plugin directory
cp target/wasm32-wasi/release/hello_plugin.wasm plugin.wasm
```

Your plugin directory should now look like:

```
~/.cortex/plugins/hello-plugin/
├── Cargo.toml
├── plugin.toml
├── plugin.wasm
└── src/
    └── lib.rs
```

### Verify Installation

```bash
# List installed plugins
cortex plugin list

# Validate your plugin
cortex plugin validate ~/.cortex/plugins/hello-plugin
```

## Testing Your Plugin

### Run Your Command

Start Cortex and try your new command:

```bash
cortex
```

Then in the Cortex TUI:

```
/hello
```

You should see the log output: "Hello, World!"

### Development Mode

For rapid iteration, use development mode:

```bash
# Watch for changes and auto-reload
cortex plugin dev ~/.cortex/plugins/hello-plugin --watch
```

This will:
- Watch for file changes
- Automatically rebuild on change
- Hot-reload the plugin

### Unit Testing

Add tests to your `src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    // Note: Tests run natively, not in WASM
    // Mock the host functions for testing
    
    #[test]
    fn test_command_name() {
        // Test that command names follow conventions
        assert_eq!("hello".replace('-', "_"), "hello");
    }
}
```

Run tests:

```bash
cargo test
```

## Debugging Tips

### 1. Enable Debug Logging

Add verbose logging to your plugin:

```rust
fn log_debug(msg: &str) {
    unsafe {
        log(1, msg.as_ptr() as i32, msg.len() as i32);
    }
}

#[no_mangle]
pub extern "C" fn cmd_hello() -> i32 {
    log_debug("cmd_hello called");
    // ... rest of function
}
```

### 2. Check Plugin Loading

```bash
# Verbose plugin discovery
RUST_LOG=debug cortex plugin list
```

### 3. Validate Manifest

```bash
cortex plugin validate ~/.cortex/plugins/hello-plugin
```

This checks:
- Manifest syntax
- Required fields
- Version format
- Command definitions
- Hook configurations

### 4. Inspect WASM Module

```bash
# List exported functions
wasm-objdump -x plugin.wasm | grep -A 100 "Export"

# Check module size
ls -lh plugin.wasm
```

### 5. Test WASM Directly

```bash
# Run with wasmtime (if installed)
wasmtime --invoke init plugin.wasm
```

### 6. Use Tracing

In your manifest, enable debug settings:

```toml
[wasm]
memory_pages = 256
timeout_ms = 30000  # Longer timeout for debugging
```

## Common Issues and Solutions

### Issue: "Plugin not found"

**Symptoms**: Plugin doesn't appear in `cortex plugin list`

**Solutions**:
1. Verify directory structure:
   ```bash
   ls -la ~/.cortex/plugins/hello-plugin/
   # Should have: plugin.toml, plugin.wasm
   ```

2. Check manifest file name (must be `plugin.toml`):
   ```bash
   cat ~/.cortex/plugins/hello-plugin/plugin.toml
   ```

3. Validate manifest syntax:
   ```bash
   cortex plugin validate ~/.cortex/plugins/hello-plugin
   ```

### Issue: "WASM compilation failed"

**Symptoms**: Error during plugin loading about WASM

**Solutions**:
1. Verify WASM file exists:
   ```bash
   file plugin.wasm
   # Should show: WebAssembly (wasm) binary module
   ```

2. Check for missing exports:
   ```bash
   wasm-objdump -x plugin.wasm | grep "Export"
   # Should list: init, shutdown, cmd_hello
   ```

3. Rebuild with verbose output:
   ```bash
   cargo build --target wasm32-wasi --release -v
   ```

### Issue: "Function 'cmd_xxx' not found"

**Symptoms**: Command exists but doesn't execute

**Solutions**:
1. Verify function naming convention:
   - Manifest: `name = "my-command"`
   - Rust: `fn cmd_my_command()`
   - Replace hyphens with underscores

2. Check `#[no_mangle]` attribute:
   ```rust
   #[no_mangle]
   pub extern "C" fn cmd_hello() -> i32 {
       // ...
   }
   ```

3. Verify export in WASM:
   ```bash
   wasm-objdump -x plugin.wasm | grep cmd_
   ```

### Issue: "Memory limit exceeded"

**Symptoms**: Plugin crashes with memory error

**Solutions**:
1. Increase memory in manifest:
   ```toml
   [wasm]
   memory_pages = 256  # Maximum: 16MB
   ```

2. Optimize allocations:
   - Use `wee_alloc` (small allocator)
   - Avoid large static allocations
   - Free memory when possible

3. Check for memory leaks in your code

### Issue: "Timeout exceeded"

**Symptoms**: Plugin execution times out

**Solutions**:
1. Increase timeout in manifest:
   ```toml
   [wasm]
   timeout_ms = 60000  # 60 seconds
   ```

2. Optimize your code:
   - Avoid infinite loops
   - Break up long operations
   - Use async patterns where possible

### Issue: "Permission denied"

**Symptoms**: Plugin can't access files/network/etc.

**Solutions**:
1. Declare required permissions in manifest:
   ```toml
   permissions = [
       { read_file = { paths = ["**/*.rs"] } },
       { network = { domains = ["api.example.com"] } }
   ]
   ```

2. Grant permissions in Cortex config:
   ```toml
   [[plugins]]
   name = "hello-plugin"
   granted_permissions = ["read_files"]
   ```

### Issue: Plugin crashes silently

**Symptoms**: No output, plugin seems to do nothing

**Solutions**:
1. Add panic handler with logging:
   ```rust
   #[panic_handler]
   fn panic(info: &core::panic::PanicInfo) -> ! {
       // Can't log here in no_std, but at least won't crash silently
       loop {}
   }
   ```

2. Wrap operations in error handling:
   ```rust
   #[no_mangle]
   pub extern "C" fn cmd_hello() -> i32 {
       log_debug("Starting cmd_hello");
       
       // Your code here
       
       log_debug("Finished cmd_hello");
       0
   }
   ```

3. Check Cortex logs for errors:
   ```bash
   RUST_LOG=debug cortex
   ```

## Next Steps

Now that you've created your first plugin:

1. **Add hooks**: See [Hooks Reference](./HOOKS.md) for intercepting tool execution, chat messages, and more.

2. **Understand security**: Read [Security Model](./SECURITY.md) for best practices.

3. **Explore advanced features**:
   - Custom widgets
   - Keyboard bindings
   - Event handling
   - Network requests

4. **Share your plugin**: Consider publishing to the Cortex plugin registry!
