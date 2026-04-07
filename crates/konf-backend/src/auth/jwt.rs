//! JWT verification with JWKS caching.
//!
//! Fetches the JWKS from Supabase's well-known endpoint, caches it,
//! and verifies JWTs against the cached keys.

use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::debug;

use konf_init::AuthConfig;

/// JWT claims extracted from a verified token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// User ID (Supabase `sub` claim).
    pub sub: String,
    /// Audience.
    pub aud: Option<String>,
    /// Issuer.
    pub iss: Option<String>,
    /// Expiration (unix timestamp).
    pub exp: i64,
    /// Role (from Supabase custom claims, if present).
    pub role: Option<String>,
}

/// JWKS-based JWT verifier with caching.
pub struct JwtVerifier {
    jwks_url: String,
    audience: String,
    cache: Arc<RwLock<Option<CachedJwks>>>,
    cache_ttl: Duration,
    http_client: reqwest::Client,
}

struct CachedJwks {
    keys: jsonwebtoken::jwk::JwkSet,
    fetched_at: Instant,
}

/// Errors from JWT verification.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid Authorization header format (expected 'Bearer <token>')")]
    InvalidHeaderFormat,
    #[error("missing 'kid' in JWT header")]
    MissingKid,
    #[error("key not found in JWKS for kid '{0}'")]
    KeyNotFound(String),
    #[error("JWT verification failed: {0}")]
    VerificationFailed(String),
    #[error("failed to fetch JWKS: {0}")]
    JwksFetchFailed(String),
    #[error("token expired")]
    Expired,
}

impl JwtVerifier {
    /// Create a new verifier for a Supabase instance.
    pub fn new(config: &AuthConfig) -> Self {
        let jwks_url = format!("{}/auth/v1/.well-known/jwks.json", config.supabase_url);
        Self {
            jwks_url,
            audience: config.jwt_audience.clone(),
            cache: Arc::new(RwLock::new(None)),
            cache_ttl: Duration::from_secs(600), // 10 minutes
            http_client: reqwest::Client::new(),
        }
    }

    /// Verify a JWT token and return the claims.
    pub async fn verify(&self, token: &str) -> Result<Claims, AuthError> {
        let header = decode_header(token)
            .map_err(|e| AuthError::VerificationFailed(e.to_string()))?;

        let kid = header.kid
            .ok_or(AuthError::MissingKid)?;

        let jwks = self.get_or_refresh_jwks().await?;

        let jwk = jwks.keys.iter()
            .find(|k| k.common.key_id.as_deref() == Some(&kid))
            .ok_or_else(|| AuthError::KeyNotFound(kid.clone()))?;

        let decoding_key = DecodingKey::from_jwk(jwk)
            .map_err(|e| AuthError::VerificationFailed(format!("invalid JWK: {e}")))?;

        let algorithm = match header.alg {
            Algorithm::RS256 => Algorithm::RS256,
            Algorithm::ES256 => Algorithm::ES256,
            alg => return Err(AuthError::VerificationFailed(format!("unsupported algorithm: {alg:?}"))),
        };

        let mut validation = Validation::new(algorithm);
        if !self.audience.is_empty() {
            validation.set_audience(&[&self.audience]);
        }

        let token_data = decode::<Claims>(token, &decoding_key, &validation)
            .map_err(|e| {
                if e.kind() == &jsonwebtoken::errors::ErrorKind::ExpiredSignature {
                    AuthError::Expired
                } else {
                    AuthError::VerificationFailed(e.to_string())
                }
            })?;

        debug!(sub = %token_data.claims.sub, "JWT verified");
        Ok(token_data.claims)
    }

    async fn get_or_refresh_jwks(&self) -> Result<jsonwebtoken::jwk::JwkSet, AuthError> {
        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.as_ref() {
                if cached.fetched_at.elapsed() < self.cache_ttl {
                    return Ok(cached.keys.clone());
                }
            }
        }

        // Fetch fresh JWKS
        debug!(url = %self.jwks_url, "fetching JWKS");
        let response = self.http_client
            .get(&self.jwks_url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| AuthError::JwksFetchFailed(e.to_string()))?;

        let jwks: jsonwebtoken::jwk::JwkSet = response
            .json()
            .await
            .map_err(|e| AuthError::JwksFetchFailed(format!("invalid JWKS JSON: {e}")))?;

        // Cache
        let mut cache = self.cache.write().await;
        *cache = Some(CachedJwks {
            keys: jwks.clone(),
            fetched_at: Instant::now(),
        });

        Ok(jwks)
    }

    /// Extract Bearer token from Authorization header value.
    pub fn extract_token(auth_header: &str) -> Result<&str, AuthError> {
        auth_header
            .strip_prefix("Bearer ")
            .ok_or(AuthError::InvalidHeaderFormat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_bearer_token() {
        assert_eq!(
            JwtVerifier::extract_token("Bearer abc123").unwrap(),
            "abc123"
        );
    }

    #[test]
    fn test_extract_token_invalid_format() {
        assert!(JwtVerifier::extract_token("Basic abc123").is_err());
        assert!(JwtVerifier::extract_token("abc123").is_err());
        assert!(JwtVerifier::extract_token("").is_err());
    }

    #[test]
    fn test_verifier_creation() {
        let config = AuthConfig {
            supabase_url: "http://localhost:9999".into(),
            jwt_audience: "authenticated".into(),
        };
        let verifier = JwtVerifier::new(&config);
        assert_eq!(verifier.jwks_url, "http://localhost:9999/auth/v1/.well-known/jwks.json");
    }
}
