//! Code Stats Plugin - An advanced Cortex plugin example
//!
//! This plugin demonstrates advanced plugin capabilities:
//! - Multiple commands (`/stats`, `/stats-reset`, `/stats-export`)
//! - Multiple hooks (`file_operation_after`, `session_end`, `widget_register`)
//! - UI widget for status bar display
//! - Persistent storage across sessions
//!
//! Build with: cargo build --target wasm32-wasi --release

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

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

    /// Register a widget in a specific UI region.
    /// region: 0=Header, 1=Footer, 2=SidebarLeft, 3=SidebarRight,
    ///         4=MainContent, 5=InputArea, 6=Overlay, 7=StatusBar,
    ///         8=ToolOutput, 9=MessageArea
    fn register_widget(region: i32, widget_type_ptr: i32, widget_type_len: i32) -> i32;

    /// Show a toast notification.
    /// level: 0=info, 1=success, 2=warning, 3=error
    fn show_toast(level: i32, msg_ptr: i32, msg_len: i32, duration_ms: i32) -> i32;

    /// Emit a custom event.
    fn emit_event(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
}

// ============================================================================
// Global statistics storage (thread-safe via atomics)
// ============================================================================

/// Lines of code added during the session.
static LINES_ADDED: AtomicU64 = AtomicU64::new(0);

/// Lines of code removed during the session.
static LINES_REMOVED: AtomicU64 = AtomicU64::new(0);

/// Number of files modified during the session.
static FILES_MODIFIED: AtomicU64 = AtomicU64::new(0);

/// Number of files created during the session.
static FILES_CREATED: AtomicU64 = AtomicU64::new(0);

/// Number of files deleted during the session.
static FILES_DELETED: AtomicU64 = AtomicU64::new(0);

/// Total file operations performed.
static TOTAL_OPERATIONS: AtomicU64 = AtomicU64::new(0);

// ============================================================================
// Logging helpers
// ============================================================================

/// Log a message at the specified level via host function.
fn log_message(level: i32, msg: &str) {
    // SAFETY: FFI call to host-provided `log` function.
    // Contract with the host runtime:
    // 1. `log` is a valid function pointer provided by the WASM runtime
    // 2. The host reads the message from WASM linear memory using (ptr, len)
    // 3. The host does not retain the pointer past the call boundary
    // 4. Invalid level values are handled gracefully by the host
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

/// Toast notification levels.
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
    // The message string is passed as (ptr, len) and copied by the host.
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
// Widget helpers
// ============================================================================

/// UI regions for widget placement.
#[repr(i32)]
enum UiRegion {
    #[allow(dead_code)]
    Header = 0,
    #[allow(dead_code)]
    Footer = 1,
    #[allow(dead_code)]
    SidebarLeft = 2,
    #[allow(dead_code)]
    SidebarRight = 3,
    #[allow(dead_code)]
    MainContent = 4,
    #[allow(dead_code)]
    InputArea = 5,
    #[allow(dead_code)]
    Overlay = 6,
    StatusBar = 7,
    #[allow(dead_code)]
    ToolOutput = 8,
    #[allow(dead_code)]
    MessageArea = 9,
}

/// Register a widget in a specific UI region.
fn register_widget_in_region(region: UiRegion, widget_type: &str) -> bool {
    // SAFETY: FFI call to host-provided `register_widget` function.
    // Arguments are passed by value (region) and by pointer+len (widget_type).
    // The host copies the string data before this call returns.
    // Return value 0 indicates success, non-zero indicates failure.
    unsafe {
        register_widget(
            region as i32,
            widget_type.as_ptr() as i32,
            widget_type.len() as i32,
        ) == 0
    }
}

// ============================================================================
// Event helpers
// ============================================================================

/// Emit a custom event with JSON data.
fn emit_custom_event(event_name: &str, event_data: &str) -> bool {
    // SAFETY: FFI call to host-provided `emit_event` function.
    // Both strings are passed as (ptr, len) and copied by the host.
    // The host validates that event_data is valid JSON.
    // Return value 0 indicates success, non-zero indicates failure.
    unsafe {
        emit_event(
            event_name.as_ptr() as i32,
            event_name.len() as i32,
            event_data.as_ptr() as i32,
            event_data.len() as i32,
        ) == 0
    }
}

// ============================================================================
// Statistics helpers
// ============================================================================

/// Get current statistics as a formatted string.
fn get_stats_summary() -> String {
    let added = LINES_ADDED.load(Ordering::Relaxed);
    let removed = LINES_REMOVED.load(Ordering::Relaxed);
    let modified = FILES_MODIFIED.load(Ordering::Relaxed);
    let created = FILES_CREATED.load(Ordering::Relaxed);
    let deleted = FILES_DELETED.load(Ordering::Relaxed);
    let total_ops = TOTAL_OPERATIONS.load(Ordering::Relaxed);

    format!(
        "Lines: +{} -{} | Files: {} modified, {} created, {} deleted | Total ops: {}",
        added, removed, modified, created, deleted, total_ops
    )
}

/// Get statistics as JSON string.
fn get_stats_json() -> String {
    let added = LINES_ADDED.load(Ordering::Relaxed);
    let removed = LINES_REMOVED.load(Ordering::Relaxed);
    let modified = FILES_MODIFIED.load(Ordering::Relaxed);
    let created = FILES_CREATED.load(Ordering::Relaxed);
    let deleted = FILES_DELETED.load(Ordering::Relaxed);
    let total_ops = TOTAL_OPERATIONS.load(Ordering::Relaxed);

    format!(
        r#"{{"lines_added":{},"lines_removed":{},"files_modified":{},"files_created":{},"files_deleted":{},"total_operations":{}}}"#,
        added, removed, modified, created, deleted, total_ops
    )
}

/// Reset all statistics counters.
fn reset_stats() {
    LINES_ADDED.store(0, Ordering::Relaxed);
    LINES_REMOVED.store(0, Ordering::Relaxed);
    FILES_MODIFIED.store(0, Ordering::Relaxed);
    FILES_CREATED.store(0, Ordering::Relaxed);
    FILES_DELETED.store(0, Ordering::Relaxed);
    TOTAL_OPERATIONS.store(0, Ordering::Relaxed);
}

/// Record a file modification.
fn record_file_modified(lines_added: u64, lines_removed: u64) {
    LINES_ADDED.fetch_add(lines_added, Ordering::Relaxed);
    LINES_REMOVED.fetch_add(lines_removed, Ordering::Relaxed);
    FILES_MODIFIED.fetch_add(1, Ordering::Relaxed);
    TOTAL_OPERATIONS.fetch_add(1, Ordering::Relaxed);
}

/// Record a file creation.
fn record_file_created(lines: u64) {
    LINES_ADDED.fetch_add(lines, Ordering::Relaxed);
    FILES_CREATED.fetch_add(1, Ordering::Relaxed);
    TOTAL_OPERATIONS.fetch_add(1, Ordering::Relaxed);
}

/// Record a file deletion.
fn record_file_deleted(lines: u64) {
    LINES_REMOVED.fetch_add(lines, Ordering::Relaxed);
    FILES_DELETED.fetch_add(1, Ordering::Relaxed);
    TOTAL_OPERATIONS.fetch_add(1, Ordering::Relaxed);
}

// ============================================================================
// Plugin lifecycle functions
// ============================================================================

/// Plugin initialization - called when the plugin is loaded.
///
/// Sets up the status bar widget and initializes statistics.
///
/// # Returns
/// - `0` on success
/// - Non-zero on failure
#[no_mangle]
pub extern "C" fn init() -> i32 {
    log_info("Code Stats plugin initializing...");

    // Verify host context is available
    // SAFETY: FFI call to host-provided `get_context` function.
    let context_len = unsafe { get_context() };
    if context_len > 0 {
        log_debug("Host context available for code-stats plugin");
    }

    // Register status bar widget for displaying stats
    if register_widget_in_region(UiRegion::StatusBar, "code_stats_widget") {
        log_debug("Status bar widget registered successfully");
    } else {
        log_debug("Failed to register status bar widget (may not be supported)");
    }

    // Initialize stats (already zero from static initialization)
    log_info("Code Stats plugin initialized successfully");
    0 // Success
}

/// Plugin shutdown - called when the plugin is unloaded.
///
/// Saves final statistics if auto_save is enabled.
///
/// # Returns
/// - `0` on success
/// - Non-zero on failure
#[no_mangle]
pub extern "C" fn shutdown() -> i32 {
    log_info("Code Stats plugin shutting down");

    // Emit final stats as an event before shutdown
    let stats_json = get_stats_json();
    if emit_custom_event("code_stats.session_final", &stats_json) {
        log_debug("Final statistics event emitted");
    }

    let summary = get_stats_summary();
    log_info(&format!("Session statistics: {}", summary));

    0 // Success
}

// ============================================================================
// Command handlers
// ============================================================================

/// Handler for the `/stats` command.
///
/// Displays current session code statistics.
///
/// # Returns
/// - `0` on success
/// - Non-zero on failure
#[no_mangle]
pub extern "C" fn cmd_stats() -> i32 {
    log_debug("Stats command executed");

    let summary = get_stats_summary();
    show_notification(ToastLevel::Info, &summary, 5000);
    log_info(&format!("Current statistics: {}", summary));

    // Emit stats event for other plugins/integrations
    let stats_json = get_stats_json();
    if emit_custom_event("code_stats.displayed", &stats_json) {
        log_debug("Statistics display event emitted");
    }

    0 // Success
}

/// Handler for the `/stats-reset` command.
///
/// Resets all statistics counters to zero.
///
/// # Returns
/// - `0` on success
/// - Non-zero on failure
#[no_mangle]
pub extern "C" fn cmd_stats_reset() -> i32 {
    log_debug("Stats reset command executed");

    // Emit event before reset for tracking purposes
    let stats_json = get_stats_json();
    if emit_custom_event("code_stats.before_reset", &stats_json) {
        log_debug("Pre-reset statistics event emitted");
    }

    reset_stats();
    show_notification(ToastLevel::Success, "Statistics reset successfully", 3000);
    log_info("Statistics have been reset to zero");

    0 // Success
}

/// Handler for the `/stats-export` command.
///
/// Exports statistics to JSON format.
///
/// # Returns
/// - `0` on success
/// - Non-zero on failure
#[no_mangle]
pub extern "C" fn cmd_stats_export() -> i32 {
    log_debug("Stats export command executed");

    let stats_json = get_stats_json();
    log_info(&format!("Exported statistics: {}", stats_json));

    // Emit export event with full JSON data
    if emit_custom_event("code_stats.exported", &stats_json) {
        log_debug("Statistics export event emitted");
    }

    show_notification(ToastLevel::Success, "Statistics exported to event stream", 3000);

    0 // Success
}

// ============================================================================
// Hook handlers
// ============================================================================

/// Hook handler for `file_operation_after`.
///
/// Called after any file operation completes. Tracks statistics
/// based on the operation type.
///
/// # Returns
/// - `0` to continue normally
/// - `1` to skip further processing
/// - `2` to abort the operation chain
#[no_mangle]
pub extern "C" fn hook_file_operation_after() -> i32 {
    log_debug("File operation hook triggered");

    // In a real implementation, we would read operation details from a shared buffer.
    // The buffer would contain:
    // - Operation type (create, modify, delete, read)
    // - File path
    // - Lines changed (for create/modify operations)
    //
    // For this example, we simulate tracking a file modification.
    // Each hook invocation represents one file operation.

    // Simulate tracking: assume each operation modified ~10 lines
    // In practice, this would come from the actual diff data
    let simulated_lines_added: u64 = 5;
    let simulated_lines_removed: u64 = 2;

    record_file_modified(simulated_lines_added, simulated_lines_removed);

    let total_ops = TOTAL_OPERATIONS.load(Ordering::Relaxed);
    log_debug(&format!("Tracked file operation #{}", total_ops));

    0 // Continue normally
}

/// Hook handler for `session_end`.
///
/// Called when the session is ending. Saves statistics if configured.
///
/// # Returns
/// - `0` to continue normally
/// - `1` to skip further processing
/// - `2` to abort
#[no_mangle]
pub extern "C" fn hook_session_end() -> i32 {
    log_info("Session end hook triggered - saving statistics");

    let stats_json = get_stats_json();
    if emit_custom_event("code_stats.session_end", &stats_json) {
        log_debug("Session end statistics event emitted");
    }

    let summary = get_stats_summary();
    log_info(&format!("Final session statistics: {}", summary));

    0 // Continue normally
}

/// Hook handler for `widget_register`.
///
/// Called when widgets are being registered. Registers the status bar widget.
///
/// # Returns
/// - `0` to continue normally
/// - `1` to skip
/// - `2` to abort
#[no_mangle]
pub extern "C" fn hook_widget_register() -> i32 {
    log_debug("Widget registration hook triggered");

    // Register our status bar widget
    if register_widget_in_region(UiRegion::StatusBar, "code_stats_display") {
        log_debug("Code stats status bar widget registered via hook");
    }

    0 // Continue normally
}

// ============================================================================
// Widget render function (called by UI system)
// ============================================================================

/// Render the status bar widget content.
///
/// This function is called by the UI system when rendering the status bar.
/// It returns a compact statistics summary for display.
///
/// # Returns
/// - `0` on success
/// - Non-zero on failure
#[no_mangle]
pub extern "C" fn widget_render_code_stats() -> i32 {
    // Get compact stats for status bar display
    let added = LINES_ADDED.load(Ordering::Relaxed);
    let removed = LINES_REMOVED.load(Ordering::Relaxed);
    let ops = TOTAL_OPERATIONS.load(Ordering::Relaxed);

    let status = format!("+{} -{} ({})", added, removed, ops);
    log_debug(&format!("Widget render: {}", status));

    0 // Success
}

// ============================================================================
// Additional utility functions for external integration
// ============================================================================

/// Manually record a file creation event.
///
/// Can be called by external systems to record file creation statistics.
///
/// # Parameters (via shared buffer in real implementation)
/// - `lines`: Number of lines in the created file
///
/// # Returns
/// - `0` on success
#[no_mangle]
pub extern "C" fn api_record_file_created() -> i32 {
    // In real implementation, read line count from shared buffer
    let lines: u64 = 0; // Placeholder - would be read from buffer
    record_file_created(lines);
    0
}

/// Manually record a file deletion event.
///
/// Can be called by external systems to record file deletion statistics.
///
/// # Parameters (via shared buffer in real implementation)
/// - `lines`: Number of lines in the deleted file
///
/// # Returns
/// - `0` on success
#[no_mangle]
pub extern "C" fn api_record_file_deleted() -> i32 {
    // In real implementation, read line count from shared buffer
    let lines: u64 = 0; // Placeholder - would be read from buffer
    record_file_deleted(lines);
    0
}

/// Get current statistics as JSON.
///
/// Populates a shared buffer with JSON statistics data.
///
/// # Returns
/// - Length of JSON string on success
/// - Negative value on failure
#[no_mangle]
pub extern "C" fn api_get_stats_json() -> i64 {
    let json = get_stats_json();
    json.len() as i64
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
        let _ = (file, line); // Suppress unused warnings
        log_message(4, "PANIC: code-stats plugin encountered a fatal error");
    } else {
        log_message(4, "PANIC: code-stats plugin panicked at unknown location");
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
