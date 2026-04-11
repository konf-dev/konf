//! Configuration schema for the SurrealDB memory backend.

use serde::{Deserialize, Serialize};

/// Default namespace when a query omits one.
pub const DEFAULT_NAMESPACE: &str = "default";
/// Default search result limit.
pub const DEFAULT_LIMIT: i64 = 10;
/// Default embedding vector dimension. Mirrors the dimension used by
/// `nomic-embed-text`, the default embedder in konf's `konf-tool-embed` crate.
pub const DEFAULT_VECTOR_DIMENSION: usize = 768;
/// Reciprocal Rank Fusion constant, matching smrti's default.
pub const DEFAULT_RRF_K: i64 = 60;
/// Maximum candidate pool per mode during hybrid search, matching smrti.
pub const DEFAULT_HYBRID_CANDIDATE_POOL: i64 = 100;

/// SurrealDB deployment mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SurrealMode {
    /// Embedded RocksDB file. Single-process, persistent.
    #[default]
    Embedded,
    /// In-memory (no persistence). Useful for tests.
    Memory,
    /// Remote SurrealDB server over WebSocket.
    Remote,
}

/// Configuration for the SurrealDB memory backend.
///
/// Deserialized from the `tools.memory.config` section of a konf product's
/// `tools.yaml`. Shape:
///
/// ```yaml
/// tools:
///   memory:
///     backend: surreal
///     config:
///       mode: embedded              # embedded | memory | remote
///       path: ./memory.db            # for embedded mode
///       endpoint: ws://host:8000     # for remote mode
///       username: root               # optional, remote mode
///       password: ${SURREAL_PASSWORD}
///       namespace: konf              # Surreal namespace
///       database: default            # Surreal database
///       vector_dimension: 768        # embedding size
///       rrf_k: 60                    # hybrid-search fusion constant
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurrealConfig {
    /// Which connection strategy to use.
    #[serde(default)]
    pub mode: SurrealMode,

    /// Filesystem path for `mode: embedded` (required in that mode).
    #[serde(default)]
    pub path: Option<String>,

    /// WebSocket endpoint for `mode: remote`, e.g. `ws://127.0.0.1:8000`.
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Optional root user for remote mode.
    #[serde(default)]
    pub username: Option<String>,

    /// Optional password for remote mode.
    #[serde(default)]
    pub password: Option<String>,

    /// Surreal namespace (one per tenant/product deployment).
    #[serde(default = "default_surreal_ns")]
    pub namespace: String,

    /// Surreal database (usually just "default" inside a namespace).
    #[serde(default = "default_surreal_db")]
    pub database: String,

    /// Embedding vector dimension used for the HNSW index. Must match the
    /// embedder used by the product. Changing this after nodes are stored
    /// requires dropping the index and re-indexing.
    #[serde(default = "default_vector_dimension")]
    pub vector_dimension: usize,

    /// Reciprocal Rank Fusion constant for hybrid search.
    #[serde(default = "default_rrf_k")]
    pub rrf_k: i64,

    /// Candidate pool size per mode during hybrid search.
    #[serde(default = "default_hybrid_candidate_pool")]
    pub hybrid_candidate_pool: i64,

    /// Default result limit if a query doesn't specify one.
    #[serde(default = "default_limit")]
    pub default_limit: i64,
}

fn default_surreal_ns() -> String {
    "konf".to_string()
}
fn default_surreal_db() -> String {
    "default".to_string()
}
fn default_vector_dimension() -> usize {
    DEFAULT_VECTOR_DIMENSION
}
fn default_rrf_k() -> i64 {
    DEFAULT_RRF_K
}
fn default_hybrid_candidate_pool() -> i64 {
    DEFAULT_HYBRID_CANDIDATE_POOL
}
fn default_limit() -> i64 {
    DEFAULT_LIMIT
}

impl SurrealConfig {
    /// Validate that required fields are present for the chosen mode.
    pub fn validate(&self) -> Result<(), String> {
        match self.mode {
            SurrealMode::Embedded => {
                if self.path.is_none() {
                    return Err("SurrealConfig.path is required when mode = embedded".to_string());
                }
            }
            SurrealMode::Memory => {}
            SurrealMode::Remote => {
                if self.endpoint.is_none() {
                    return Err("SurrealConfig.endpoint is required when mode = remote".to_string());
                }
            }
        }
        if self.vector_dimension == 0 {
            return Err("SurrealConfig.vector_dimension must be > 0".to_string());
        }
        if self.rrf_k <= 0 {
            return Err("SurrealConfig.rrf_k must be > 0".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_mode_requires_path() {
        let cfg = SurrealConfig {
            mode: SurrealMode::Embedded,
            path: None,
            endpoint: None,
            username: None,
            password: None,
            namespace: "konf".into(),
            database: "default".into(),
            vector_dimension: 768,
            rrf_k: 60,
            hybrid_candidate_pool: 100,
            default_limit: 10,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn memory_mode_needs_no_path() {
        let cfg = SurrealConfig {
            mode: SurrealMode::Memory,
            path: None,
            endpoint: None,
            username: None,
            password: None,
            namespace: "konf".into(),
            database: "default".into(),
            vector_dimension: 768,
            rrf_k: 60,
            hybrid_candidate_pool: 100,
            default_limit: 10,
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn remote_mode_requires_endpoint() {
        let cfg = SurrealConfig {
            mode: SurrealMode::Remote,
            path: None,
            endpoint: None,
            username: None,
            password: None,
            namespace: "konf".into(),
            database: "default".into(),
            vector_dimension: 768,
            rrf_k: 60,
            hybrid_candidate_pool: 100,
            default_limit: 10,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn from_json_defaults() {
        let j = serde_json::json!({
            "mode": "memory"
        });
        let cfg: SurrealConfig = serde_json::from_value(j).unwrap();
        assert_eq!(cfg.mode, SurrealMode::Memory);
        assert_eq!(cfg.namespace, "konf");
        assert_eq!(cfg.database, "default");
        assert_eq!(cfg.vector_dimension, 768);
        assert_eq!(cfg.rrf_k, 60);
    }
}
