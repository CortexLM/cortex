# Code Stats Plugin

An advanced example plugin demonstrating comprehensive Cortex plugin capabilities.

## Features

### Commands

- **`/stats`** (aliases: `/statistics`, `/metrics`) - Display current session code statistics
- **`/stats-reset`** (alias: `/reset-stats`) - Reset all statistics counters to zero
- **`/stats-export`** (alias: `/export-stats`) - Export statistics to JSON format

### Hooks

- **`file_operation_after`** - Tracks statistics after each file operation
- **`session_end`** - Saves statistics when the session ends
- **`widget_register`** - Registers the status bar widget

### UI Widget

- **Status Bar Widget** - Displays compact statistics (`+123 -45 (67)`) in the status bar

### Configuration

- **`auto_save`** (boolean, default: true) - Automatically save stats on session end
- **`display_format`** (string, default: "compact") - Format for displaying stats (compact, detailed)
- **`track_by_language`** (boolean, default: true) - Track statistics per programming language

## Project Structure

```
code-stats/
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
cd examples/plugins/code-stats
cargo build --target wasm32-wasi --release
```

The compiled WASM module will be at:
```
target/wasm32-wasi/release/code_stats_plugin.wasm
```

## Installing

Copy the compiled WASM and manifest to your Cortex plugins directory:

```bash
# Create plugin directory
mkdir -p ~/.cortex/plugins/code-stats

# Copy manifest
cp plugin.toml ~/.cortex/plugins/code-stats/

# Copy compiled WASM
cp target/wasm32-wasi/release/code_stats_plugin.wasm ~/.cortex/plugins/code-stats/plugin.wasm
```

## Configuration

Add to your Cortex configuration to customize the plugin:

```toml
[plugins.code-stats]
enabled = true
auto_save = true
display_format = "detailed"  # or "compact"
track_by_language = true
```

## Usage

### View Statistics

```
/stats
```

Output: `Lines: +123 -45 | Files: 10 modified, 3 created, 1 deleted | Total ops: 14`

### Reset Statistics

```
/stats-reset
```

This clears all counters to zero for a fresh start.

### Export Statistics

```
/stats-export
```

Exports statistics as JSON to the event stream:

```json
{
  "lines_added": 123,
  "lines_removed": 45,
  "files_modified": 10,
  "files_created": 3,
  "files_deleted": 1,
  "total_operations": 14
}
```

## Plugin Architecture

### Statistics Tracking

The plugin uses atomic counters to track:
- **lines_added** - Total lines of code added
- **lines_removed** - Total lines of code removed
- **files_modified** - Number of files edited
- **files_created** - Number of new files created
- **files_deleted** - Number of files removed
- **total_operations** - Total file operations

### Event System

The plugin emits events for external integrations:

| Event Name | Trigger | Data |
|------------|---------|------|
| `code_stats.displayed` | `/stats` command | Current stats JSON |
| `code_stats.before_reset` | `/stats-reset` command | Stats before reset |
| `code_stats.exported` | `/stats-export` command | Full stats JSON |
| `code_stats.session_end` | Session ending | Final stats JSON |
| `code_stats.session_final` | Plugin shutdown | Final stats JSON |

### Widget Rendering

The status bar widget displays a compact summary:
```
+{lines_added} -{lines_removed} ({total_ops})
```

Example: `+123 -45 (14)`

## Development

### Adding New Metrics

1. Add an atomic counter in `src/lib.rs`:
   ```rust
   static MY_METRIC: AtomicU64 = AtomicU64::new(0);
   ```

2. Update the `get_stats_summary()` function to include it.

3. Update the `get_stats_json()` function to include it.

4. Add a recording function:
   ```rust
   fn record_my_metric(value: u64) {
       MY_METRIC.fetch_add(value, Ordering::Relaxed);
   }
   ```

### Adding New Commands

1. Add the command definition in `plugin.toml`:
   ```toml
   [[commands]]
   name = "my-command"
   description = "My new command"
   usage = "/my-command [args]"
   ```

2. Implement the handler in `src/lib.rs`:
   ```rust
   #[no_mangle]
   pub extern "C" fn cmd_my_command() -> i32 {
       log_info("My command executed!");
       0
   }
   ```

### Adding New Hooks

1. Add the hook definition in `plugin.toml`:
   ```toml
   [[hooks]]
   hook_type = "tool_execute_after"
   priority = 100
   function = "hook_tool_execute_after"
   ```

2. Implement the handler in `src/lib.rs`:
   ```rust
   #[no_mangle]
   pub extern "C" fn hook_tool_execute_after() -> i32 {
       log_debug("Tool execution completed");
       0
   }
   ```

## API Reference

### Exported Functions

| Function | Purpose | Returns |
|----------|---------|---------|
| `init()` | Plugin initialization | 0 on success |
| `shutdown()` | Plugin cleanup | 0 on success |
| `cmd_stats()` | Display stats | 0 on success |
| `cmd_stats_reset()` | Reset counters | 0 on success |
| `cmd_stats_export()` | Export JSON | 0 on success |
| `hook_file_operation_after()` | Track file ops | 0 to continue |
| `hook_session_end()` | Save on exit | 0 to continue |
| `hook_widget_register()` | Register widget | 0 to continue |
| `widget_render_code_stats()` | Render widget | 0 on success |
| `api_get_stats_json()` | Get stats JSON | Length or error |

### Host Functions Used

| Function | Purpose |
|----------|---------|
| `log(level, ptr, len)` | Logging |
| `get_context()` | Access host context |
| `register_widget(region, ptr, len)` | Register UI widget |
| `show_toast(level, ptr, len, duration)` | Show notification |
| `emit_event(name_ptr, name_len, data_ptr, data_len)` | Emit custom event |

## Testing

The plugin can be tested by:

1. Building and installing the plugin
2. Performing various file operations in Cortex
3. Running `/stats` to verify tracking
4. Running `/stats-reset` and verifying counters reset
5. Running `/stats-export` and verifying JSON output

## License

MIT License - See LICENSE file for details.
