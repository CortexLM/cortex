//! Hello World Plugin - A basic Cortex plugin example
//!
//! This plugin demonstrates fundamental plugin capabilities:
//! - Custom slash commands (`/hello`)
//! - Hook handlers (`tool_execute_before`)
//! - Configuration usage
//!
//! Build with: cargo build --target wasm32-wasi --release

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;

// ============================================================================
// Host function imports from the "cortex" module
// ============================================================================

#[link(wasm_import_module = "cortex")]
extern "C" {
    /// Log a message at the specified level.
    /// level: 0=trace, 1=debug, 2=info, 3=warn, 4=error
    fn log(level: i32, msg_ptr: i32, msg_len: i32);

    /// Get context JSON (returns length of JSON string)
    fn get_context() -> i64;

    /// Show a toast notification
    /// level: 0=info, 1=success, 2=warning, 3=error
    fn show_toast(level: i32, msg_ptr: i32, msg_len: i32, duration_ms: i32) -> i32;
}

// ============================================================================
// Logging helpers
// ============================================================================

/// Log a message at the specified level via host function.
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

/// Log at trace level (level 0).
#[allow(dead_code)]
fn log_trace(msg: &str) {
    log_message(0, msg);
}

/// Log at debug level (level 1).
fn log_debug(msg: &str) {
    log_message(1, msg);
}

/// Log at info level (level 2).
fn log_info(msg: &str) {
    log_message(2, msg);
}

/// Log at warn level (level 3).
#[allow(dead_code)]
fn log_warn(msg: &str) {
    log_message(3, msg);
}

/// Log at error level (level 4).
#[allow(dead_code)]
fn log_error(msg: &str) {
    log_message(4, msg);
}

// ============================================================================
// Toast notification helpers
// ============================================================================

/// Toast notification levels matching host expectations.
#[repr(i32)]
enum ToastLevel {
    Info = 0,
    Success = 1,
    #[allow(dead_code)]
    Warning = 2,
    #[allow(dead_code)]
    Error = 3,
}

/// Display a toast notification to the user.
fn show_notification(level: ToastLevel, message: &str, duration_ms: i32) {
    // SAFETY: FFI call to host-provided `show_toast` function.
    // Contract with the host runtime:
    // 1. `show_toast` is a valid function pointer provided by the WASM runtime
    // 2. The level is passed by value and validated by the host (invalid = Info)
    // 3. The message string is passed as (ptr, len) and copied by the host
    // 4. duration_ms is passed by value; invalid values are clamped by host
    // 5. The host does not retain the message pointer past this call
    // 6. Return value indicates success (0) or failure (non-zero)
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
// Plugin lifecycle functions
// ============================================================================

/// Plugin initialization - called when the plugin is loaded.
///
/// This function is invoked once when Cortex loads the plugin.
/// Use it to set up any initial state and register resources.
///
/// # Returns
/// - `0` on success
/// - Non-zero on failure (plugin will not be activated)
#[no_mangle]
pub extern "C" fn init() -> i32 {
    log_info("Hello World plugin initializing...");

    // Verify we can access the host context
    // SAFETY: FFI call to host-provided `get_context` function.
    // The function returns a length value indicating context availability.
    // A positive value means context is available; negative indicates error.
    let context_len = unsafe { get_context() };
    if context_len > 0 {
        log_debug("Host context is available");
    } else {
        log_debug("Host context returned error or empty response");
    }

    log_info("Hello World plugin initialized successfully");
    0 // Success
}

/// Plugin shutdown - called when the plugin is unloaded.
///
/// This function is invoked when Cortex unloads the plugin.
/// Use it to clean up any resources allocated during the plugin's lifetime.
///
/// # Returns
/// - `0` on success
/// - Non-zero on failure (logged but doesn't prevent unloading)
#[no_mangle]
pub extern "C" fn shutdown() -> i32 {
    log_info("Hello World plugin shutting down");
    0 // Success
}

// ============================================================================
// Command handlers
// ============================================================================

/// Handler for the `/hello` command.
///
/// This command greets the user with a customizable message.
/// The greeting prefix can be configured via the plugin's config.
///
/// Usage: `/hello [name]`
///
/// # Returns
/// - `0` on success
/// - Non-zero on failure
#[no_mangle]
pub extern "C" fn cmd_hello() -> i32 {
    log_info("Hello command executed");

    // In a real implementation, we would:
    // 1. Read the command arguments from a shared buffer
    // 2. Read the greeting_prefix from config
    // 3. Format the message accordingly
    //
    // For this example, we use a default greeting since we don't have
    // access to the full argument passing mechanism yet.
    let greeting_prefix = "Hello";
    let name = "World";
    let message = format!("{}, {}!", greeting_prefix, name);

    // Show a toast notification with the greeting
    show_notification(ToastLevel::Success, &message, 3000);

    log_debug("Hello command completed successfully");
    0 // Success
}

// ============================================================================
// Hook handlers
// ============================================================================

/// Hook handler for `tool_execute_before`.
///
/// This hook is called before any tool is executed, allowing plugins
/// to log, modify, or prevent tool execution.
///
/// # Returns
/// - `0` to continue with tool execution
/// - `1` to skip this tool execution
/// - `2` to abort the entire operation
#[no_mangle]
pub extern "C" fn hook_tool_execute_before() -> i32 {
    log_debug("Tool execution intercepted by hello-world plugin");

    // In a real implementation, we could:
    // 1. Read tool information from a shared buffer
    // 2. Log details about which tool is being executed
    // 3. Optionally modify tool parameters
    // 4. Decide whether to allow, skip, or abort the execution

    // For this example, we simply log that a tool is about to execute
    // and allow it to proceed normally.
    log_info("Tool about to execute - logging from hello-world plugin");

    0 // Continue with tool execution
}

// ============================================================================
// Panic handler (required for #![no_std])
// ============================================================================

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Attempt to log panic information before halting
    if let Some(location) = info.location() {
        let file = location.file();
        let line = location.line();
        // We can't easily format strings in no_std without allocating,
        // so we log a generic message
        let _ = (file, line); // Suppress unused warnings
        log_message(4, "PANIC: hello-world plugin encountered a fatal error");
    } else {
        log_message(4, "PANIC: hello-world plugin panicked at unknown location");
    }

    // Enter infinite loop as required by panic handler
    loop {
        core::hint::spin_loop();
    }
}

// ============================================================================
// Global allocator (required for alloc crate)
// ============================================================================

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
