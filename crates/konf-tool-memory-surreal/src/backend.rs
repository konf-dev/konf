//! `MemoryBackend` impl over SurrealDB.
//!
//! This file is the glue between the trait and the underlying `Surreal<Any>`
//! handle. The heavy lifting for searches is in [`crate::search`] and for
//! session KV is in [`crate::session`]; this module wires them together,
//! handles input validation, and appends the audit event rows.

use async_trait::async_trait;
use konf_tool_memory::{MemoryBackend, MemoryError, SearchParams};
use serde_json::{json, Value};
use surrealdb::engine::any::Any;
use surrealdb::Surreal;

use crate::config::{SurrealConfig, DEFAULT_NAMESPACE};
use crate::error::map_db_error;
use crate::{search, session};

/// SurrealDB-backed implementation of [`MemoryBackend`].
pub struct SurrealBackend {
    db: Surreal<Any>,
    cfg: SurrealConfig,
}

impl SurrealBackend {
    /// Build a backend from an already-connected SurrealDB handle and its config.
    ///
    /// Prefer [`crate::connect::connect`] at runtime; this constructor is
    /// exposed for tests and advanced embedding scenarios where the caller
    /// has already set up the connection.
    pub fn new(db: Surreal<Any>, cfg: SurrealConfig) -> Self {
        Self { db, cfg }
    }

    /// Access the underlying SurrealDB handle. Used by `search` and `session`
    /// modules inside the crate; not part of the public API.
    pub(crate) fn db(&self) -> &Surreal<Any> {
        &self.db
    }

    /// Access the backend config. Used by sibling modules.
    pub(crate) fn cfg(&self) -> &SurrealConfig {
        &self.cfg
    }

    /// Resolve the namespace parameter against the config default.
    pub(crate) fn resolve_namespace(&self, param: Option<&str>) -> String {
        param
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_NAMESPACE.to_string())
    }

    /// Append an event row in the audit log. Best-effort — failures are
    /// returned to the caller so a malformed mutation surfaces loudly rather
    /// than silently skipping the audit.
    async fn record_event(
        &self,
        namespace: &str,
        event_type: &str,
        payload: Value,
    ) -> Result<(), MemoryError> {
        let sql = "CREATE event SET namespace = $ns, event_type = $et, payload = $p";
        self.db
            .query(sql)
            .bind(("ns", namespace.to_string()))
            .bind(("et", event_type.to_string()))
            .bind(("p", payload))
            .await
            .map_err(map_db_error)?
            .check()
            .map_err(map_db_error)?;
        Ok(())
    }
}

#[async_trait]
impl MemoryBackend for SurrealBackend {
    async fn search(&self, params: SearchParams) -> Result<Value, MemoryError> {
        search::run(self, params).await
    }

    async fn add_nodes(
        &self,
        nodes: &[Value],
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError> {
        if nodes.is_empty() {
            return Err(MemoryError::Validation(
                "add_nodes: nodes array is empty".to_string(),
            ));
        }

        let ns = self.resolve_namespace(namespace);
        let mut created_count: usize = 0;

        // For each node we issue one SurrealQL block that creates the node and,
        // if an embedding is present, also creates the node_embedding row
        // using `$created.id` from the LET binding. This keeps the record id
        // entirely inside SurrealDB so we never parse it back in Rust.
        for node in nodes {
            let content = node.get("content").and_then(Value::as_str).ok_or_else(|| {
                MemoryError::Validation(
                    "add_nodes: every node must have a `content` string".to_string(),
                )
            })?;
            let node_type = node
                .get("node_type")
                .and_then(Value::as_str)
                .unwrap_or("memory")
                .to_string();
            let metadata = node.get("metadata").cloned().unwrap_or_else(|| json!({}));

            let embedding_vec: Option<Vec<f64>> = node
                .get("embedding")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(Value::as_f64).collect());

            if let Some(ref v) = embedding_vec {
                if !v.is_empty() && v.len() != self.cfg.vector_dimension {
                    return Err(MemoryError::Validation(format!(
                        "add_nodes: embedding has {} dims, expected {}",
                        v.len(),
                        self.cfg.vector_dimension
                    )));
                }
            }

            let model_name = node
                .get("model_name")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_string();

            // Build the statement block. The LET + second CREATE run in one
            // round-trip so the node's record id (a `Thing`) never leaves
            // SurrealDB's namespace.
            let has_embedding = embedding_vec
                .as_ref()
                .map(|v| !v.is_empty())
                .unwrap_or(false);
            let sql = if has_embedding {
                r#"
LET $created = (CREATE node SET
    namespace = $ns,
    node_type = $nt,
    content   = $c,
    metadata  = $m);
CREATE node_embedding SET
    node       = $created[0].id,
    namespace  = $ns,
    model_name = $mn,
    embedding  = $e;
"#
            } else {
                "CREATE node SET namespace = $ns, node_type = $nt, content = $c, metadata = $m;"
            };

            let mut q = self
                .db
                .query(sql)
                .bind(("ns", ns.clone()))
                .bind(("nt", node_type))
                .bind(("c", content.to_string()))
                .bind(("m", metadata));
            if has_embedding {
                q = q
                    .bind(("mn", model_name))
                    .bind(("e", embedding_vec.unwrap_or_default()));
            }
            q.await
                .map_err(map_db_error)?
                .check()
                .map_err(map_db_error)?;

            created_count += 1;
        }

        self.record_event(&ns, "nodes_added", json!({ "count": created_count }))
            .await?;

        Ok(json!({
            "added": created_count,
            "_meta": { "namespace": ns }
        }))
    }

    async fn state_set(
        &self,
        key: &str,
        value: &Value,
        session_id: &str,
        namespace: Option<&str>,
        ttl: Option<i64>,
    ) -> Result<Value, MemoryError> {
        session::set(self, key, value, session_id, namespace, ttl).await
    }

    async fn state_get(
        &self,
        key: &str,
        session_id: &str,
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError> {
        session::get(self, key, session_id, namespace).await
    }

    async fn state_delete(
        &self,
        key: &str,
        session_id: &str,
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError> {
        session::delete(self, key, session_id, namespace).await
    }

    async fn state_list(
        &self,
        session_id: &str,
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError> {
        session::list(self, session_id, namespace).await
    }

    async fn state_clear(
        &self,
        session_id: &str,
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError> {
        session::clear(self, session_id, namespace).await
    }

    fn supported_search_modes(&self) -> Vec<String> {
        vec!["text".into(), "vector".into(), "hybrid".into()]
    }
}
