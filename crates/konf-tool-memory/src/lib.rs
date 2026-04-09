//! Memory tools and the [`MemoryBackend`] trait for the Konf platform.
//!
//! This crate provides:
//! - The `MemoryBackend` trait — the VFS abstraction for pluggable storage
//! - Tool shells (memory_search, memory_store, state_*) that delegate to a backend
//! - Registration function to wire tools into the engine
//!
//! Backend implementations (smrti, SurrealDB, SQLite) live in separate crates.
#![warn(missing_docs)]

mod tools;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use konflux::Engine;

pub use tools::*;

/// Errors from memory backend operations.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// Query or storage operation failed
    #[error("memory operation failed: {0}")]
    OperationFailed(String),

    /// Input validation failed
    #[error("validation error: {0}")]
    Validation(String),

    /// Backend not connected or unavailable
    #[error("backend unavailable: {0}")]
    Unavailable(String),

    /// Feature not supported by this backend
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}

/// Parameters for a memory search query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchParams {
    /// Text query (for text and hybrid search)
    pub query: Option<String>,
    /// Search mode: "hybrid", "vector", "text"
    pub mode: Option<String>,
    /// Maximum results to return
    pub limit: Option<i64>,
    /// Namespace filter (injected by VirtualizedTool)
    pub namespace: Option<String>,
    /// Filter by node type
    pub node_type: Option<String>,
    /// Filter by edge type
    pub edge_type: Option<String>,
    /// JSONB metadata filter
    pub metadata_filter: Option<Value>,
    /// Minimum similarity threshold
    pub min_similarity: Option<f64>,
}

/// The VFS abstraction for pluggable memory storage.
///
/// Every memory backend (Postgres/smrti, SurrealDB, SQLite) implements this trait.
/// Tools in this crate hold `Arc<dyn MemoryBackend>` and delegate to it.
/// Namespace injection happens at the tool level via VirtualizedTool — the
/// backend receives namespace as a parameter and enforces isolation internally.
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Search for nodes matching a query.
    async fn search(&self, params: SearchParams) -> Result<Value, MemoryError>;

    /// Add nodes to the knowledge graph.
    async fn add_nodes(
        &self,
        nodes: &[Value],
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError>;

    /// Set a session state key (working memory scratchpad).
    async fn state_set(
        &self,
        key: &str,
        value: &Value,
        session_id: &str,
        namespace: Option<&str>,
        ttl: Option<i64>,
    ) -> Result<Value, MemoryError>;

    /// Get a session state key.
    async fn state_get(
        &self,
        key: &str,
        session_id: &str,
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError>;

    /// Delete a session state key.
    async fn state_delete(
        &self,
        key: &str,
        session_id: &str,
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError>;

    /// List all session state keys for a session.
    async fn state_list(
        &self,
        session_id: &str,
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError>;

    /// Clear all session state for a session.
    async fn state_clear(
        &self,
        session_id: &str,
        namespace: Option<&str>,
    ) -> Result<Value, MemoryError>;

    /// Which search modes this backend supports (e.g. \["text", "vector", "hybrid"\]).
    /// Used to dynamically build the input schema for memory_search.
    fn supported_search_modes(&self) -> Vec<String>;
}

/// Register all memory tools using the given backend.
///
/// This is the two-step registration pattern:
/// 1. Backend crate's `connect(config)` creates the backend
/// 2. This function registers tool shells that delegate to it
pub async fn register(engine: &Engine, backend: Arc<dyn MemoryBackend>) -> anyhow::Result<()> {
    engine.register_tool(Arc::new(SearchTool::new(backend.clone())));
    engine.register_tool(Arc::new(StoreTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateSetTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateGetTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateDeleteTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateListTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateClearTool::new(backend)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use konflux::Tool;
    use serde_json::json;

    struct MockBackend;

    #[async_trait]
    impl MemoryBackend for MockBackend {
        async fn search(&self, params: SearchParams) -> Result<Value, MemoryError> {
            Ok(json!({"results": [], "query": params.query}))
        }
        async fn add_nodes(
            &self,
            nodes: &[Value],
            _ns: Option<&str>,
        ) -> Result<Value, MemoryError> {
            Ok(json!({"added": nodes.len()}))
        }
        async fn state_set(
            &self,
            key: &str,
            value: &Value,
            _sid: &str,
            _ns: Option<&str>,
            _ttl: Option<i64>,
        ) -> Result<Value, MemoryError> {
            Ok(json!({"key": key, "value": value}))
        }
        async fn state_get(
            &self,
            key: &str,
            _sid: &str,
            _ns: Option<&str>,
        ) -> Result<Value, MemoryError> {
            Ok(json!({"key": key, "value": null}))
        }
        async fn state_delete(
            &self,
            key: &str,
            _sid: &str,
            _ns: Option<&str>,
        ) -> Result<Value, MemoryError> {
            Ok(json!({"deleted": key}))
        }
        async fn state_list(&self, _sid: &str, _ns: Option<&str>) -> Result<Value, MemoryError> {
            Ok(json!({"keys": []}))
        }
        async fn state_clear(&self, _sid: &str, _ns: Option<&str>) -> Result<Value, MemoryError> {
            Ok(json!({"cleared": 0}))
        }
        fn supported_search_modes(&self) -> Vec<String> {
            vec!["text".into(), "hybrid".into()]
        }
    }

    #[test]
    fn test_register_all_tools() {
        let engine = Engine::new();
        let backend = Arc::new(MockBackend);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(register(&engine, backend)).unwrap();

        let registry = engine.registry();
        assert!(registry.contains("memory:search"));
        assert!(registry.contains("memory:store"));
        assert!(registry.contains("state:set"));
        assert!(registry.contains("state:get"));
        assert!(registry.contains("state:delete"));
        assert!(registry.contains("state:list"));
        assert!(registry.contains("state:clear"));
        assert_eq!(registry.len(), 7);
    }

    fn test_ctx() -> konflux::tool::ToolContext {
        konflux::tool::ToolContext {
            capabilities: vec!["memory_*".into()],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: std::collections::HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_search_tool_delegates_to_backend() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = SearchTool::new(backend);
        let result = tool
            .invoke(
                json!({"query": "hello", "mode": "text", "limit": 5}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert_eq!(result["query"], "hello");
    }

    #[tokio::test]
    async fn test_search_tool_dynamic_modes_in_schema() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = SearchTool::new(backend);
        let info = tool.info();
        let modes = info.input_schema["properties"]["mode"]["enum"]
            .as_array()
            .unwrap();
        assert!(modes.contains(&json!("text")));
        assert!(modes.contains(&json!("hybrid")));
    }

    #[tokio::test]
    async fn test_store_tool_delegates_to_backend() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = StoreTool::new(backend);
        let result = tool
            .invoke(json!({"nodes": [{"content": "test"}]}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(result["added"], 1);
    }

    #[tokio::test]
    async fn test_store_tool_rejects_missing_nodes() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = StoreTool::new(backend);
        let result = tool.invoke(json!({}), &test_ctx()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_state_set_delegates() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = StateSetTool::new(backend);
        let result = tool
            .invoke(
                json!({"key": "plan", "value": [1,2,3], "session_id": "s1"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert_eq!(result["key"], "plan");
    }

    #[tokio::test]
    async fn test_state_set_rejects_missing_key() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = StateSetTool::new(backend);
        let result = tool
            .invoke(json!({"value": 1, "session_id": "s1"}), &test_ctx())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_state_get_delegates() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = StateGetTool::new(backend);
        let result = tool
            .invoke(json!({"key": "plan", "session_id": "s1"}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(result["key"], "plan");
    }

    #[tokio::test]
    async fn test_state_delete_delegates() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = StateDeleteTool::new(backend);
        let result = tool
            .invoke(json!({"key": "plan", "session_id": "s1"}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(result["deleted"], "plan");
    }

    #[tokio::test]
    async fn test_state_list_delegates() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = StateListTool::new(backend);
        let result = tool
            .invoke(json!({"session_id": "s1"}), &test_ctx())
            .await
            .unwrap();
        assert!(result["keys"].is_array());
    }

    #[tokio::test]
    async fn test_state_clear_delegates() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);
        let tool = StateClearTool::new(backend);
        let result = tool
            .invoke(json!({"session_id": "s1"}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(result["cleared"], 0);
    }

    #[test]
    fn test_tool_annotations_correct() {
        let backend: Arc<dyn MemoryBackend> = Arc::new(MockBackend);

        let search = SearchTool::new(backend.clone());
        assert!(search.info().annotations.read_only);
        assert!(search.info().annotations.idempotent);

        let store = StoreTool::new(backend.clone());
        assert!(!store.info().annotations.read_only);
        assert!(!store.info().annotations.destructive);

        let delete = StateDeleteTool::new(backend.clone());
        assert!(delete.info().annotations.destructive);

        let clear = StateClearTool::new(backend.clone());
        assert!(clear.info().annotations.destructive);
    }
}
