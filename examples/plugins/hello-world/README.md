# Hello World Plugin

A simple example plugin demonstrating basic Cortex plugin capabilities.

## Features

- **Command**: `/hello [name]` - Greets the user with a customizable message
- **Hook**: `tool_execute_before` - Logs all tool executions
- **Configuration**: `greeting_prefix` setting for customizing greetings

## Project Structure

```
hello-world/
├── plugin.toml     # Plugin manifest (metadata, commands, hooks, config)
├── Cargo.toml      # Rust build configuration
├── src/
│   └── lib.rs      # Plugin implementation
└── README.md       # This file
```

## Building

### Prerequisites

1. Install Rust: https://rustup.rs/
2. Add the WASM target:
   ```bash
   rustup target add wasm32-wasi
   ```

### Build the Plugin

```bash
cd examples/plugins/hello-world
cargo build --target wasm32-wasi --release
```

The compiled WASM module will be at:
```
target/wasm32-wasi/release/hello_world_plugin.wasm
```

## Installing

Copy the compiled WASM and manifest to your Cortex plugins directory:

```bash
# Create plugin directory
mkdir -p ~/.cortex/plugins/hello-world

# Copy manifest
cp plugin.toml ~/.cortex/plugins/hello-world/

# Copy compiled WASM
cp target/wasm32-wasi/release/hello_world_plugin.wasm ~/.cortex/plugins/hello-world/plugin.wasm
```

## Configuration

Add to your Cortex configuration to customize the plugin:

```toml
[plugins.hello-world]
enabled = true
greeting_prefix = "Howdy"  # Default: "Hello"
```

## Usage

Once installed and enabled, use the command in Cortex:

```
/hello          # Output: "Hello, World!"
/hello Alice    # Output: "Hello, Alice!"
/hi Bob         # Alias also works: "Hello, Bob!"
```

## Plugin Manifest Reference

The `plugin.toml` file contains:

- **[plugin]**: Metadata (id, name, version, description, authors)
- **capabilities**: What the plugin can do (commands, hooks, config)
- **[[commands]]**: Command definitions with arguments
- **[[hooks]]**: Hook registrations with priorities
- **[config]**: Configuration schema with validation
- **[wasm]**: WASM runtime settings (memory, timeout)

## Hook Behavior

The `tool_execute_before` hook logs every tool execution:

```
[DEBUG] Tool execution intercepted by hello-world plugin
[INFO] Tool about to execute - logging from hello-world plugin
```

This is useful for debugging and auditing tool usage.

## Development

### Code Structure

1. **Host function imports**: Functions provided by Cortex (`log`, `get_context`, etc.)
2. **Lifecycle functions**: `init()` and `shutdown()` for plugin lifecycle
3. **Command handlers**: `cmd_<name>()` functions for each command
4. **Hook handlers**: `hook_<type>()` functions for each hook

### Adding a New Command

1. Add the command definition in `plugin.toml`:
   ```toml
   [[commands]]
   name = "mycommand"
   description = "My new command"
   ```

2. Implement the handler in `src/lib.rs`:
   ```rust
   #[no_mangle]
   pub extern "C" fn cmd_mycommand() -> i32 {
       log_info("My command executed!");
       0
   }
   ```

### Adding a New Hook

1. Add the hook definition in `plugin.toml`:
   ```toml
   [[hooks]]
   hook_type = "session_start"
   priority = 100
   ```

2. Implement the handler in `src/lib.rs`:
   ```rust
   #[no_mangle]
   pub extern "C" fn hook_session_start() -> i32 {
       log_info("Session started!");
       0
   }
   ```

## License

MIT License - See LICENSE file for details.
