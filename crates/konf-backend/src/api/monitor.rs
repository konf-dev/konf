//! Monitoring and admin API endpoints.

use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

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

/// Query parameters for [`stream`].
#[derive(Debug, Deserialize)]
pub struct StreamParams {
    /// Optional namespace prefix filter. Only events whose `namespace`
    /// field starts with this string are forwarded. Events without a
    /// namespace (e.g. `IntentReplayed`) are always forwarded.
    #[serde(default)]
    pub namespace: Option<String>,
}

/// GET /v1/monitor/stream — Server-Sent Events pipe of every [`RunEvent`]
/// emitted by the runtime.
///
/// Subscribes to the runtime's broadcast channel and forwards each event
/// as an SSE frame with `event:` set to the event kind (e.g.
/// `run_started`, `node_start`, `tool_invoked`) and the payload as JSON.
///
/// # Slow subscribers
///
/// If a client falls behind, its receiver returns [`RecvError::Lagged`]
/// with the number of missed events. The handler forwards this as a
/// `lagged` SSE frame so the client can refetch state via the REST API
/// and resume the stream. The channel is never blocked by a slow reader.
pub async fn stream(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
    Query(params): Query<StreamParams>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    info!(user_id = %claims.sub, namespace = ?params.namespace, "opening monitor stream");
    let mut rx = state.runtime.event_bus().subscribe();
    let filter = params.namespace;

    let body = async_stream::stream! {
        yield Ok::<Event, Infallible>(
            Event::default()
                .event("hello")
                .data(r#"{"status":"connected"}"#),
        );
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if let Some(ref prefix) = filter {
                        let ns = ev.namespace();
                        if !ns.is_empty() && !ns.starts_with(prefix.as_str()) {
                            continue;
                        }
                    }
                    let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
                    yield Ok(Event::default().event(ev.kind()).data(data));
                }
                Err(RecvError::Lagged(n)) => {
                    yield Ok(Event::default().event("lagged").data(n.to_string()));
                }
                Err(RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(body).keep_alive(KeepAlive::default())
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
