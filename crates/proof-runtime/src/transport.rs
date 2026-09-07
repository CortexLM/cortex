use std::{sync::Arc, time::Duration};

use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::json;

use crate::{RuntimeCall, RuntimeError, RuntimeOperations, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};

/// Bind without replacing any existing path. The private parent must already
/// exist, be canonical and inaccessible to other users. Put it outside kernels.
///
/// # Errors
/// Unsafe parent, existing socket, invalid path or local I/O failure.
#[cfg(unix)]
pub fn bind_private_socket(
    path: &std::path::Path,
) -> Result<tokio::net::UnixListener, RuntimeError> {
    use std::os::unix::fs::PermissionsExt;
    let parent = path.parent().ok_or(RuntimeError::Scope)?;
    let metadata = std::fs::metadata(parent).map_err(|_| RuntimeError::Scope)?;
    if !path.is_absolute()
        || !metadata.is_dir()
        || metadata.permissions().mode() & 0o077 != 0
        || std::fs::canonicalize(parent).map_err(|_| RuntimeError::Scope)? != parent
    {
        return Err(RuntimeError::Scope);
    }
    let listener = tokio::net::UnixListener::bind(path).map_err(|_| RuntimeError::Unavailable)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| RuntimeError::Unavailable)?;
    Ok(listener)
}

/// Controller-private router. Serve only over an owner-only Unix socket outside
/// every agent workspace. Do not merge it into the public Proof router.
pub fn private_router(operations: Arc<dyn RuntimeOperations>) -> Router {
    Router::new()
        .route("/call", post(call))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(operations)
}

impl IntoResponse for RuntimeError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Scope => StatusCode::FORBIDDEN,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status, "Cortex controller operation denied or unavailable").into_response()
    }
}

async fn call(
    State(operations): State<Arc<dyn RuntimeOperations>>,
    input: Result<Json<RuntimeCall>, JsonRejection>,
) -> Result<Json<serde_json::Value>, RuntimeError> {
    let Json(input) = input.map_err(|_| RuntimeError::Scope)?;
    if input.schema_version != 1 || !input.arguments.is_object() {
        return Err(RuntimeError::Scope);
    }
    let result = tokio::time::timeout(Duration::from_secs(30), operations.call(input))
        .await
        .map_err(|_| RuntimeError::Unavailable)??;
    if !result.is_object() {
        return Err(RuntimeError::Unavailable);
    }
    let reply = json!({ "schema_version": 1, "result": result });
    if serde_json::to_vec(&reply)?.len() > MAX_RESPONSE_BYTES {
        return Err(RuntimeError::Unavailable);
    }
    Ok(Json(reply))
}
