# Memory Backends Specification

**Status:** Authoritative
**Crate:** `konf-tool-memory` (trait + tools) + `konf-tool-memory-*` (implementations)
**Role:** VFS — pluggable storage behind a common interface

---

## Overview

Memory is just a tool. The engine, runtime, and workflows don't know which database backs it. The `MemoryBackend` trait defines the interface. Implementations live in separate crates. Backend selection is config-driven via tools.yaml.

---

## MemoryBackend Trait

Every memory backend implements this trait. Tools in `konf-tool-memory` hold `Arc<dyn MemoryBackend>` and delegate to it.

```rust
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Search for nodes matching a query
    async fn search(&self, params: SearchParams) -> Result<Value, MemoryError>;

    /// Add nodes to the graph
    async fn add_nodes(&self, nodes: &[Value], namespace: Option<&str>) -> Result<Value, MemoryError>;

    /// Session state operations
    async fn state_set(&self, key: &str, value: &Value, session_id: &str, namespace: Option<&str>, ttl: Option<i64>) -> Result<Value, MemoryError>;
    async fn state_get(&self, key: &str, session_id: &str, namespace: Option<&str>) -> Result<Value, MemoryError>;
    async fn state_delete(&self, key: &str, session_id: &str, namespace: Option<&str>) -> Result<Value, MemoryError>;
    async fn state_list(&self, session_id: &str, namespace: Option<&str>) -> Result<Value, MemoryError>;
    async fn state_clear(&self, session_id: &str, namespace: Option<&str>) -> Result<Value, MemoryError>;

    /// Which search modes this backend supports (e.g. ["text", "vector", "hybrid"])
    fn supported_search_modes(&self) -> Vec<String>;
}
```

### Optional methods

Backends MAY additionally implement extended operations. Default implementations return `MemoryError::Unsupported`.

```rust
#[async_trait]
pub trait MemoryBackendExt: MemoryBackend {
    async fn traverse(&self, start_node: &str, depth: u32, edge_type: Option<&str>, direction: Option<&str>) -> Result<Value, MemoryError>;
    async fn aggregate(&self, query: AggregateQuery) -> Result<Value, MemoryError>;
    async fn update_node(&self, id: &str, content: Option<&str>, metadata: Option<&Value>, node_type: Option<&str>) -> Result<Value, MemoryError>;
    async fn retract_node(&self, id: &str) -> Result<Value, MemoryError>;
    async fn add_edges(&self, edges: &[Value], namespace: Option<&str>) -> Result<Value, MemoryError>;
    async fn retract_edge(&self, id: &str) -> Result<Value, MemoryError>;
    async fn merge_nodes(&self, keep_id: &str, remove_id: &str) -> Result<Value, MemoryError>;
}
```

### SearchParams

```rust
pub struct SearchParams {
    pub query: Option<String>,
    pub mode: Option<String>,           // "hybrid", "vector", "text"
    pub limit: Option<i64>,
    pub namespace: Option<String>,
    pub node_type: Option<String>,
    pub edge_type: Option<String>,
    pub metadata_filter: Option<Value>,
    pub min_similarity: Option<f64>,
}
```

If the requested search mode is not supported by the backend, the tool falls back to the first available mode and includes a `_meta.fallback_mode` field in the response.

---

## Tool Registration

`konf-tool-memory` provides the tool shells. konf-init passes the configured backend:

```rust
// konf-tool-memory/src/lib.rs
pub async fn register(
    engine: &Engine,
    backend: Arc<dyn MemoryBackend>,
) -> anyhow::Result<()> {
    engine.register_tool(Arc::new(SearchTool::new(backend.clone())));
    engine.register_tool(Arc::new(StoreTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateSetTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateGetTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateDeleteTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateListTool::new(backend.clone())));
    engine.register_tool(Arc::new(StateClearTool::new(backend.clone())));
    Ok(())
}
```

Each tool struct holds `Arc<dyn MemoryBackend>` and delegates to it:

```rust
pub struct SearchTool {
    backend: Arc<dyn MemoryBackend>,
}

#[async_trait]
impl Tool for SearchTool {
    fn info(&self) -> ToolInfo {
        let modes = self.backend.supported_search_modes();
        ToolInfo {
            name: "memory:search".into(),
            description: "Search the knowledge graph for relevant information.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "mode": { "type": "string", "enum": modes },
                    "limit": { "type": "integer", "default": 10 },
                },
                "required": ["query"]
            }),
            // ... output_schema, capabilities, annotations
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let params = SearchParams::from_value(&input)?;
        self.backend.search(params).await.map_err(|e| ToolError::ExecutionFailed {
            message: e.to_string(),
            retryable: false,
        })
    }
}
```

The `input_schema` dynamically reflects the backend's capabilities — the agent sees only what the backend actually supports.

---

## Namespace Injection

Multi-tenancy is enforced at the tool level, not the backend level:

1. Product config grants capabilities with bindings: `{ pattern: "memory:*", bindings: { namespace: "konf:product:${user_id}" } }`
2. `VirtualizedTool` wraps each memory tool, injecting `namespace` into input before the tool sees it
3. The backend receives `namespace` as a parameter and filters by it
4. The LLM cannot override the injected namespace — bindings overwrite any existing keys

Backends handle namespace isolation internally:
- **Postgres (smrti):** WHERE clause on namespace column, optionally RLS
- **SurrealDB:** Native namespace/database isolation
- **SQLite:** WHERE clause on namespace column

---

## Backend Implementations

> **Backend implementations live in EXTERNAL repos, not in this monorepo.**
> - `konf-tool-memory-smrti` lives in the [konf-dev/smrti](https://github.com/konf-dev/smrti) repo.
> - SurrealDB and SQLite backends are **planned, not yet implemented**.

### konf-tool-memory-smrti (Postgres + pgvector)

Wraps the existing `smrti::Memory` crate.

```rust
pub struct SmrtiBackend {
    memory: Arc<smrti_core::Memory>,
}

impl MemoryBackend for SmrtiBackend {
    async fn search(&self, params: SearchParams) -> Result<Value, MemoryError> {
        self.memory.search(/* map params */).await.map_err(Into::into)
    }
    // ... delegate all methods
}

pub async fn connect(config: &Value) -> Result<Arc<dyn MemoryBackend>, anyhow::Error> {
    let smrti_config: SmrtiConfig = serde_json::from_value(config.clone())?;
    let memory = smrti_core::Memory::connect(smrti_config).await?;
    Ok(Arc::new(SmrtiBackend { memory: Arc::new(memory) }))
}
```

**Supports:** hybrid/vector/text search, graph traversal, aggregation, event sourcing, full-text search, session state with TTL
**Best for:** Server deployments with existing Postgres infrastructure
**Requires:** PostgreSQL with pgvector extension

### konf-tool-memory-surrealdb (Edge + Cloud universal)

Single codebase, runs everywhere. Connection string determines deployment tier.

```rust
pub struct SurrealBackend {
    db: Surreal<Any>,  // engine::any — same API for rocksdb, memory, ws
}

pub async fn connect(config: &Value) -> Result<Arc<dyn MemoryBackend>, anyhow::Error> {
    let dsn: String = config.get("dsn").unwrap().as_str().unwrap().into();
    let db = Surreal::new::<Any>(&dsn).await?;
    // ... schema setup
    Ok(Arc::new(SurrealBackend { db }))
}
```

Connection strings:
- `rocksdb://local.db` — phone/edge (embedded, no server)
- `mem://` — in-memory (testing)
- `wss://cluster.konf.dev` — production cluster (distributed)

**Supports:** vector search (HNSW), full-text search, graph queries, namespace isolation, session state
**Best for:** Universal deployment (same code edge and cloud)
**Requires:** SurrealDB crate (embedded) or SurrealDB server (cluster)

### konf-tool-memory-sqlite (Ultra-lightweight edge)

SQLite with extensions for vector search and full-text search.

**Supports:** vector search (sqlite-vec), full-text search (FTS5), session state
**Best for:** Ultra-lightweight edge, mobile, WASM
**Requires:** SQLite with sqlite-vec and FTS5 extensions

---

## Configuration

```yaml
# tools.yaml
memory:
  backend: surrealdb
  config:
    dsn: "rocksdb://local.db"
```

konf-init reads this, calls the corresponding backend's `connect(config)`, then passes the result to `konf_tool_memory::register(engine, backend)`.

### Multi-backend overrides (future)

The architecture supports using different backends for different tools. For example, graph memory on SurrealDB with session state on a separate backend. This requires implementing a second `MemoryBackend` crate and configuring overrides in tools.yaml:

```yaml
memory:
  backend: surrealdb
  config:
    dsn: "rocksdb://graph.db"

# Future: per-tool backend overrides
# overrides:
#   state:set:
#     backend: another-backend
#     config: { ... }
```

Currently, all memory tools use the same backend. Multi-backend overrides are a planned extension point.

---

## Capability Requirements

Memory and state tools use **different capability prefixes**. A grant of `memory:*` does NOT cover `state:*` tools and vice versa. This is intentional — graph memory and session state are separate security domains.

| Tool | Required capability | Prefix |
|------|-------------------|--------|
| `memory:search` | `memory:search` or `memory:*` | `memory:` |
| `memory:store` | `memory:store` or `memory:*` | `memory:` |
| `state:set` | `state:set` or `state:*` | `state:` |
| `state:get` | `state:get` or `state:*` | `state:` |
| `state:delete` | `state:delete` or `state:*` | `state:` |
| `state:list` | `state:list` or `state:*` | `state:` |
| `state:clear` | `state:clear` or `state:*` | `state:` |

**Common grant patterns:**
- Full memory + state access: `["memory:*", "state:*"]`
- Read-only memory: `["memory:search"]`
- Full state, no memory: `["state:*"]`
- Everything: `["*"]`

> **Warning:** Granting only `memory:*` will NOT allow the agent to use session state. Always include `state:*` if your workflows use `state:set`/`state:get`.

---

## Extended Operations (MemoryBackendExt)

The `MemoryBackendExt` trait defines optional operations: `traverse`, `aggregate`, `update_node`, `retract_node`, `add_edges`, `retract_edge`, `merge_nodes`. These are **not registered as tools by default** — the base `register()` function only registers the 7 core tools listed above.

To expose extended operations as tools, a backend crate can provide an additional registration function that creates tools for each supported operation. konf-init calls this only if the backend reports support. Tools like `memory:traverse` (requiring `memory:traverse` capability) become available only when the backend implements them.

This is a future extension point. The core 7 tools cover the most common agent workflows. Extended operations are for advanced use cases like graph exploration and batch processing.

---

## Related Specs

- [konf-tools-spec](konf-tools-spec.md) — tool protocol, plugin crate structure
- [konf-engine-spec](konf-engine-spec.md) — Tool trait, ToolInfo
- [konf-architecture](konf-architecture.md) — VFS analogy, pluggable storage vision
- [session-state](session-state.md) — session state API details, TTL behavior
- [multi-tenancy](multi-tenancy.md) — namespace hierarchy, VirtualizedTool injection
- [configuration-strategy](configuration-strategy.md) — tools.yaml format
