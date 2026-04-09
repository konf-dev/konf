//! Admin API endpoints — config management and audit log.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use tracing::info;

use crate::api::chat::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;

/// GET /v1/admin/config — read the current product config.
pub async fn get_config(AuthUser(claims): AuthUser, State(state): State<AppState>) -> Json<Value> {
    info!(user_id = %claims.sub, "Admin: reading product config");
    let product_config = state.runtime.engine().config().clone();
    Json(json!({
        "engine": {
            "max_steps": product_config.max_steps,
            "default_timeout_ms": product_config.default_timeout_ms,
            "max_workflow_timeout_ms": product_config.max_workflow_timeout_ms,
            "stream_buffer": product_config.stream_buffer,
            "max_concurrent_nodes": product_config.max_concurrent_nodes,
        },
        "tools": state.runtime.engine().registry().list().iter()
            .map(|t| json!({
                "name": t.name,
                "description": t.description,
                "annotations": t.annotations,
            }))
            .collect::<Vec<_>>(),
        "resources": state.runtime.engine().resources().list().iter()
            .map(|r| json!({
                "uri": r.uri,
                "name": r.name,
                "mime_type": r.mime_type,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// GET /v1/admin/audit — query the event journal.
pub async fn get_audit(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    info!(user_id = %claims.sub, "Admin: querying audit log");

    match state.runtime.journal() {
        Some(journal) => {
            let events = journal
                .recent(100)
                .await
                .map_err(|e| AppError::Internal(format!("Journal query failed: {e}")))?;
            Ok(Json(json!({ "events": events })))
        }
        None => Ok(Json(json!({
            "events": [],
            "note": "Event journal is not available (no database configured)"
        }))),
    }
}
