//! Tool implementations that delegate to a MemoryBackend.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use konflux::error::ToolError;
use konflux::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};

use crate::{MemoryBackend, SearchParams};

fn mem_err(e: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        message: e.to_string(),
        retryable: false,
    }
}

// ============================================================
// memory:search
// ============================================================

/// Search the knowledge graph. Delegates to backend.search().
pub struct SearchTool {
    backend: Arc<dyn MemoryBackend>,
}

impl SearchTool {
    /// Create a new SearchTool backed by the given MemoryBackend.
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn info(&self) -> ToolInfo {
        let modes = self.backend.supported_search_modes();
        ToolInfo {
            name: "memory:search".into(),
            description: "Search the knowledge graph for relevant information. Returns nodes with similarity scores.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for" },
                    "mode": { "type": "string", "enum": modes, "default": modes.first().cloned().unwrap_or_default() },
                    "limit": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                    "node_type": { "type": "string", "description": "Filter by node type" }
                },
                "required": ["query"]
            }),
            output_schema: None,
            capabilities: vec!["memory:search".into()],
            supports_streaming: false,
            annotations: ToolAnnotations { read_only: true, idempotent: true, ..Default::default() },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let params = SearchParams {
            query: input.get("query").and_then(|v| v.as_str()).map(String::from),
            mode: input.get("mode").and_then(|v| v.as_str()).map(String::from),
            limit: input.get("limit").and_then(|v| v.as_i64()),
            namespace: input.get("namespace").and_then(|v| v.as_str()).map(String::from),
            node_type: input.get("node_type").and_then(|v| v.as_str()).map(String::from),
            edge_type: input.get("edge_type").and_then(|v| v.as_str()).map(String::from),
            metadata_filter: input.get("metadata_filter").cloned(),
            min_similarity: input.get("min_similarity").and_then(|v| v.as_f64()),
        };
        self.backend.search(params).await.map_err(mem_err)
    }
}

// ============================================================
// memory:store
// ============================================================

/// Add nodes to the knowledge graph. Delegates to backend.add_nodes().
pub struct StoreTool {
    backend: Arc<dyn MemoryBackend>,
}

impl StoreTool {
    /// Create a new StoreTool.
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for StoreTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "memory:store".into(),
            description: "Add nodes to the knowledge graph.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "nodes": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string" },
                                "node_type": { "type": "string", "default": "memory" },
                                "metadata": { "type": "object" }
                            },
                            "required": ["content"]
                        }
                    }
                },
                "required": ["nodes"]
            }),
            output_schema: None,
            capabilities: vec!["memory:store".into()],
            supports_streaming: false,
            annotations: ToolAnnotations::default(),
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let namespace = input.get("namespace").and_then(|v| v.as_str());
        let nodes = input.get("nodes").and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "missing 'nodes' array".into(),
                field: Some("nodes".into()),
            })?;
        self.backend.add_nodes(nodes, namespace).await.map_err(mem_err)
    }
}

// ============================================================
// state:set
// ============================================================

/// Set a session state key. Delegates to backend.state_set().
pub struct StateSetTool {
    backend: Arc<dyn MemoryBackend>,
}

impl StateSetTool {
    /// Create a new StateSetTool.
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for StateSetTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "state:set".into(),
            description: "Set a session state key (working memory scratchpad).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "value": { "description": "Any JSON value" },
                    "session_id": { "type": "string" },
                    "ttl_seconds": { "type": "integer", "description": "Time-to-live in seconds" }
                },
                "required": ["key", "value", "session_id"]
            }),
            output_schema: None,
            capabilities: vec!["state:set".into()],
            supports_streaming: false,
            annotations: ToolAnnotations { idempotent: true, ..Default::default() },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let namespace = input.get("namespace").and_then(|v| v.as_str());
        let key = input.get("key").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'key'".into(), field: Some("key".into()) })?;
        let value = input.get("value")
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'value'".into(), field: Some("value".into()) })?;
        let session_id = input.get("session_id").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'session_id'".into(), field: Some("session_id".into()) })?;
        let ttl = input.get("ttl_seconds").and_then(|v| v.as_i64());

        self.backend.state_set(key, value, session_id, namespace, ttl).await.map_err(mem_err)
    }
}

// ============================================================
// state:get
// ============================================================

/// Get a session state key. Delegates to backend.state_get().
pub struct StateGetTool {
    backend: Arc<dyn MemoryBackend>,
}

impl StateGetTool {
    /// Create a new StateGetTool.
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for StateGetTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "state:get".into(),
            description: "Get a session state value by key.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "session_id": { "type": "string" }
                },
                "required": ["key", "session_id"]
            }),
            output_schema: None,
            capabilities: vec!["state:get".into()],
            supports_streaming: false,
            annotations: ToolAnnotations { read_only: true, idempotent: true, ..Default::default() },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let namespace = input.get("namespace").and_then(|v| v.as_str());
        let key = input.get("key").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'key'".into(), field: Some("key".into()) })?;
        let session_id = input.get("session_id").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'session_id'".into(), field: Some("session_id".into()) })?;

        self.backend.state_get(key, session_id, namespace).await.map_err(mem_err)
    }
}

// ============================================================
// state:delete
// ============================================================

/// Delete a session state key. Delegates to backend.state_delete().
pub struct StateDeleteTool {
    backend: Arc<dyn MemoryBackend>,
}

impl StateDeleteTool {
    /// Create a new StateDeleteTool.
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for StateDeleteTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "state:delete".into(),
            description: "Delete a session state key.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "session_id": { "type": "string" }
                },
                "required": ["key", "session_id"]
            }),
            output_schema: None,
            capabilities: vec!["state:delete".into()],
            supports_streaming: false,
            annotations: ToolAnnotations { destructive: true, ..Default::default() },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let namespace = input.get("namespace").and_then(|v| v.as_str());
        let key = input.get("key").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'key'".into(), field: Some("key".into()) })?;
        let session_id = input.get("session_id").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'session_id'".into(), field: Some("session_id".into()) })?;

        self.backend.state_delete(key, session_id, namespace).await.map_err(mem_err)
    }
}

// ============================================================
// state:list
// ============================================================

/// List all session state keys. Delegates to backend.state_list().
pub struct StateListTool {
    backend: Arc<dyn MemoryBackend>,
}

impl StateListTool {
    /// Create a new StateListTool.
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for StateListTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "state:list".into(),
            description: "List all session state keys for a session.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" }
                },
                "required": ["session_id"]
            }),
            output_schema: None,
            capabilities: vec!["state:list".into()],
            supports_streaming: false,
            annotations: ToolAnnotations { read_only: true, idempotent: true, ..Default::default() },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let namespace = input.get("namespace").and_then(|v| v.as_str());
        let session_id = input.get("session_id").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'session_id'".into(), field: Some("session_id".into()) })?;

        self.backend.state_list(session_id, namespace).await.map_err(mem_err)
    }
}

// ============================================================
// state:clear
// ============================================================

/// Clear all session state for a session. Delegates to backend.state_clear().
pub struct StateClearTool {
    backend: Arc<dyn MemoryBackend>,
}

impl StateClearTool {
    /// Create a new StateClearTool.
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for StateClearTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "state:clear".into(),
            description: "Clear all session state for a session.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" }
                },
                "required": ["session_id"]
            }),
            output_schema: None,
            capabilities: vec!["state:clear".into()],
            supports_streaming: false,
            annotations: ToolAnnotations { destructive: true, ..Default::default() },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let namespace = input.get("namespace").and_then(|v| v.as_str());
        let session_id = input.get("session_id").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'session_id'".into(), field: Some("session_id".into()) })?;

        self.backend.state_clear(session_id, namespace).await.map_err(mem_err)
    }
}
