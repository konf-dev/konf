//! Monitoring and admin API endpoints.

use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};

use tracing::info;

use crate::api::chat::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;

/// GET /v1/monitor/runs — list active workflow runs.
pub async fn list_runs(AuthUser(claims): AuthUser, State(state): State<AppState>) -> Json<Value> {
    info!(user_id = %claims.sub, "Listing workflow runs");
    let runs = state.runtime.list_runs(None);
    Json(json!({ "runs": runs }))
}

/// GET /v1/monitor/runs/:id — get run detail.
pub async fn get_run(
    AuthUser(_claims): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let run_id: uuid::Uuid = id
        .parse()
        .map_err(|_| AppError::BadRequest("Invalid run ID".into()))?;

    state
        .runtime
        .get_run(run_id)
        .map(|detail| Json(json!(detail)))
        .ok_or_else(|| AppError::NotFound(format!("Run {id} not found")))
}

/// GET /v1/monitor/runs/:id/tree — get process tree.
pub async fn get_tree(
    AuthUser(_claims): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let run_id: uuid::Uuid = id
        .parse()
        .map_err(|_| AppError::BadRequest("Invalid run ID".into()))?;

    state
        .runtime
        .get_tree(run_id)
        .map(|tree| Json(json!(tree)))
        .ok_or_else(|| AppError::NotFound(format!("Run {id} not found")))
}

/// GET /v1/monitor/metrics — runtime metrics.
pub async fn metrics(AuthUser(_claims): AuthUser, State(state): State<AppState>) -> Json<Value> {
    let m = state.runtime.metrics();
    Json(json!(m))
}

/// DELETE /v1/monitor/runs/:id — cancel a run.
pub async fn cancel_run(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let run_id: uuid::Uuid = id
        .parse()
        .map_err(|_| AppError::BadRequest("Invalid run ID".into()))?;

    info!(user_id = %claims.sub, run_id = %id, "Run cancellation requested");
    state
        .runtime
        .cancel(run_id, "cancelled via API")
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(json!({"cancelled": true, "run_id": id})))
}
