//! Miner API only. Account enrollment, provider observations, leases, resource
//! adoption and evidence admission are trusted controller operations, not routes.
//! V2 cannot call the legacy operator-key Lium scorer.

#![forbid(unsafe_code)]

use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use proof_autonomy::{ContractError, SignedAction, SignedConsent};
use proof_autonomy_pg::{
    CancelExperiment, CreateExperiment, Experiment, ExperimentView, PgStore, StoreError,
    ViewExperiment,
};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authorized<T> {
    pub request: T,
    pub authorization: SignedAction,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentRequest {
    pub revision: i64,
    pub consent: SignedConsent,
}

/// Mount only after checking database availability and application privileges.
///
/// # Errors
/// Missing migrations or unsafe/unavailable database connection.
pub async fn router(store: PgStore) -> Result<Router, StoreError> {
    store.ready().await?;
    Ok(Router::new()
        .route("/v2/experiments", post(create))
        .route("/v2/experiments/{id}/view", post(view))
        .route("/v2/experiments/{id}/consent", post(consent))
        .route("/v2/experiments/{id}/cancel", post(cancel))
        .layer(DefaultBodyLimit::max(32 * 1024))
        .with_state(store))
}

struct ApiError(StoreError);

impl From<StoreError> for ApiError {
    fn from(value: StoreError) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self.0 {
            StoreError::Scope
            | StoreError::Contract(
                ContractError::Signature | ContractError::Scope | ContractError::Expired,
            ) => (StatusCode::FORBIDDEN, "authorization denied"),
            StoreError::Conflict
            | StoreError::Replay
            | StoreError::Fenced
            | StoreError::Contract(ContractError::Transition) => {
                (StatusCode::CONFLICT, "stale or already consumed command")
            }
            StoreError::Contract(_) => (StatusCode::BAD_REQUEST, "invalid command"),
            StoreError::Quota => (
                StatusCode::TOO_MANY_REQUESTS,
                "experiment intake quota exhausted",
            ),
            StoreError::Database | StoreError::Corrupt => (
                StatusCode::SERVICE_UNAVAILABLE,
                "durable service unavailable",
            ),
        };
        (status, message).into_response()
    }
}

fn body<T>(value: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    // Do not echo deserializer diagnostics, submitted bodies or signatures.
    value
        .map(|Json(value)| value)
        .map_err(|_| ApiError(StoreError::Contract(ContractError::Encoding)))
}

async fn create(
    State(store): State<PgStore>,
    input: Result<Json<Authorized<CreateExperiment>>, JsonRejection>,
) -> Result<(StatusCode, Json<Experiment>), ApiError> {
    let input = body(input)?;
    let experiment = store
        .create_experiment(&input.request, &input.authorization)
        .await?;
    Ok((StatusCode::CREATED, Json(experiment)))
}

async fn view(
    State(store): State<PgStore>,
    Path(id): Path<Uuid>,
    input: Result<Json<Authorized<ViewExperiment>>, JsonRejection>,
) -> Result<Json<ExperimentView>, ApiError> {
    let input = body(input)?;
    if input.request.experiment_id != id {
        return Err(StoreError::Scope.into());
    }
    Ok(Json(
        store.view(&input.request, &input.authorization).await?,
    ))
}

async fn consent(
    State(store): State<PgStore>,
    Path(id): Path<Uuid>,
    input: Result<Json<ConsentRequest>, JsonRejection>,
) -> Result<Json<Experiment>, ApiError> {
    let input = body(input)?;
    Ok(Json(
        store.consent(id, input.revision, &input.consent).await?,
    ))
}

async fn cancel(
    State(store): State<PgStore>,
    Path(id): Path<Uuid>,
    input: Result<Json<Authorized<CancelExperiment>>, JsonRejection>,
) -> Result<Json<Experiment>, ApiError> {
    let input = body(input)?;
    if input.request.experiment_id != id {
        return Err(StoreError::Scope.into());
    }
    Ok(Json(
        store.cancel(&input.request, &input.authorization).await?,
    ))
}
