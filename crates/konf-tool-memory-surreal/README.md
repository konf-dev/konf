# konf-tool-memory-surreal

SurrealDB-backed implementation of the konf [`MemoryBackend`](../konf-tool-memory/src/lib.rs) trait.

One crate, two deployment modes, same query code:

- **Embedded** (`kv-rocksdb`) — single-file RocksDB database, in-process, no daemon. Ideal for local use and for products shipped as a single binary.
- **Remote** (`protocol-ws`) — WebSocket connection to a SurrealDB server. Ideal for shared-state deployments.

Also ships a `memory` mode for ephemeral, in-process storage that is useful for tests.

The backend is the **default** memory backend in konf. Old Postgres deployments using `konf-tool-memory-smrti` keep working behind the `memory-smrti` feature flag; nothing is being taken away.

## Why SurrealDB

- **License twin**: BUSL-1.1, same as konf. Auto-converts to Apache on 2030-01-01.
- **Graph-native**: typed edges via `RELATE` on a `TYPE RELATION` table, not SQL-modelled.
- **Vector search**: built-in HNSW index.
- **Full-text search**: built-in BM25 analyzer with a pluggable tokenizer/filter pipeline.
- **Same SurrealQL in embedded and server mode**: zero-divergence semantics, verified by the crate's integration tests running against the `mem://` backend.

## Feature matrix

| Capability               | Status | Notes |
|---|---|---|
| Node CRUD                 | full   | `CREATE node` with namespace, type, content, metadata. Event row appended in the same statement. |
| Node embeddings           | full   | Stored in a separate `node_embedding` table, keyed by `(node, model_name)`. Multi-model per node supported. |
| HNSW vector index         | full   | `DEFINE INDEX ... HNSW DIMENSION <n> DIST COSINE`. Dimension is set once at connect time. |
| Text search (BM25)        | full   | `FULLTEXT ANALYZER konf_ft BM25 HIGHLIGHTS`. Ranked by `search::score(0)`. |
| Vector search             | full   | `<|K,EF|>` KNN operator + `vector::similarity::cosine` scoring. Caller supplies the query vector via `metadata_filter.query_vector`. |
| Hybrid (RRF fusion)       | full   | `score = Σ 1 / (k + rank_i)` over text + vector ranked lists. `k` defaults to 60, matching smrti. |
| Typed edges (`RELATE`)    | schema | Table and indexes defined; no product currently exercises edge CRUD via the trait. |
| Temporal edges            | schema | `valid_from` / `valid_to` columns present; filter logic will land when a product needs it. |
| Session KV                | full   | `state:set` / `get` / `delete` / `list` / `clear`. |
| Session KV TTL            | full   | TTL expiry is lazy — every `get`/`list` prunes expired rows first, in the same round-trip. No background sweeper. |
| Event log                 | full   | Every `add_nodes` call appends an `event` row in the same transaction as the mutation. |
| Namespace isolation       | full   | Every query carries `WHERE namespace = $ns`. Verified by cross-namespace isolation tests. |

## Configuration

Set the memory backend in your product's `tools.yaml`:

```yaml
tools:
  memory:
    backend: surreal
    config:
      mode: embedded                 # embedded | memory | remote
      path: ./memory.db              # required for mode: embedded
      endpoint: ws://127.0.0.1:8000  # required for mode: remote
      username: ${SURREAL_USER}      # optional, remote mode
      password: ${SURREAL_PASSWORD}  # optional, remote mode
      namespace: konf                # Surreal namespace (tenant scope)
      database: default              # Surreal database
      vector_dimension: 768          # embedding size (must match your embedder)
      rrf_k: 60                      # hybrid-search fusion constant
      hybrid_candidate_pool: 100     # candidates per mode before fusion
      default_limit: 10              # fallback limit when a query omits one
```

All fields except `mode` have sensible defaults. The `path` is required for `embedded` mode; the `endpoint` is required for `remote` mode. Setting an unsupported mode raises a clear validation error at `connect()` time.

### Choosing `vector_dimension`

Must match the embedder your product uses. konf's default embedder (`nomic-embed-text`) produces 768-dim vectors. Changing this after you've stored nodes requires dropping the HNSW index and re-indexing — the backend does not automatically migrate.

## Vector search — how to supply a query vector

The `MemoryBackend` trait's `SearchParams` carries `query: Option<String>` (text) but no dedicated embedding field, so the caller is responsible for pre-embedding the query. This crate reads the pre-embedded vector from `metadata_filter.query_vector`:

```rust
let params = SearchParams {
    query: Some("alpha".into()),   // used for text / hybrid mode
    mode: Some("hybrid".into()),
    namespace: Some("tenant1".into()),
    limit: Some(10),
    metadata_filter: Some(serde_json::json!({
        "query_vector": [0.01, 0.02, /* ... 768 floats total ... */]
    })),
    ..Default::default()
};
```

Dimension mismatches are rejected with a clear validation error. Text-only search does not need `query_vector`; vector-only search requires it; hybrid mode uses whichever inputs are present and degrades gracefully (text-only or vector-only) if one is missing.

## Limitations

- **HNSW dimension is fixed at schema time.** You cannot change it without dropping the index.
- **TTL expiry is lazy**, not background-swept. Expired session-KV rows are deleted when a `get` or `list` touches the same `(namespace, session_id)` tuple. If no one ever reads, expired rows persist until a read lands or the database is compacted.
- **The event log is append-only but not the system of record.** `node` and `edge` rows are mutated directly; the `event` rows are an audit trail.
- **No smrti parity test under Postgres.** This crate is tested against SurrealDB's in-memory engine, not against smrti. Cross-backend behavior is asserted only at the trait level — you can swap backends in `tools.yaml` and the shape of returned values stays the same.

## Running the tests

```bash
cargo test -p konf-tool-memory-surreal
```

All tests run against the `mem://` engine — no external database, no SSH keys, no environment setup. The first build takes several minutes because SurrealDB's `kv-rocksdb` feature pulls in a large native dependency tree (rocksdb, lz4, zstd, aws-lc); subsequent builds are cached.

## License

BUSL-1.1. See [LICENSE](../../LICENSE).
