//! mDNS service discovery endpoint.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Query, State},
};

use crate::error::AppResult;
use crate::mdns::MdnsDiscovery;
use crate::state::AppState;

use super::types::{DiscoverQuery, DiscoverResponse};

/// Discover Cortex servers on the local network using mDNS/Bonjour.
///
/// This endpoint scans the local network for other Cortex servers
/// that have mDNS publishing enabled.
///
/// Query parameters:
/// - `timeout`: Discovery timeout in seconds (default: 3, max: 30)
///
/// Example response:
/// ```json
/// {
///   "servers": [
///     {
///       "name": "cortex-8080",
///       "fullname": "cortex-8080._cortex._tcp.local.",
///       "host": "mycomputer.local.",
///       "port": 8080,
///       "addresses": ["192.168.1.100"],
///       "properties": {
///         "version": "0.1.0",
///         "api": "v1",
///         "path": "/"
///       },
///       "discovered_at": 1704326400000
///     }
///   ],
///   "count": 1,
///   "duration_ms": 3012
/// }
/// ```
pub async fn discover_servers(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiscoverQuery>,
) -> AppResult<Json<DiscoverResponse>> {
    // Cap the timeout at 30 seconds
    let timeout_secs = query.timeout.min(30);
    let timeout = Duration::from_secs(timeout_secs);

    let start = Instant::now();

    // Create a new discovery client
    let discovery = MdnsDiscovery::new()?;

    // Discover servers
    let servers = discovery.discover(timeout).await?;

    // Cleanup
    if let Err(e) = discovery.shutdown() {
        tracing::warn!("Failed to shutdown mDNS discovery daemon: {}", e);
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    let count = servers.len();

    // Update metrics
    state.increment_counter("mdns_discovery_requests").await;

    Ok(Json(DiscoverResponse {
        servers,
        count,
        duration_ms,
    }))
}
