# Cortex Plugin Security Model

This document describes the security architecture of the Cortex plugin system, including sandboxing, resource limits, and best practices for plugin developers.

## Table of Contents

- [Overview](#overview)
- [WASM Sandboxing](#wasm-sandboxing)
- [Resource Limits](#resource-limits)
- [Permission System](#permission-system)
- [Path Traversal Prevention](#path-traversal-prevention)
- [Network Security (SSRF Protection)](#network-security-ssrf-protection)
- [Dangerous Port Blocking](#dangerous-port-blocking)
- [Plugin Signing and Verification](#plugin-signing-and-verification)
- [Best Practices for Plugin Developers](#best-practices-for-plugin-developers)

## Overview

The Cortex plugin system implements multiple layers of security:

```
┌─────────────────────────────────────────────────────────────────┐
│                     Cortex Host Process                         │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                  Permission Layer                          │  │
│  │  ┌─────────────────────────────────────────────────────┐  │  │
│  │  │              Resource Limiter                        │  │  │
│  │  │  ┌───────────────────────────────────────────────┐  │  │  │
│  │  │  │           WASM Sandbox                         │  │  │  │
│  │  │  │  ┌─────────────────────────────────────────┐  │  │  │  │
│  │  │  │  │           Plugin Code                    │  │  │  │  │
│  │  │  │  │   (Isolated Linear Memory)               │  │  │  │  │
│  │  │  │  └─────────────────────────────────────────┘  │  │  │  │
│  │  │  └───────────────────────────────────────────────┘  │  │  │
│  │  └─────────────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## WASM Sandboxing

### Isolation Model

Plugins run in WebAssembly (WASM) sandboxes using the wasmtime runtime. This provides:

| Feature | Description |
|---------|-------------|
| Memory Isolation | Each plugin has its own linear memory, completely isolated from the host and other plugins |
| No Direct System Access | Plugins cannot directly access files, network, or system resources |
| Controlled Imports | Plugins can only call explicitly exported host functions |
| No Arbitrary Code Execution | WASM bytecode is validated before execution |

### How It Works

1. **Compilation**: Plugin WASM bytecode is validated and compiled to native code by wasmtime
2. **Instantiation**: A new instance is created with its own isolated memory
3. **Execution**: Code runs within the sandbox with controlled host function access
4. **Termination**: Memory is freed when the instance is dropped

### Security Guarantees

- **Memory safety**: WASM enforces bounds checking on all memory accesses
- **Type safety**: Function signatures are validated at link time
- **Control flow integrity**: WASM's structured control flow prevents ROP/JOP attacks
- **No ambient authority**: Plugins start with no capabilities; all access must be explicitly granted

## Resource Limits

### Memory Limits

| Limit | Value | Description |
|-------|-------|-------------|
| Maximum Memory | 16 MB | Hard limit per plugin instance |
| Memory Pages | 256 pages | Each page is 64KB |
| Initial Memory | Configurable | Set in plugin.toml |

Configuration in `plugin.toml`:

```toml
[wasm]
memory_pages = 256  # 256 × 64KB = 16MB (maximum)
```

Memory growth beyond the limit is rejected:

```rust
// Host-side enforcement
fn memory_growing(&mut self, current: usize, desired: usize, _maximum: Option<usize>) -> anyhow::Result<bool> {
    if desired > MAX_MEMORY_SIZE {
        tracing::warn!("Plugin memory request denied: exceeds maximum allowed");
        return Ok(false);  // Deny the growth
    }
    Ok(true)
}
```

### CPU Limits (Fuel)

The runtime uses **fuel-based CPU limiting** to prevent infinite loops and excessive CPU usage:

| Limit | Value | Description |
|-------|-------|-------------|
| Default Fuel | 10,000,000 | Approximately 10 million operations |
| Timeout | 30 seconds | Default execution timeout |

When fuel is exhausted, execution stops with an error. This prevents:
- Infinite loops
- Computationally expensive operations
- Denial of service attacks

Configuration:

```toml
[wasm]
timeout_ms = 30000  # 30 seconds
```

### Table Limits

| Limit | Value | Description |
|-------|-------|-------------|
| Table Elements | 10,000 | Maximum function table entries |
| Instances | 10 | Maximum instances per plugin |
| Memories | 1 | One linear memory per instance |
| Tables | 10 | Maximum tables per instance |

## Permission System

### Permission Types

| Permission | Risk Level | Description |
|------------|------------|-------------|
| `read_file` | Medium | Read files from specific paths |
| `write_file` | High | Write files to specific paths |
| `execute` | High | Execute specific shell commands |
| `network` | Medium | Access specific network domains |
| `environment` | Low | Access environment variables |
| `config` | Low | Access configuration keys |
| `clipboard` | Medium | Access clipboard |
| `notifications` | Low | Show notifications |

### Declaring Permissions

In `plugin.toml`:

```toml
permissions = [
    # File access - specify exact paths or patterns
    { read_file = { paths = ["**/*.rs", "**/*.toml"] } },
    { write_file = { paths = [".cortex/plugins/my-plugin/**"] } },
    
    # Command execution - whitelist specific commands
    { execute = { commands = ["cargo", "rustc", "git"] } },
    
    # Network access - whitelist specific domains
    { network = { domains = ["api.github.com", "crates.io"] } },
    
    # Environment variables - optionally restrict to specific vars
    { environment = { vars = ["HOME", "PATH", "CARGO_HOME"] } },
    
    # Configuration keys
    { config = { keys = ["theme", "model"] } },
    
    # Simple permissions
    "clipboard",
    "notifications"
]
```

### Permission Enforcement

Permissions are enforced at two levels:

1. **Declaration**: Plugin must declare permissions in manifest
2. **Grant**: User must grant permissions in Cortex config

```toml
# In Cortex config
[[plugins]]
name = "my-plugin"
granted_permissions = ["read_files", "network"]
```

### Fail-Closed Design

All permission checks follow a **fail-closed** design:

- Empty allowlist = no access (not unlimited access)
- Unknown permission = denied
- Validation error = denied

```rust
fn is_command_allowed(&self, command: &str) -> bool {
    // SECURITY: Fail-closed - empty allowlist means no commands allowed
    if self.allowed_commands.is_empty() {
        return false;
    }
    self.allowed_commands.iter().any(|c| c == command || c == "*")
}
```

## Path Traversal Prevention

### Attack Vector

Path traversal attacks attempt to escape allowed directories using sequences like:
- `../../../etc/passwd`
- `/absolute/path/to/sensitive/file`
- Symlinks to sensitive locations

### Protection Mechanisms

1. **Path Canonicalization**
   
   All paths are canonicalized to resolve `..`, `.`, and symlinks:

   ```rust
   fn resolve_path(&self, path: &str) -> Result<PathBuf> {
       let resolved = if path.is_absolute() {
           path.to_path_buf()
       } else {
           self.cwd.join(path)
       };

       // SECURITY: Canonicalize to resolve `..`, `.`, and symlinks
       let canonical = resolved.canonicalize()?;

       // SECURITY: Verify the canonical path is within allowed boundaries
       if !canonical.starts_with(&self.cwd) {
           return Err(PluginError::PermissionDenied("Path escapes working directory"));
       }

       Ok(canonical)
   }
   ```

2. **Boundary Checking**
   
   After canonicalization, paths are verified to be within allowed directories.

3. **Symlink Resolution**
   
   Symlinks are resolved to their real paths before checking permissions.

### Example Blocked Attacks

| Attack | Result |
|--------|--------|
| `../../../etc/passwd` | Blocked: escapes working directory |
| `/etc/passwd` | Blocked: outside allowed paths |
| `./symlink-to-etc` | Blocked: resolves outside allowed paths |
| `file%00.txt` | Blocked: invalid path character |

## Network Security (SSRF Protection)

### Server-Side Request Forgery (SSRF)

SSRF attacks attempt to make the server access internal resources. The plugin system blocks:

### Blocked Hosts

| Category | Examples | Reason |
|----------|----------|--------|
| Localhost | `localhost`, `127.0.0.1`, `::1`, `0.0.0.0` | Local services |
| Private IPv4 | `192.168.*`, `10.*`, `172.16-31.*` | Internal network |
| Link-local | `169.254.*` | Cloud metadata (AWS, GCP, Azure) |
| Private domains | `*.local`, `*.internal`, `*.localhost` | Internal DNS |
| IPv6 private | `fe80:*`, `fc00:*`, `fd*` | Link-local, unique local |

### Implementation

```rust
fn is_private_host(host: &str) -> bool {
    // Localhost variations
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return true;
    }

    // Private IPv4 ranges
    if host.starts_with("192.168.") || host.starts_with("10.") {
        return true;
    }

    // AWS/GCP/Azure metadata endpoint
    if host == "169.254.169.254" {
        return true;
    }

    // Private domain suffixes
    if host.ends_with(".local") || host.ends_with(".internal") {
        return true;
    }

    false
}
```

### Protocol Restrictions

Only HTTP and HTTPS protocols are allowed:

```rust
match parsed.scheme() {
    "http" | "https" => {}  // Allowed
    _ => return false;       // Blocked: file://, ftp://, gopher://, etc.
}
```

## Dangerous Port Blocking

### Blocked Ports

The following ports are blocked to prevent access to internal services:

| Port | Service | Risk |
|------|---------|------|
| 22 | SSH | Remote access |
| 23 | Telnet | Unencrypted remote access |
| 25 | SMTP | Email relay |
| 53 | DNS | DNS manipulation |
| 110 | POP3 | Email access |
| 135 | RPC | Windows services |
| 139 | NetBIOS | Windows networking |
| 143 | IMAP | Email access |
| 445 | SMB | File shares |
| 1433 | MSSQL | Database |
| 1521 | Oracle | Database |
| 3306 | MySQL | Database |
| 3389 | RDP | Remote desktop |
| 5432 | PostgreSQL | Database |
| 5900 | VNC | Remote desktop |
| 6379 | Redis | Cache/database |
| 9200 | Elasticsearch | Search engine |
| 11211 | Memcached | Cache |
| 27017 | MongoDB | Database |

### Safe Ports

Standard web ports are allowed:
- 80 (HTTP)
- 443 (HTTPS)
- 8080, 8443 (Alternative HTTP/HTTPS)

## Plugin Signing and Verification

### Future Enhancement

Plugin signing is planned to provide:

- **Authenticity**: Verify the plugin author
- **Integrity**: Detect tampering
- **Trust levels**: Different trust for signed vs unsigned plugins

### Current Recommendations

Until signing is implemented:

1. **Source verification**: Only install plugins from trusted sources
2. **Code review**: Review plugin code before installation
3. **Permission review**: Carefully review requested permissions
4. **Sandbox testing**: Test plugins in isolated environments first

## Best Practices for Plugin Developers

### 1. Request Minimum Permissions

```toml
# ❌ Bad: Overly broad permissions
permissions = [
    { read_file = { paths = ["**/*"] } },
    { network = { domains = ["*"] } }
]

# ✅ Good: Specific permissions
permissions = [
    { read_file = { paths = ["src/**/*.rs", "Cargo.toml"] } },
    { network = { domains = ["api.example.com"] } }
]
```

### 2. Handle Errors Gracefully

```rust
#[no_mangle]
pub extern "C" fn cmd_my_command() -> i32 {
    // Don't panic - return error code
    match do_something() {
        Ok(_) => 0,
        Err(_) => {
            log_error("Operation failed");
            1  // Return error code, don't panic
        }
    }
}
```

### 3. Avoid Storing Secrets

```rust
// ❌ Bad: Hardcoded secrets
const API_KEY: &str = "sk_live_abc123";

// ✅ Good: Use config
let api_key = get_config("api_key");
```

### 4. Validate All Input

```rust
fn process_path(path: &str) -> Result<String> {
    // Validate input before use
    if path.is_empty() {
        return Err("Path cannot be empty");
    }
    if path.contains('\0') {
        return Err("Path contains null bytes");
    }
    // ... continue processing
}
```

### 5. Use Minimal Memory

```rust
// ❌ Bad: Large static allocations
static mut BUFFER: [u8; 10_000_000] = [0; 10_000_000];

// ✅ Good: Allocate as needed
fn process_data(data: &[u8]) {
    let mut buffer = Vec::with_capacity(data.len());
    // ... use buffer
    // buffer is freed when function returns
}
```

### 6. Don't Bypass Security Checks

```rust
// ❌ Bad: Trying to bypass permission checks
fn read_secret_file() {
    // Don't try to read files not in your permissions
    let content = read_file("/etc/shadow");  // Will fail
}

// ✅ Good: Work within permissions
fn read_config() {
    // Read files declared in permissions
    let content = read_file("config.toml");  // OK if permitted
}
```

### 7. Log Security Events

```rust
fn handle_sensitive_operation() {
    log_info("Starting sensitive operation");
    
    // ... do operation ...
    
    log_info("Sensitive operation completed");
}
```

### 8. Clean Up Resources

```rust
#[no_mangle]
pub extern "C" fn shutdown() -> i32 {
    // Clean up any resources
    cleanup_temp_files();
    close_connections();
    
    log_info("Plugin shutdown complete");
    0
}
```

### Security Checklist

Before publishing your plugin:

- [ ] Request only necessary permissions
- [ ] No hardcoded secrets or credentials
- [ ] All user input is validated
- [ ] Errors are handled gracefully (no panics)
- [ ] Resources are cleaned up on shutdown
- [ ] Security-sensitive operations are logged
- [ ] Code has been reviewed for vulnerabilities
- [ ] Plugin works within resource limits
