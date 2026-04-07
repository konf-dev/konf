//! Health check endpoint.

use axum::Json;
use serde_json::{json, Value};

/// GET /v1/health
pub async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
