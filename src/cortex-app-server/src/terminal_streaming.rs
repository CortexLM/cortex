//! Terminal output streaming to WebSocket clients.
//!
//! NOTE: Terminal tool functionality has been removed. This module is kept
//! as a stub to maintain API compatibility but does not perform any operations.

use tokio::sync::broadcast;
use tracing::info;

use crate::websocket::WsMessage;

/// Start terminal output streaming.
/// Returns a task handle that should be awaited or aborted on shutdown.
///
/// NOTE: Terminal functionality has been removed. This is now a no-op stub.
pub fn start_terminal_streaming(
    _broadcast_tx: broadcast::Sender<WsMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("Terminal streaming disabled - terminal tools have been removed");
    })
}

/// Broadcast a terminal created event.
///
/// NOTE: Terminal functionality has been removed. This is now a no-op stub.
pub fn broadcast_terminal_created(
    _broadcast_tx: &broadcast::Sender<WsMessage>,
    _terminal_id: String,
    _name: String,
    _cwd: String,
) {
    // No-op: Terminal functionality removed
}

/// Broadcast a terminal status change event.
///
/// NOTE: Terminal functionality has been removed. This is now a no-op stub.
pub fn broadcast_terminal_status(
    _broadcast_tx: &broadcast::Sender<WsMessage>,
    _terminal_id: String,
    _status: String,
    _exit_code: Option<i32>,
) {
    // No-op: Terminal functionality removed
}

/// Broadcast terminal list.
///
/// NOTE: Terminal functionality has been removed. This is now a no-op stub.
pub fn broadcast_terminal_list(
    _broadcast_tx: &broadcast::Sender<WsMessage>,
    _terminals: Vec<serde_json::Value>,
) {
    // No-op: Terminal functionality removed
}
