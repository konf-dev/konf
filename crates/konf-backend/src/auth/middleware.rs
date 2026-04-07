//! Axum middleware for JWT authentication.
//!
//! Extracts the Bearer token from the Authorization header,
//! verifies it, and injects `Claims` into request extensions.

use std::sync::Arc;

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
    Json,
};
use serde_json::json;

use super::jwt::{Claims, JwtVerifier};

/// Axum middleware that verifies JWT and injects Claims.
///
/// When `KONF_DEV_MODE=true`, JWT verification is bypassed and a fake dev user
/// is injected. This is for local development and examples ONLY.
pub async fn auth_middleware(
    verifier: Arc<JwtVerifier>,
    mut request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    // Dev mode: bypass JWT, inject fake user
    if std::env::var("KONF_DEV_MODE").is_ok() {
        let dev_claims = Claims {
            sub: "dev_user".into(),
            aud: None,
            iss: None,
            exp: i64::MAX,
            role: Some("user".into()),
        };
        request.extensions_mut().insert(dev_claims);
        return Ok(next.run(request).await);
    }

    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(header) => {
            JwtVerifier::extract_token(header)
                .map_err(|e| (StatusCode::UNAUTHORIZED, Json(json!({"error": e.to_string()}))))?
        }
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Missing Authorization header"})),
            ));
        }
    };

    let claims = verifier
        .verify(token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, Json(json!({"error": e.to_string()}))))?;

    // Inject claims into request extensions for handlers to access
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

/// Extractor for authenticated user claims.
/// Use in handlers: `async fn handler(claims: AuthUser) -> ...`
#[derive(Debug, Clone)]
pub struct AuthUser(pub Claims);

impl<S> axum::extract::FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Claims>()
            .cloned()
            .map(AuthUser)
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "Not authenticated"})),
                )
            })
    }
}
