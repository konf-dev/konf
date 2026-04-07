# Session State Specification

**Status:** Authoritative
**Scope:** Ephemeral KV store for agent working memory

> **Note:** Session state is a MemoryBackend concern — any backend implements the state_* methods. See [memory-backends.md](memory-backends.md) for the trait definition. TTL implementation is backend-specific.

---

## What

A lightweight session-scoped KV store for working memory. Agents use this as a scratchpad for intermediate state during complex tasks — plans, sub-task lists, accumulated results — without cluttering the long-term knowledge graph.

## Why

- Agents working on multi-step tasks need somewhere to store intermediate state
- Storing scratchpad data as graph nodes creates noise (not searchable, not relational, temporary by nature)
- Session state is scoped by namespace + session_id, isolated by VirtualizedTool namespace injection

## Requirements

### API Surface

Part of the `MemoryBackend` trait (see [memory-backends.md](memory-backends.md)):

```rust
async fn state_set(&self, key: &str, value: &Value, session_id: &str, namespace: Option<&str>, ttl: Option<i64>) -> Result<Value, MemoryError>;
async fn state_get(&self, key: &str, session_id: &str, namespace: Option<&str>) -> Result<Value, MemoryError>;
async fn state_delete(&self, key: &str, session_id: &str, namespace: Option<&str>) -> Result<Value, MemoryError>;
async fn state_list(&self, session_id: &str, namespace: Option<&str>) -> Result<Value, MemoryError>;
async fn state_clear(&self, session_id: &str, namespace: Option<&str>) -> Result<Value, MemoryError>;
```

All return `serde_json::Value` with a `_meta` key containing operational metadata. Namespace is injected by VirtualizedTool — tools and agents never set it directly.

### Storage

Each memory backend implements session state storage internally. The data model is:

| Field | Type | Description |
|-------|------|-------------|
| namespace | string | Tenant isolation (injected by VirtualizedTool) |
| session_id | string | Session scope |
| key | string | User-defined key |
| value | JSON | Any JSON value |
| created_at | timestamp | When the key was first set |
| updated_at | timestamp | When the key was last updated |
| expires_at | timestamp (optional) | TTL expiry, null = no expiry |

Primary key: `(namespace, session_id, key)`.

**Backend-specific implementations:**
- **Postgres (smrti):** `session_state` table with JSONB value column, index on expires_at
- **SurrealDB:** Record type with `DEFINE TABLE ... TTL` for automatic expiry
- **SQLite:** Table with JSON column, application-level timer for cleanup

### Behavior

- `state_set` upserts (INSERT ON CONFLICT UPDATE). Overwrites value and resets `updated_at`.
- `state_get` returns `None`/null if key doesn't exist or has expired.
- `state_delete` removes a single key. Returns whether the key existed.
- `state_clear` removes ALL keys for a session. Returns count of keys removed.
- `state_list` returns all non-expired keys for a session as `[(key, value)]`.
- Expired rows are filtered on read (`WHERE expires_at IS NULL OR expires_at > NOW()`).
- Expired rows are cleaned up periodically (lazy cleanup on read is fine; optional background cleanup).

### TTL

`state_set` accepts an optional `ttl_seconds: int` parameter. If set, `expires_at = NOW() + interval`. If not set, `expires_at = NULL` (lives until explicitly cleared or session ends).

```yaml
# In a workflow — expires in 1 hour
nodes:
  save_plan:
    do: state:set
    input:
      key: "plan"
      value: { steps: ["step1", "step2"] }
      session_id: "{{session_id}}"
      ttl_seconds: 3600

  save_prefs:
    do: state:set
    input:
      key: "preferences"
      value: { format: "markdown" }
      session_id: "{{session_id}}"
      # No ttl_seconds — lives until state_clear
```

### Configuration

Backend-specific TTL and cleanup settings are configured in tools.yaml under the memory backend config. Each backend handles expiry in its own way (see [memory-backends.md](memory-backends.md)).

### What This Is NOT

- NOT event-sourced. No events in the event log. This is ephemeral state.
- NOT searchable. No embeddings, no full-text search, no vector index.
- NOT graph data. No nodes, no edges, no relationships.
- NOT a cache. It's user-facing working memory, not an optimization layer.

### Error Handling

- `state_get` for non-existent key: returns `{"value": null, "_meta": {"found": false}}`
- `state_set` with invalid value (not JSON-serializable): raises `ValidationError`
- `state_clear` for non-existent session: returns `{"cleared": 0, "_meta": {...}}` (not an error)

### Testing

- Basic CRUD: set, get, delete, clear, list
- Upsert: set same key twice, verify overwrite
- TTL: set with TTL, verify expiry filtering
- Namespace isolation: set in namespace A, verify not visible in namespace B
- Session isolation: set in session A, verify not visible in session B
- Concurrent access: multiple sets to same key from different tasks
- Large values: JSONB with nested structures, arrays
- state_list ordering: consistent ordering (by key name)
