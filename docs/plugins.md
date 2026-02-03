# Plugin Development Guide

This guide covers the Cortex plugin system, including how plugins are managed and the roadmap for future capabilities.

## Overview

The Cortex plugin system provides a foundation for extending Cortex functionality. Plugins are organized in directories with manifest files that describe their metadata.

**Current Status:**
- ✅ Basic plugin management (list, install, remove, enable, disable)
- ✅ Plugin manifest files with metadata
- ✅ Plugin directory structure
- 🚧 WASM runtime integration (planned)
- 🚧 Capabilities and permissions system (planned)
- 🚧 Custom commands and hooks (planned)

> **Note:** The plugin system is in active development. This documentation describes both currently implemented features and planned capabilities. Sections marked with 🚧 describe features on the roadmap.

## Plugin Directory Structure

Plugins are stored in individual directories under `~/.cortex/plugins/`:

```
~/.cortex/plugins/
└── my-plugin/
    └── plugin.toml    # Manifest file (required)
```

## Plugin Manifest

Every plugin requires a `plugin.toml` manifest file that describes the plugin.

### Current Manifest Format

The following fields are currently supported:

```toml
# Plugin manifest (plugin.toml)
name = "my-plugin"
version = "1.0.0"
description = "A description of what the plugin does"
enabled = true
author = "Author Name <email@example.com>"
```

### Manifest Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Human-readable name of the plugin |
| `version` | string | Yes | Semantic version (e.g., "1.0.0") |
| `description` | string | No | Brief description of the plugin |
| `enabled` | boolean | No | Whether the plugin is enabled (default: true) |
| `author` | string | No | Plugin author information |

## Plugin CLI Commands

Cortex provides commands for managing plugins.

### List Plugins

```bash
# List all installed plugins
cortex plugin list

# Short form
cortex plugin ls

# Output as JSON
cortex plugin list --json

# List only enabled plugins
cortex plugin list --enabled

# List only disabled plugins
cortex plugin list --disabled
```

### Install Plugin

```bash
# Install from a local directory
cortex plugin install /path/to/plugin

# Short form
cortex plugin add my-plugin

# Install specific version
cortex plugin install my-plugin -v 1.2.0
cortex plugin install my-plugin --version 1.2.0

# Force reinstall
cortex plugin install my-plugin -f
cortex plugin install my-plugin --force
```

> **Note:** Currently, plugins can be installed from local directories. Registry-based installation is planned for future releases.

### Remove Plugin

```bash
# Remove a plugin
cortex plugin remove my-plugin

# Short forms
cortex plugin rm my-plugin
cortex plugin uninstall my-plugin

# Skip confirmation
cortex plugin remove my-plugin -y
cortex plugin remove my-plugin --yes
```

### Enable/Disable Plugin

```bash
# Enable a plugin
cortex plugin enable my-plugin

# Disable a plugin
cortex plugin disable my-plugin
```

### Show Plugin Details

```bash
# Show plugin information
cortex plugin show my-plugin

# Short form
cortex plugin info my-plugin

# Output as JSON
cortex plugin show my-plugin --json
```

## Creating a Plugin

### Basic Plugin Setup

1. Create a plugin directory:

```bash
mkdir -p ~/.cortex/plugins/my-plugin/
```

2. Create a `plugin.toml` manifest:

```toml
name = "My Plugin"
version = "1.0.0"
description = "A simple plugin for Cortex"
enabled = true
author = "Your Name <your.email@example.com>"
```

3. Verify installation:

```bash
cortex plugin list
cortex plugin show my-plugin
```

## Best Practices

### Plugin Development

1. **Use semantic versioning** - Follow semver for your plugin version
2. **Write clear descriptions** - Help users understand what your plugin does
3. **Test thoroughly** - Verify your plugin works correctly before sharing

### Plugin Management

1. **Review before enabling** - Check plugin details before enabling
2. **Disable unused plugins** - Keep only the plugins you need enabled
3. **Update regularly** - Keep plugins up to date

---

## 🚧 Roadmap: Planned Features

The following sections describe features that are planned for future releases. They are included here to provide visibility into the direction of the plugin system.

### WASM Runtime Support (Planned)

Future versions will support WebAssembly (WASM) plugins compiled with the `wasm32-wasi` target, providing:

- Sandboxed execution environment
- Cross-platform compatibility
- Security isolation

**Planned manifest extension:**

```toml
[wasm]
memory_pages = 256
timeout_ms = 30000
wasi_enabled = true
```

### Capabilities System (Planned)

Plugins will be able to declare capabilities that define what features they provide:

| Capability | Description |
|------------|-------------|
| `commands` | Provide custom slash commands |
| `hooks` | Register hooks for various events |
| `tools` | Provide MCP-style tools |
| `config` | Access configuration system |
| `filesystem` | Access file system (requires permissions) |
| `shell` | Execute shell commands (requires permissions) |
| `network` | Make network requests (requires permissions) |

### Permissions System (Planned)

A fine-grained permissions system will control what resources plugins can access:

```toml
permissions = [
    { read_file = { paths = ["**/*.rs"] } },
    { write_file = { paths = ["output/**"] } },
    { execute = { commands = ["ls", "cat"] } },
    { network = { domains = ["api.example.com"] } },
    { environment = { vars = ["API_KEY"] } },
]
```

### Custom Commands (Planned)

Plugins will be able to provide custom slash commands:

```toml
[[commands]]
name = "mycommand"
aliases = ["mc"]
description = "My custom command"
usage = "/mycommand [args]"
category = "utilities"

[[commands.args]]
name = "arg1"
description = "Argument description"
required = false
default = "default"
arg_type = "string"
```

### Hook System (Planned)

Plugins will be able to register hooks to intercept and respond to various events:

**Tool Hooks:**
- `tool_execute_before` - Called before a tool executes
- `tool_execute_after` - Called after a tool executes

**AI Hooks:**
- `prompt_inject` - Inject content into prompts
- `ai_response_before` - Called before AI generates response
- `ai_response_after` - Called after AI response completes

**Session Hooks:**
- `session_start` - Called when a session starts
- `session_end` - Called when a session ends

**File Hooks:**
- `file_operation_before` - Called before file operations
- `file_operation_after` - Called after file operations

### Building WASM Plugins (Planned)

When WASM support is implemented, plugins will be built using:

```bash
# Prerequisites
rustup target add wasm32-wasi

# Build
cargo build --target wasm32-wasi --release
```

---

## Troubleshooting

### Plugin Not Found

1. Check that the plugin directory exists under `~/.cortex/plugins/`
2. Verify `plugin.toml` exists in the plugin directory
3. Ensure the plugin name matches the directory name

### Plugin Not Loading

1. Check that `plugin.toml` is valid TOML syntax
2. Verify required fields (`name`, `version`) are present
3. Check Cortex logs for error messages

### Enable/Disable Not Working

1. Verify the plugin is installed: `cortex plugin list`
2. Check that the manifest can be updated (file permissions)
3. Try removing and reinstalling the plugin
