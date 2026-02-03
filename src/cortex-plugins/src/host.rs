//! WASM host functions for plugin communication.
//!
//! This module provides the host-side implementation of functions that WASM plugins
//! can call. These functions are exposed to plugins through the wasmtime Linker.
//!
//! # Synchronous Context
//!
//! WASM host functions run in a synchronous context (called from wasmtime's sync API).
//! We avoid using `tokio::runtime::Handle::block_on()` to prevent potential deadlocks
//! when the tokio runtime is already blocked on the WASM call. Instead, we use
//! `std::sync::Mutex` for state that needs synchronous access from host functions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wasmtime::{Caller, Engine, Linker};

use crate::Result;
use crate::api::PluginContext;
use crate::hooks::UiRegion;

/// Error codes returned by host functions.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    Success = 0,
    MemoryOutOfBounds = -1,
    InvalidUtf8 = -2,
    InvalidArgument = -3,
    InternalError = -4,
    NotSupported = -5,
}

impl From<HostError> for i32 {
    fn from(err: HostError) -> Self {
        err as i32
    }
}

/// Log levels matching the SDK's expected values.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

impl LogLevel {
    fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::Trace,
            1 => Self::Debug,
            2 => Self::Info,
            3 => Self::Warn,
            4 => Self::Error,
            _ => Self::Info,
        }
    }
}

/// Toast notification levels.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info = 0,
    Success = 1,
    Warning = 2,
    Error = 3,
}

impl ToastLevel {
    fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::Info,
            1 => Self::Success,
            2 => Self::Warning,
            3 => Self::Error,
            _ => Self::Info,
        }
    }
}

/// State shared between the host and WASM plugins.
///
/// Uses `std::sync::Mutex` instead of `tokio::sync::RwLock` to allow synchronous
/// access from WASM host functions without risking deadlocks. WASM host functions
/// are called synchronously by wasmtime, and using `block_on()` to access async
/// locks could deadlock if the tokio runtime is already blocked on the WASM call.
#[derive(Debug, Clone)]
pub struct PluginHostState {
    pub plugin_id: String,
    pub context: PluginContext,
    /// Registered widgets by UI region. Uses sync Mutex for safe access from WASM host functions.
    pub widgets: Arc<Mutex<HashMap<UiRegion, Vec<String>>>>,
    /// Registered keybindings (key -> action). Uses sync Mutex for safe access from WASM host functions.
    pub keybindings: Arc<Mutex<HashMap<String, String>>>,
    /// Emitted events queue. Uses sync Mutex for safe access from WASM host functions.
    pub events: Arc<Mutex<Vec<PluginEvent>>>,
    /// Toast notifications queue. Uses sync Mutex for safe access from WASM host functions.
    pub toasts: Arc<Mutex<Vec<ToastNotification>>>,
}

impl PluginHostState {
    pub fn new(plugin_id: impl Into<String>, context: PluginContext) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            context,
            widgets: Arc::new(Mutex::new(HashMap::new())),
            keybindings: Arc::new(Mutex::new(HashMap::new())),
            events: Arc::new(Mutex::new(Vec::new())),
            toasts: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

/// A custom event emitted by a plugin.
#[derive(Debug, Clone)]
pub struct PluginEvent {
    pub name: String,
    pub data: String,
    pub plugin_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// A toast notification from a plugin.
#[derive(Debug, Clone)]
pub struct ToastNotification {
    pub level: ToastLevel,
    pub message: String,
    pub duration_ms: u32,
    pub plugin_id: String,
}

/// Trait for types that can provide access to PluginHostState.
pub trait HasHostState {
    fn host_state(&self) -> &PluginHostState;
    fn host_state_mut(&mut self) -> &mut PluginHostState;
}

impl HasHostState for PluginHostState {
    fn host_state(&self) -> &PluginHostState {
        self
    }
    fn host_state_mut(&mut self) -> &mut PluginHostState {
        self
    }
}

fn read_string_from_memory<T>(
    mut caller: Caller<'_, T>,
    ptr: i32,
    len: i32,
) -> (Caller<'_, T>, std::result::Result<String, HostError>) {
    if ptr < 0 || len < 0 {
        return (caller, Err(HostError::MemoryOutOfBounds));
    }
    let ptr_usize = ptr as usize;
    let len_usize = len as usize;
    let end = match ptr_usize.checked_add(len_usize) {
        Some(e) => e,
        None => return (caller, Err(HostError::MemoryOutOfBounds)),
    };

    let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
        Some(m) => m,
        None => return (caller, Err(HostError::InternalError)),
    };

    let data = memory.data(&caller);
    if end > data.len() {
        return (caller, Err(HostError::MemoryOutOfBounds));
    }

    let result = std::str::from_utf8(&data[ptr_usize..end])
        .map(|s| s.to_string())
        .map_err(|_| HostError::InvalidUtf8);

    (caller, result)
}

/// Register all host functions with the wasmtime Linker.
pub fn register_host_functions<T>(linker: &mut Linker<T>) -> Result<()>
where
    T: HasHostState + 'static,
{
    linker
        .func_wrap(
            "cortex",
            "log",
            |caller: Caller<'_, T>, level: i32, msg_ptr: i32, msg_len: i32| {
                log_impl(caller, level, msg_ptr, msg_len)
            },
        )
        .map_err(|e| {
            crate::PluginError::execution_error("host", format!("Failed to register log: {}", e))
        })?;

    linker
        .func_wrap("cortex", "get_context", |caller: Caller<'_, T>| {
            get_context_impl(caller)
        })
        .map_err(|e| {
            crate::PluginError::execution_error(
                "host",
                format!("Failed to register get_context: {}", e),
            )
        })?;

    linker
        .func_wrap(
            "cortex",
            "register_widget",
            |caller: Caller<'_, T>, region: i32, type_ptr: i32, type_len: i32| {
                register_widget_impl(caller, region, type_ptr, type_len)
            },
        )
        .map_err(|e| {
            crate::PluginError::execution_error(
                "host",
                format!("Failed to register register_widget: {}", e),
            )
        })?;

    linker
        .func_wrap(
            "cortex",
            "register_keybinding",
            |caller: Caller<'_, T>,
             key_ptr: i32,
             key_len: i32,
             action_ptr: i32,
             action_len: i32| {
                register_keybinding_impl(caller, key_ptr, key_len, action_ptr, action_len)
            },
        )
        .map_err(|e| {
            crate::PluginError::execution_error(
                "host",
                format!("Failed to register register_keybinding: {}", e),
            )
        })?;

    linker
        .func_wrap(
            "cortex",
            "show_toast",
            |caller: Caller<'_, T>, level: i32, msg_ptr: i32, msg_len: i32, duration_ms: i32| {
                show_toast_impl(caller, level, msg_ptr, msg_len, duration_ms)
            },
        )
        .map_err(|e| {
            crate::PluginError::execution_error(
                "host",
                format!("Failed to register show_toast: {}", e),
            )
        })?;

    linker
        .func_wrap(
            "cortex",
            "emit_event",
            |caller: Caller<'_, T>, name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32| {
                emit_event_impl(caller, name_ptr, name_len, data_ptr, data_len)
            },
        )
        .map_err(|e| {
            crate::PluginError::execution_error(
                "host",
                format!("Failed to register emit_event: {}", e),
            )
        })?;

    Ok(())
}

/// Create a new Linker with all host functions registered.
pub fn create_linker<T>(engine: &Engine) -> Result<Linker<T>>
where
    T: HasHostState + 'static,
{
    let mut linker = Linker::new(engine);
    register_host_functions(&mut linker)?;
    Ok(linker)
}

fn log_impl<T: HasHostState>(caller: Caller<'_, T>, level: i32, msg_ptr: i32, msg_len: i32) {
    let plugin_id = caller.data().host_state().plugin_id.clone();
    let (_, result) = read_string_from_memory(caller, msg_ptr, msg_len);
    match result {
        Ok(message) => {
            let log_level = LogLevel::from_i32(level);
            match log_level {
                LogLevel::Trace => tracing::trace!(plugin = %plugin_id, "{}", message),
                LogLevel::Debug => tracing::debug!(plugin = %plugin_id, "{}", message),
                LogLevel::Info => tracing::info!(plugin = %plugin_id, "{}", message),
                LogLevel::Warn => tracing::warn!(plugin = %plugin_id, "{}", message),
                LogLevel::Error => tracing::error!(plugin = %plugin_id, "{}", message),
            }
        }
        Err(e) => {
            tracing::warn!(plugin = %plugin_id, error = ?e, "Failed to read log message from WASM memory");
        }
    }
}

fn get_context_impl<T: HasHostState>(caller: Caller<'_, T>) -> i64 {
    let host_state = caller.data().host_state();
    match serde_json::to_string(&host_state.context) {
        Ok(json) => json.len() as i64,
        Err(e) => {
            tracing::warn!(plugin = %host_state.plugin_id, error = %e, "Failed to serialize context");
            HostError::InternalError as i64
        }
    }
}

fn register_widget_impl<T: HasHostState>(
    caller: Caller<'_, T>,
    region: i32,
    type_ptr: i32,
    type_len: i32,
) -> i32 {
    let plugin_id = caller.data().host_state().plugin_id.clone();
    let widgets = caller.data().host_state().widgets.clone();

    let (_, result) = read_string_from_memory(caller, type_ptr, type_len);
    let widget_type = match result {
        Ok(s) => s,
        Err(e) => return e.into(),
    };

    let ui_region = match region {
        0 => UiRegion::Header,
        1 => UiRegion::Footer,
        2 => UiRegion::SidebarLeft,
        3 => UiRegion::SidebarRight,
        4 => UiRegion::MainContent,
        5 => UiRegion::InputArea,
        6 => UiRegion::Overlay,
        7 => UiRegion::StatusBar,
        8 => UiRegion::ToolOutput,
        9 => UiRegion::MessageArea,
        _ => {
            tracing::warn!(plugin = %plugin_id, region = region, "Invalid UI region");
            return HostError::InvalidArgument.into();
        }
    };

    // Use sync Mutex instead of async RwLock to avoid deadlock risk.
    // WASM host functions run synchronously, and using block_on() on an async lock
    // could deadlock if the tokio runtime is already blocked on this WASM call.
    match widgets.lock() {
        Ok(mut w) => {
            w.entry(ui_region).or_default().push(widget_type.clone());
        }
        Err(e) => {
            tracing::error!(plugin = %plugin_id, error = %e, "Failed to acquire widget lock (poisoned)");
            return HostError::InternalError.into();
        }
    }
    tracing::debug!(plugin = %plugin_id, widget_type = %widget_type, region = ?ui_region, "Widget registered");
    HostError::Success.into()
}

fn register_keybinding_impl<T: HasHostState>(
    caller: Caller<'_, T>,
    key_ptr: i32,
    key_len: i32,
    action_ptr: i32,
    action_len: i32,
) -> i32 {
    let plugin_id = caller.data().host_state().plugin_id.clone();
    let keybindings = caller.data().host_state().keybindings.clone();

    let (caller, key_result) = read_string_from_memory(caller, key_ptr, key_len);
    let key = match key_result {
        Ok(s) => s,
        Err(e) => return e.into(),
    };

    let (_, action_result) = read_string_from_memory(caller, action_ptr, action_len);
    let action = match action_result {
        Ok(s) => s,
        Err(e) => return e.into(),
    };

    if key.is_empty() || action.is_empty() {
        return HostError::InvalidArgument.into();
    }

    // Use sync Mutex instead of async RwLock to avoid deadlock risk.
    // WASM host functions run synchronously, and using block_on() on an async lock
    // could deadlock if the tokio runtime is already blocked on this WASM call.
    match keybindings.lock() {
        Ok(mut kb) => {
            kb.insert(key.clone(), action.clone());
        }
        Err(e) => {
            tracing::error!(plugin = %plugin_id, error = %e, "Failed to acquire keybinding lock (poisoned)");
            return HostError::InternalError.into();
        }
    }
    tracing::debug!(plugin = %plugin_id, key = %key, action = %action, "Keybinding registered");
    HostError::Success.into()
}

fn show_toast_impl<T: HasHostState>(
    caller: Caller<'_, T>,
    level: i32,
    msg_ptr: i32,
    msg_len: i32,
    duration_ms: i32,
) -> i32 {
    let plugin_id = caller.data().host_state().plugin_id.clone();
    let toasts = caller.data().host_state().toasts.clone();

    let (_, result) = read_string_from_memory(caller, msg_ptr, msg_len);
    let message = match result {
        Ok(s) => s,
        Err(e) => return e.into(),
    };

    if duration_ms < 0 {
        return HostError::InvalidArgument.into();
    }

    let toast = ToastNotification {
        level: ToastLevel::from_i32(level),
        message: message.clone(),
        duration_ms: duration_ms as u32,
        plugin_id: plugin_id.clone(),
    };

    // Use sync Mutex instead of async RwLock to avoid deadlock risk.
    // WASM host functions run synchronously, and using block_on() on an async lock
    // could deadlock if the tokio runtime is already blocked on this WASM call.
    match toasts.lock() {
        Ok(mut t) => {
            t.push(toast);
        }
        Err(e) => {
            tracing::error!(plugin = %plugin_id, error = %e, "Failed to acquire toast lock (poisoned)");
            return HostError::InternalError.into();
        }
    }
    tracing::debug!(plugin = %plugin_id, message = %message, "Toast queued");
    HostError::Success.into()
}

fn emit_event_impl<T: HasHostState>(
    caller: Caller<'_, T>,
    name_ptr: i32,
    name_len: i32,
    data_ptr: i32,
    data_len: i32,
) -> i32 {
    let plugin_id = caller.data().host_state().plugin_id.clone();
    let events = caller.data().host_state().events.clone();

    let (caller, name_result) = read_string_from_memory(caller, name_ptr, name_len);
    let name = match name_result {
        Ok(s) => s,
        Err(e) => return e.into(),
    };

    let (_, data_result) = read_string_from_memory(caller, data_ptr, data_len);
    let data = match data_result {
        Ok(s) => s,
        Err(e) => return e.into(),
    };

    if name.is_empty() {
        return HostError::InvalidArgument.into();
    }

    // Validate that data is valid JSON if non-empty.
    // Empty data is allowed and represents "no data" (null/empty event payload).
    // This avoids confusing behavior where `serde_json::from_str("")` would fail,
    // which we explicitly want to allow as a valid "no data" case.
    if !data.is_empty() && serde_json::from_str::<serde_json::Value>(&data).is_err() {
        return HostError::InvalidArgument.into();
    }

    let event = PluginEvent {
        name: name.clone(),
        data,
        plugin_id: plugin_id.clone(),
        timestamp: chrono::Utc::now(),
    };

    // Use sync Mutex instead of async RwLock to avoid deadlock risk.
    // WASM host functions run synchronously, and using block_on() on an async lock
    // could deadlock if the tokio runtime is already blocked on this WASM call.
    match events.lock() {
        Ok(mut e) => {
            e.push(event);
        }
        Err(e) => {
            tracing::error!(plugin = %plugin_id, error = %e, "Failed to acquire event lock (poisoned)");
            return HostError::InternalError.into();
        }
    }
    tracing::debug!(plugin = %plugin_id, event_name = %name, "Event emitted");
    HostError::Success.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_from_i32() {
        assert_eq!(LogLevel::from_i32(0), LogLevel::Trace);
        assert_eq!(LogLevel::from_i32(1), LogLevel::Debug);
        assert_eq!(LogLevel::from_i32(2), LogLevel::Info);
        assert_eq!(LogLevel::from_i32(3), LogLevel::Warn);
        assert_eq!(LogLevel::from_i32(4), LogLevel::Error);
        assert_eq!(LogLevel::from_i32(-1), LogLevel::Info);
    }

    #[test]
    fn test_toast_level_from_i32() {
        assert_eq!(ToastLevel::from_i32(0), ToastLevel::Info);
        assert_eq!(ToastLevel::from_i32(1), ToastLevel::Success);
        assert_eq!(ToastLevel::from_i32(2), ToastLevel::Warning);
        assert_eq!(ToastLevel::from_i32(3), ToastLevel::Error);
    }

    #[test]
    fn test_host_error_conversion() {
        assert_eq!(i32::from(HostError::Success), 0);
        assert_eq!(i32::from(HostError::MemoryOutOfBounds), -1);
    }

    #[test]
    fn test_plugin_host_state_creation() {
        let context = PluginContext::new("/tmp");
        let state = PluginHostState::new("test-plugin", context);
        assert_eq!(state.plugin_id, "test-plugin");
    }

    #[tokio::test]
    async fn test_linker_creation() {
        let mut config = wasmtime::Config::new();
        config.async_support(false);
        let engine = Engine::new(&config).expect("Failed to create engine");
        let result = create_linker::<PluginHostState>(&engine);
        assert!(result.is_ok());
    }

    #[test]
    fn test_plugin_host_state_widgets() {
        let context = PluginContext::new("/tmp");
        let state = PluginHostState::new("test-plugin", context);
        {
            let mut widgets = state.widgets.lock().expect("lock should not be poisoned");
            widgets
                .entry(UiRegion::StatusBar)
                .or_default()
                .push("test_widget".to_string());
        }
        {
            let widgets = state.widgets.lock().expect("lock should not be poisoned");
            assert!(widgets.get(&UiRegion::StatusBar).is_some());
            assert_eq!(widgets.get(&UiRegion::StatusBar).unwrap()[0], "test_widget");
        }
    }
}
