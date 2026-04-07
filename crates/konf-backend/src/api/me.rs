//! Authenticated user info endpoint.

use axum::Json;
use serde_json::{json, Value};

use crate::auth::middleware::AuthUser;

/// GET /v1/me — returns the authenticated user's claims.
pub async fn me(AuthUser(claims): AuthUser) -> Json<Value> {
    Json(json!({
        "user_id": claims.sub,
        "role": claims.role,
    }))
}
