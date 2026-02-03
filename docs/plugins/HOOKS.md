# Cortex Plugin Hooks Reference

Hooks allow plugins to intercept and modify Cortex behavior at various points. This document provides a complete reference for all available hook types.

## Table of Contents

- [Overview](#overview)
- [Hook Registration](#hook-registration)
- [Hook Types](#hook-types)
  - [Tool Hooks](#tool-hooks)
  - [Chat Hooks](#chat-hooks)
  - [Permission Hooks](#permission-hooks)
  - [Prompt/AI Hooks](#promptai-hooks)
  - [Session Hooks](#session-hooks)
  - [File Operation Hooks](#file-operation-hooks)
  - [Command Hooks](#command-hooks)
  - [Input Hooks](#input-hooks)
  - [Error Hooks](#error-hooks)
  - [Config Hooks](#config-hooks)
  - [Workspace Hooks](#workspace-hooks)
  - [Clipboard Hooks](#clipboard-hooks)
  - [UI Hooks](#ui-hooks)
  - [TUI Event Hooks](#tui-event-hooks)
  - [Focus Hooks](#focus-hooks)
- [Hook Results](#hook-results)
- [Examples](#examples)

## Overview

### Hook Execution Flow

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   Event      │ ──▶ │  Hook Call   │ ──▶ │   Result     │
│   Triggered  │     │  (by priority)│     │   Applied    │
└──────────────┘     └──────────────┘     └──────────────┘
                            │
                            ▼
                     ┌──────────────┐
                     │ Plugin 1     │
                     │ (priority 10)│
                     └──────────────┘
                            │
                            ▼
                     ┌──────────────┐
                     │ Plugin 2     │
                     │ (priority 50)│
                     └──────────────┘
                            │
                            ▼
                     ┌──────────────┐
                     │ Plugin 3     │
                     │ (priority 100)│
                     └──────────────┘
```

Hooks are called in priority order (lower values run first). Each hook can:
- **Continue**: Allow normal execution to proceed
- **Skip**: Skip this hook's processing but allow others
- **Abort**: Stop execution entirely
- **Replace**: Replace the result with custom data

## Hook Registration

Register hooks in your `plugin.toml`:

```toml
[[hooks]]
hook_type = "tool_execute_before"
priority = 50
pattern = "*.rs"              # Optional: pattern filter
function = "my_hook_handler"  # Optional: custom function name

[[hooks]]
hook_type = "session_start"
priority = 100
```

Default priority is 100. Lower values execute first.

## Hook Types

### Tool Hooks

#### tool_execute_before

Called before a tool is executed.

| Property | Value |
|----------|-------|
| Hook Type | `tool_execute_before` |
| Trigger | Before any tool execution |
| Can Modify | Tool arguments |
| Can Abort | Yes |

**Input Structure:**
```json
{
    "tool": "read_file",
    "session_id": "abc123",
    "call_id": "def456",
    "args": {
        "path": "/path/to/file.txt"
    }
}
```

**Return Values:**
| Value | Effect |
|-------|--------|
| 0 | Continue with (possibly modified) args |
| 1 | Skip this tool call |
| 2 | Abort with error |

**Example:**
```rust
#[no_mangle]
pub extern "C" fn hook_tool_execute_before() -> i32 {
    log_debug("Tool about to execute");
    0 // Continue
}
```

---

#### tool_execute_after

Called after a tool completes execution.

| Property | Value |
|----------|-------|
| Hook Type | `tool_execute_after` |
| Trigger | After tool execution completes |
| Can Modify | Tool output |
| Can Abort | Yes (for subsequent processing) |

**Input Structure:**
```json
{
    "tool": "read_file",
    "session_id": "abc123",
    "call_id": "def456",
    "success": true,
    "duration_ms": 150,
    "output": "file contents..."
}
```

**Return Values:**
| Value | Effect |
|-------|--------|
| 0 | Continue with (possibly modified) output |
| 1 | Skip further processing |
| 2 | Mark as failed |

---

### Chat Hooks

#### chat_message

Called when processing chat messages.

| Property | Value |
|----------|-------|
| Hook Type | `chat_message` |
| Trigger | When a chat message is processed |
| Can Modify | Message content |
| Can Abort | Yes |

**Input Structure:**
```json
{
    "session_id": "abc123",
    "role": "user",
    "message_id": "msg789",
    "agent": "default",
    "model": "claude-3-opus",
    "content": "Hello, can you help me?"
}
```

**Return Values:**
| Value | Effect |
|-------|--------|
| 0 | Continue with (possibly modified) message |
| 1 | Skip message processing |
| 2 | Abort with error |

---

### Permission Hooks

#### permission_ask

Called when a permission is requested.

| Property | Value |
|----------|-------|
| Hook Type | `permission_ask` |
| Trigger | When permission is requested |
| Can Modify | Permission decision |
| Can Abort | N/A |

**Input Structure:**
```json
{
    "session_id": "abc123",
    "permission": "file_read",
    "resource": "/path/to/sensitive/file",
    "reason": "Required to analyze code"
}
```

**Return Values:**
| Value | Effect |
|-------|--------|
| 0 | Defer to user (Ask) |
| 1 | Auto-allow (requires trust) |
| 2 | Auto-deny |

**Security Note:** Only trusted system plugins should return auto-allow (1). Third-party plugins auto-allowing permissions is a security risk.

---

### Prompt/AI Hooks

#### prompt_inject

Modify prompts before AI processing.

| Property | Value |
|----------|-------|
| Hook Type | `prompt_inject` |
| Trigger | Before prompt sent to AI |
| Can Modify | Prompt content |
| Can Abort | No |

**Input Structure:**
```json
{
    "session_id": "abc123",
    "prompt": "Original prompt content...",
    "model": "claude-3-opus",
    "context": {}
}
```

---

#### ai_response_before

Called before AI response starts.

| Property | Value |
|----------|-------|
| Hook Type | `ai_response_before` |
| Trigger | Before AI begins responding |
| Can Modify | Request parameters |
| Can Abort | Yes |

---

#### ai_response_stream

Called during AI streaming response.

| Property | Value |
|----------|-------|
| Hook Type | `ai_response_stream` |
| Trigger | During streaming chunks |
| Can Modify | Stream content |
| Can Abort | Yes (stops stream) |

---

#### ai_response_after

Called after AI response completes.

| Property | Value |
|----------|-------|
| Hook Type | `ai_response_after` |
| Trigger | After AI response completes |
| Can Modify | Final response |
| Can Abort | No |

---

### Session Hooks

#### session_start

Called when a session starts.

| Property | Value |
|----------|-------|
| Hook Type | `session_start` |
| Trigger | Session initialization |
| Can Modify | System prompt additions, greeting |
| Can Abort | Yes |

**Input Structure:**
```json
{
    "session_id": "abc123",
    "agent": "default",
    "model": "claude-3-opus",
    "cwd": "/path/to/project",
    "resumed": false
}
```

**Output Capabilities:**
- Add system prompt content
- Set greeting message
- Initialize plugin state

---

#### session_end

Called when a session ends.

| Property | Value |
|----------|-------|
| Hook Type | `session_end` |
| Trigger | Session termination |
| Can Modify | N/A |
| Can Abort | No |

**Input Structure:**
```json
{
    "session_id": "abc123",
    "duration_secs": 3600,
    "total_messages": 42,
    "total_tokens": 15000,
    "saved": true
}
```

**Use Cases:**
- Generate session summaries
- Export chat logs
- Cleanup resources

---

### File Operation Hooks

#### file_operation_before

Called before a file operation.

| Property | Value |
|----------|-------|
| Hook Type | `file_operation_before` |
| Trigger | Before read/write/delete |
| Can Modify | Operation parameters |
| Can Abort | Yes |

**Input Structure:**
```json
{
    "operation": "write",
    "path": "/path/to/file.txt",
    "content": "new content...",
    "session_id": "abc123"
}
```

---

#### file_operation_after

Called after a file operation.

| Property | Value |
|----------|-------|
| Hook Type | `file_operation_after` |
| Trigger | After operation completes |
| Can Modify | Result |
| Can Abort | No |

---

#### file_edited

Legacy hook for file edits (kept for compatibility).

| Property | Value |
|----------|-------|
| Hook Type | `file_edited` |
| Trigger | After file is edited |
| Can Modify | N/A |
| Can Abort | No |

---

### Command Hooks

#### command_execute_before

Called before a command executes.

| Property | Value |
|----------|-------|
| Hook Type | `command_execute_before` |
| Trigger | Before shell command |
| Can Modify | Command arguments |
| Can Abort | Yes |

**Input Structure:**
```json
{
    "command": ["ls", "-la", "/path"],
    "cwd": "/working/directory",
    "session_id": "abc123"
}
```

---

#### command_execute_after

Called after a command completes.

| Property | Value |
|----------|-------|
| Hook Type | `command_execute_after` |
| Trigger | After shell command |
| Can Modify | Output |
| Can Abort | No |

**Input Structure:**
```json
{
    "command": ["ls", "-la"],
    "exit_code": 0,
    "stdout": "...",
    "stderr": "",
    "duration_ms": 50
}
```

---

### Input Hooks

#### input_intercept

Intercept user input.

| Property | Value |
|----------|-------|
| Hook Type | `input_intercept` |
| Trigger | User types input |
| Can Modify | Input text |
| Can Abort | Yes (cancels input) |

**Input Structure:**
```json
{
    "input": "user typed text",
    "session_id": "abc123",
    "cursor_position": 15
}
```

---

### Error Hooks

#### error_handle

Handle errors that occur.

| Property | Value |
|----------|-------|
| Hook Type | `error_handle` |
| Trigger | When an error occurs |
| Can Modify | Error response |
| Can Abort | No |

**Input Structure:**
```json
{
    "session_id": "abc123",
    "error": "Connection timeout",
    "error_code": "E_TIMEOUT",
    "source": "network",
    "retriable": true,
    "retry_count": 0
}
```

---

### Config Hooks

#### config_changed

Called when configuration changes.

| Property | Value |
|----------|-------|
| Hook Type | `config_changed` |
| Trigger | Config value modified |
| Can Modify | N/A |
| Can Abort | No |

**Input Structure:**
```json
{
    "key": "theme",
    "old_value": "ocean",
    "new_value": "forest",
    "session_id": "abc123"
}
```

---

#### model_changed

Called when the AI model changes.

| Property | Value |
|----------|-------|
| Hook Type | `model_changed` |
| Trigger | Model switch |
| Can Modify | N/A |
| Can Abort | No |

**Input Structure:**
```json
{
    "old_model": "claude-3-opus",
    "new_model": "gpt-4",
    "session_id": "abc123"
}
```

---

### Workspace Hooks

#### workspace_changed

Called when workspace/working directory changes.

| Property | Value |
|----------|-------|
| Hook Type | `workspace_changed` |
| Trigger | Directory change |
| Can Modify | N/A |
| Can Abort | No |

**Input Structure:**
```json
{
    "old_cwd": "/old/path",
    "new_cwd": "/new/path",
    "session_id": "abc123"
}
```

---

### Clipboard Hooks

#### clipboard_copy

Called before clipboard copy.

| Property | Value |
|----------|-------|
| Hook Type | `clipboard_copy` |
| Trigger | Copy operation |
| Can Modify | Clipboard content |
| Can Abort | Yes |

**Input Structure:**
```json
{
    "content": "text being copied",
    "session_id": "abc123"
}
```

---

#### clipboard_paste

Called before clipboard paste.

| Property | Value |
|----------|-------|
| Hook Type | `clipboard_paste` |
| Trigger | Paste operation |
| Can Modify | Paste content |
| Can Abort | Yes |

**Input Structure:**
```json
{
    "content": "text being pasted",
    "session_id": "abc123"
}
```

---

### UI Hooks

#### ui_render

Customize UI rendering.

| Property | Value |
|----------|-------|
| Hook Type | `ui_render` |
| Trigger | UI render cycle |
| Can Modify | Styles, widgets |
| Can Abort | No |

**Input Structure:**
```json
{
    "session_id": "abc123",
    "component": "message_area",
    "theme": "ocean",
    "dimensions": [120, 40]
}
```

---

#### widget_register

Register custom widgets.

| Property | Value |
|----------|-------|
| Hook Type | `widget_register` |
| Trigger | Plugin initialization |
| Can Modify | Widget registry |
| Can Abort | No |

---

#### key_binding

Register keyboard shortcuts.

| Property | Value |
|----------|-------|
| Hook Type | `key_binding` |
| Trigger | Key press |
| Can Modify | Action |
| Can Abort | Yes (consume key) |

---

#### theme_override

Override theme settings.

| Property | Value |
|----------|-------|
| Hook Type | `theme_override` |
| Trigger | Theme application |
| Can Modify | Theme colors/styles |
| Can Abort | No |

---

#### layout_customize

Customize layout.

| Property | Value |
|----------|-------|
| Hook Type | `layout_customize` |
| Trigger | Layout calculation |
| Can Modify | Layout parameters |
| Can Abort | No |

---

#### modal_inject

Inject modal dialogs.

| Property | Value |
|----------|-------|
| Hook Type | `modal_inject` |
| Trigger | Modal display |
| Can Modify | Modal content |
| Can Abort | Yes |

---

#### toast_show

Show toast notifications.

| Property | Value |
|----------|-------|
| Hook Type | `toast_show` |
| Trigger | Notification |
| Can Modify | Toast content |
| Can Abort | Yes |

---

### TUI Event Hooks

#### tui_event_subscribe

Subscribe to TUI events.

| Property | Value |
|----------|-------|
| Hook Type | `tui_event_subscribe` |
| Trigger | Event registration |
| Can Modify | Subscription list |
| Can Abort | No |

---

#### tui_event_dispatch

Handle TUI events.

| Property | Value |
|----------|-------|
| Hook Type | `tui_event_dispatch` |
| Trigger | Event dispatch |
| Can Modify | Event handling |
| Can Abort | Yes (consume event) |

---

#### custom_event_emit

Emit custom events.

| Property | Value |
|----------|-------|
| Hook Type | `custom_event_emit` |
| Trigger | Custom event |
| Can Modify | Event data |
| Can Abort | No |

---

#### event_intercept

Intercept events.

| Property | Value |
|----------|-------|
| Hook Type | `event_intercept` |
| Trigger | Any event |
| Can Modify | Event |
| Can Abort | Yes |

---

#### animation_frame

Animation frame callback.

| Property | Value |
|----------|-------|
| Hook Type | `animation_frame` |
| Trigger | Each frame |
| Can Modify | Animation state |
| Can Abort | No |

**Input:**
- `frame`: Frame number (u64)
- `delta_us`: Microseconds since last frame (u64)

**Return Values:**
| Value | Effect |
|-------|--------|
| 0 | Stop animations |
| 1 | Request another frame |

---

### Focus Hooks

#### focus_change

Called when focus changes.

| Property | Value |
|----------|-------|
| Hook Type | `focus_change` |
| Trigger | Focus gained/lost |
| Can Modify | N/A |
| Can Abort | No |

**Input:**
- `focused`: Whether window/component is focused (i32: 0=lost, 1=gained)

---

## Hook Results

Hooks return an integer to indicate the result:

| Return Value | Constant | Description |
|--------------|----------|-------------|
| 0 | `HOOK_CONTINUE` | Continue with normal execution |
| 1 | `HOOK_SKIP` | Skip this operation |
| 2 | `HOOK_ABORT` | Abort execution with error |

## Examples

### Example 1: Logging All Tool Executions

```toml
# plugin.toml
[[hooks]]
hook_type = "tool_execute_before"
priority = 10

[[hooks]]
hook_type = "tool_execute_after"
priority = 10
```

```rust
#[no_mangle]
pub extern "C" fn hook_tool_execute_before() -> i32 {
    log_info("Tool execution starting...");
    0 // Continue
}

#[no_mangle]
pub extern "C" fn hook_tool_execute_after() -> i32 {
    log_info("Tool execution completed");
    0 // Continue
}
```

### Example 2: Blocking Dangerous Commands

```toml
# plugin.toml
[[hooks]]
hook_type = "command_execute_before"
priority = 1  # Run early to block before others
```

```rust
static BLOCKED_COMMANDS: &[&str] = &["rm", "rmdir", "del"];

#[no_mangle]
pub extern "C" fn hook_command_execute_before() -> i32 {
    // In a real implementation, you'd parse the input
    // For now, just demonstrate the pattern
    log_warn("Checking command against blocklist...");
    
    // Return 2 to abort if blocked, 0 to continue
    0
}
```

### Example 3: Session Summary Plugin

```toml
# plugin.toml
[[hooks]]
hook_type = "session_start"
priority = 100

[[hooks]]
hook_type = "session_end"
priority = 100
```

```rust
#[no_mangle]
pub extern "C" fn hook_session_start() -> i32 {
    log_info("Session started - tracking activity...");
    0
}

#[no_mangle]
pub extern "C" fn hook_session_end() -> i32 {
    log_info("Session ended - generating summary...");
    // Generate and save summary
    0
}
```

### Example 4: Custom Status Widget

```toml
# plugin.toml
[[hooks]]
hook_type = "ui_render"
priority = 50
```

```rust
#[no_mangle]
pub extern "C" fn init() -> i32 {
    // Register widget in status bar
    unsafe {
        register_widget(7, "my_status_widget".as_ptr() as i32, 16);
    }
    0
}

#[no_mangle]
pub extern "C" fn hook_ui_render() -> i32 {
    // Update widget content each render
    0
}
```

### Example 5: Keyboard Shortcut Handler

```toml
# plugin.toml
capabilities = ["hooks"]

[[hooks]]
hook_type = "key_binding"
priority = 50
```

```rust
#[no_mangle]
pub extern "C" fn init() -> i32 {
    // Register Ctrl+Shift+P shortcut
    unsafe {
        register_keybinding(
            "ctrl+shift+p".as_ptr() as i32, 14,
            "my_plugin_action".as_ptr() as i32, 16
        );
    }
    0
}

#[no_mangle]
pub extern "C" fn action_my_plugin_action() -> i32 {
    log_info("Custom shortcut triggered!");
    unsafe {
        show_toast(1, "Action executed!".as_ptr() as i32, 16, 2000);
    }
    0
}
```
