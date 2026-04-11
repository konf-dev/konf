# konf audit — issues PRD

Generated: 2026-04-11
Scope: konf main branch at `5a2de8093ea4b3b991269b52f11b12086ab4c2d0`
Auditor: fresh-eyes pass, no prior context about the project

## Summary

- CRITICAL: 3
- HIGH: 5
- MEDIUM: 7
- LOW: 4
- NIT: 4

---

## CRITICAL issues

### C1. `wait_terminal` has a lost-wakeup race that can hang `runner:wait` indefinitely

**File**: `crates/konf-tool-runner/src/registry.rs:173-181`
**Category**: logic-bug

```rust
pub async fn wait_terminal(&self, id: &RunId) -> Option<RunRecord> {
    let slot = self.slot(id)?;
    loop {
        let current = { slot.record.read().await.clone() };
        if current.state.is_terminal() {
            return Some(current);
        }
        slot.done.notified().await;   // ← RACE HERE
    }
}
```

**Why it's suspicious**: `Tokio::sync::Notify::notify_waiters()` only wakes futures that are **already suspended** in `notified().await`. If the background task completes and calls `notify_waiters()` in the window between the `is_terminal()` check returning `false` and the `notified().await` future being polled, the notification is lost and the loop suspends forever (until the next unrelated notification). The correct fix is `notify_one()` combined with `Notify::new_with_notify_on_drop()`, or replacing `Notify` with `tokio::sync::watch`.

---

### C2. `gate` workflow description is a false claim — `cargo_test_crate` is never spawned and `input.crates` is never used

**File**: `products/ci/config/workflows/gate.yaml:1-51`
**Category**: false-claim

```yaml
description: |
  Parallel "ship gate" workflow. Spawns cargo_fmt_check, cargo_clippy, and
  cargo_test_crate for each crate in `input.crates` as concurrent runs via
  runner:spawn ...
input_schema:
  type: object
  properties:
    crates:
      type: array
      ...
  required: ["crates"]
nodes:
  spawn_fmt:
    do: runner:spawn
    with: { workflow: cargo_fmt_check }
  spawn_clippy:
    do: runner:spawn
    with: { workflow: cargo_clippy }
  # (no cargo_test_crate spawn, no iteration over input.crates)
```

**Why it's suspicious**: The `description` states the workflow spawns `cargo_test_crate` for each crate in `input.crates`. Neither is true: `cargo_test_crate` is never spawned, the workflow never reads `input.crates`, and there is no iteration construct. Callers passing a `crates` array get silently ignored output. The workflow only runs `cargo_fmt_check` and `cargo_clippy` as a flat pair, which is also what the README says — the description is the lie, not the README.

---

### C3. `MENTAL_MODEL.md` still says "Postgres with pgvector is required for memory-backed products" after SurrealDB became the default

**File**: `docs/MENTAL_MODEL.md:72-73`
**Category**: false-claim

```
Postgres with pgvector is required for memory-backed products. Everything else
(scheduler, journal) is optional and degrades gracefully.
```

**Why it's suspicious**: `konf-tool-memory-surreal` (SurrealDB, embedded RocksDB) is now the default memory backend. It requires no Postgres, no pgvector, and no daemon. The sentence directly contradicts the codebase it is supposed to document and will mislead every new reader about setup requirements.

---

## HIGH issues

### H1. `vector_search` does not filter `is_retracted = false`, returning deleted nodes in semantic search results

**File**: `crates/konf-tool-memory-surreal/src/search.rs:117-168`
**Category**: logic-bug

```sql
SELECT
    node.id           AS id,
    node.node_type    AS node_type,
    node.content      AS content,
    node.metadata     AS metadata,
    vector::similarity::cosine(embedding, $qv) AS score
  FROM node_embedding
 WHERE namespace = $ns
   AND embedding <|{k},{ef}|> $qv
```

**Why it's suspicious**: `text_search` explicitly filters `AND is_retracted = false` (line 85), but `vector_search` queries the `node_embedding` table via the `node.*` relation and never applies this filter. Retracted nodes (soft-deleted) can surface in vector results even though they are invisible to text search. The hybrid RRF path inherits this bug because it calls `vector_search` directly.

---

### H2. `add_nodes` with embeddings is non-atomic: node can be created without its embedding row

**File**: `crates/konf-tool-memory-surreal/src/backend.rs:136-172`
**Category**: failure-mode

```rust
let sql = if has_embedding {
    r#"
LET $created = (CREATE node SET ...);
CREATE node_embedding SET node = $created[0].id, ...;
"#
} else { ... };
```

**Why it's suspicious**: SurrealDB does not wrap multi-statement queries in a transaction automatically. If the `CREATE node_embedding` statement fails (type mismatch, dimension violation, DB error), the node row already exists but has no embedding. The node is then findable by text search but silently invisible to vector and hybrid search. The code has no rollback, and there is no `BEGIN TRANSACTION / COMMIT` block.

---

### H3. `state_set` DELETE-then-CREATE is non-atomic: concurrent writers can create duplicate rows despite the `UNIQUE` index

**File**: `crates/konf-tool-memory-surreal/src/session.rs:59-117`
**Category**: logic-bug

```rust
// "upsert" via DELETE-then-CREATE
backend.db().query(delete_sql) ...
backend.db().query(create_sql) ...
```

**Why it's suspicious**: Two concurrent `state_set` calls for the same `(namespace, session_id, key)` tuple can both execute their DELETE before either issues the CREATE. Both then attempt to CREATE, and one will violate the `UNIQUE` index on `session_state_pk` — surfacing as an error to the caller. SurrealDB's `UPSERT ... ON CONFLICT` or a `BEGIN TRANSACTION` block would make this atomic.

---

### H4. `docs/architecture/init.md` `KonfInstance` struct definition is stale — shows `engine` field that was removed

**File**: `docs/architecture/init.md:19-30`
**Category**: doc-code-drift

```rust
pub struct KonfInstance {
    pub engine: Arc<Engine>,    // ← does not exist in actual code
    pub runtime: Arc<Runtime>,
    ...
}
```

**Why it's suspicious**: The actual `KonfInstance` struct in `crates/konf-init/src/lib.rs:29-45` has no `engine` field (only `runtime`, `config`, `product_config`, and the feature-gated `pool`). The doc comment in the source code explicitly says "The `engine` field is dropped" (line 218). Any code example in the architecture doc that accesses `instance.engine` will fail to compile.

---

### H5. `docs/architecture/init.md` boot sequence is wrong in multiple ways

**File**: `docs/architecture/init.md:43-56`
**Category**: doc-code-drift

```
4. Validate product config — ...
8. Register workflows as tools
9. Register resources
11. Create runtime
```

**Why it's suspicious**: Step 4 does not exist in the actual code (no product config validation step). Step 8 says "Register workflows as tools" but in the code this happens at step 10 (after runtime creation), not step 8. Step 9 says "Register resources" which happens at step 7 in the code. Step 11 says "Create runtime" but the code labels it step 9. The sequence is out of sync with the implementation and misnumbers several insertions (9b/9c/9d runner registration is not mentioned at all). The init.md also notes memory backend implementations are "external dependencies, not part of this monorepo" — which is now false since `konf-tool-memory-surreal` is in the monorepo.

---

## MEDIUM issues

### M1. `resolve_namespace(None)` always returns `"default"`, ignoring `cfg.namespace` — silently misdirects all un-namespaced writes

**File**: `crates/konf-tool-memory-surreal/src/backend.rs:46-50`, `crates/konf-tool-memory-surreal/src/config.rs:6`
**Category**: hidden-assumption

```rust
pub const DEFAULT_NAMESPACE: &str = "default";

pub(crate) fn resolve_namespace(&self, param: Option<&str>) -> String {
    param
        .map(str::to_string)
        .unwrap_or_else(|| DEFAULT_NAMESPACE.to_string())  // always "default"
}
```

**Why it's suspicious**: `SurrealConfig.namespace` defaults to `"konf"` (not `"default"`). `resolve_namespace(None)` returns `"default"` regardless of the configured namespace. A product that configures `namespace: my_product` and calls any memory operation without an explicit namespace parameter will have its data silently written to `"default"` instead, mixing it with all other un-namespaced data. The configured namespace is only used for the SurrealDB `USE NS` scope, not as the data namespace default.

---

### M2. `konf-tool-runner` crate doc says "they see the five tools" but only four tools are registered

**File**: `crates/konf-tool-runner/src/lib.rs:20`
**Category**: doc-code-drift

```rust
//! `konf-runtime`. Callers never see the trait — they see the five tools.
```

**Why it's suspicious**: Only four tools are registered: `runner:spawn`, `runner:status`, `runner:wait`, `runner:cancel`. The module comment references "five tools" with no corresponding fifth tool in the code or the `register()` function.

---

### M3. Session TTL expression uses fragile string concatenation in SurrealQL

**File**: `crates/konf-tool-memory-surreal/src/session.rs:82-88`
**Category**: bad-design

```sql
expires_at = time::now() + <duration>($ttl + "s")
```

With binding: `q = q.bind(("ttl", seconds.to_string()));`

**Why it's suspicious**: `$ttl` is bound as a Rust `String` (e.g. `"30"`). The SurrealQL expression `$ttl + "s"` performs string concatenation to produce `"30s"` and then casts it with `<duration>(...)`. This is fragile: if SurrealDB's type coercion rules for `+` change between versions, or if the binding serializes the integer differently, the TTL silently becomes `NONE` or errors. Binding `ttl` as an integer and using a SurrealQL duration literal (`time::now() + <duration>(string::concat($ttl, "s"))`) or a precomputed `chrono::DateTime` would be more robust.

---

### M4. `reapply_schema` in `connect.rs` is dead code but marked `pub(crate)` with `#[allow(dead_code)]`

**File**: `crates/konf-tool-memory-surreal/src/connect.rs:103-117`
**Category**: dead-code

```rust
#[allow(dead_code)]
pub(crate) async fn reapply_schema(
    db: &Surreal<Any>,
    cfg: &SurrealConfig,
) -> Result<(), MemoryError> {
```

**Why it's suspicious**: The function is suppressed with `#[allow(dead_code)]` rather than being removed or wired up. The comment says it is "kept `pub(crate)` so the backend can invoke it in recovery paths without reopening a connection", but no recovery path exists or is referenced. This is speculative generality (YAGNI) with dead code masking a real warning.

---

### M5. Boot sequence doc-comment in `konf-init/src/lib.rs` says "12-step" but lists only 11 steps

**File**: `crates/konf-init/src/lib.rs:50-61`
**Category**: doc-code-drift

```rust
/// The 12-step boot sequence:
/// 1. Load platform config (konf.toml + env vars)
/// ...
/// 11. Return KonfInstance
```

**Why it's suspicious**: The docstring says "12-step" but the enumerated list contains only 11 items. The actual code now has several sub-steps (9b, 9c, 9d), and the docstring numbering is out of sync with both the claim and the implementation.

---

### M6. `resp_count` in `search.rs` is dead code that always returns 0 and is called with a dummy argument

**File**: `crates/konf-tool-memory-surreal/src/search.rs:112-113, 276-278`
**Category**: dead-code

```rust
"_meta": { "mode": "text", "namespace": ns, "count": resp_count(&Value::Null) }
...

#[allow(dead_code)]
fn resp_count(_v: &Value) -> usize {
    0
}
```

**Why it's suspicious**: `resp_count` is called with `&Value::Null` and always returns `0`. The result is immediately overridden by `.merged_count()` which correctly counts rows. The function signature takes a `&Value` argument it ignores, which is suppressed with `#[allow(dead_code)]`. The call-site is misleading: the initial `count: 0` in the JSON is a transient value that `.merged_count()` fixes, but a future reader may not notice the correction.

---

### M7. `Cargo.toml` description for `konf-init-kell` uses the banned word "kell"

**File**: `crates/konf-init-kell/Cargo.toml:7`
**Category**: doc-code-drift (banned vocabulary)

```toml
description = "CLI tool to scaffold new Konf kells (products)"
```

**Why it's suspicious**: `MENTAL_MODEL.md` explicitly bans the word "kell" and the docs-lint kill list states: "deprecated term; use 'product'". The Cargo.toml description is a public-facing string (appears in `cargo search`, crates.io) and uses the banned term.

---

## LOW issues

### L1. `konf-tool-runner` lib.rs comment says `InlineRunner` is exported but misidentifies the stable API surface

**File**: `crates/konf-tool-runner/src/lib.rs:19-21`
**Category**: doc-code-drift

```rust
//! The `Runner` trait therefore lives **inside** this crate, not in
//! `konf-runtime`. Callers never see the trait — they see the five tools.
```

**Why it's suspicious**: `Runner`, `WorkflowSpec`, `RunRegistry`, `RunRecord`, `RunState`, and `InlineRunner` are all re-exported as `pub` (lines 41-44). Callers absolutely can see and use the `Runner` trait — it is part of the crate's public API. The comment is inaccurate on two counts: it undercounts the tools (four, not five) and incorrectly claims the trait is hidden.

---

### L2. `products/ci/README.md` references a non-existent finding file

**File**: `products/ci/README.md:52`
**Category**: doc-code-drift

```
see `konf-experiments/findings/017-runner-tool-not-kernel.md`
```

**Why it's suspicious**: The actual file is `konf-experiments/findings/017-runner-is-a-tool-family.md`. The referenced path does not exist and will produce a broken link for anyone following the cross-reference.

---

### L3. Registry grows unbounded — runs are never evicted

**File**: `crates/konf-tool-runner/src/registry.rs:122-125`
**Category**: failure-mode

```rust
pub struct RunRegistry {
    inner: Arc<PapayaMap<RunId, Arc<RunSlot>>>,
}
```

**Why it's suspicious**: Completed runs (Succeeded, Failed, Cancelled) are never removed from the registry. In any long-running deployment, or during a CI fan-out that runs hundreds of crates, the map grows without bound and is never GC'd. There is no eviction policy, no TTL, and no documented acknowledgement that this is intentional ("the caller should query status and move on").

---

### L4. `add_nodes` counts `created_count` but does not distinguish nodes that failed mid-batch from nodes that succeeded

**File**: `crates/konf-tool-memory-surreal/src/backend.rs:99-175`
**Category**: logic-bug

```rust
for node in nodes {
    // ... run query ...
    q.await.map_err(map_db_error)?...;
    created_count += 1;
}
```

**Why it's suspicious**: If any single node fails (e.g. content validation, DB error), the function returns an error, but previously written nodes in the batch are not rolled back (see H2). The caller gets an error with no indication of how many nodes were committed. This is correct behavior if the contract is "all or nothing," but it is not documented, and the implementation is not "all or nothing" in practice.

---

## NIT issues

### N1. `konf-init/src/lib.rs` step 7 label in the code comment ("Register config files as resources") doesn't match the docstring step 7 ("Register workflows as tools")

**File**: `crates/konf-init/src/lib.rs:129`
**Category**: doc-code-drift

The inline code comment says `// 7. Register config files as resources` but the docstring step 7 says "Register workflows as tools". Minor numbering drift that accumulated when steps were inserted.

---

### N2. `schema.rs` comment says "SurrealDB 3.x: use FULLTEXT (the SEARCH form is legacy)" but search.rs uses `@0@ $q` which is the old SEARCH operator

**File**: `crates/konf-tool-memory-surreal/src/schema.rs:37`, `crates/konf-tool-memory-surreal/src/search.rs:86`
**Category**: doc-code-drift (uncertain)

The schema comment warns that the SEARCH form is legacy and that FULLTEXT is the v3 way. The `text_search` query uses `content @0@ $q` which is SurrealDB's BM25 search operator. Whether `@0@` is the correct SurrealDB 3.x syntax for querying a `FULLTEXT` index needs verification against the surrealdb 3.x docs. If it is legacy syntax, text search is silently falling back to a table scan or failing.

---

### N3. `records = engine.resources().len()` at boot-complete log is reported from the `engine` variable, not `runtime.engine()`

**File**: `crates/konf-init/src/lib.rs:207-210`
**Category**: other

```rust
info!(
    tools = final_tool_count,
    resources = engine.resources().len(),
    ...
);
```

`final_tool_count` comes from `runtime.engine()` (line 205) but `resources` comes from `engine` (the local clone). Since `Engine::clone()` shares Arc-backed state (all fields are `Arc<RwLock<...>>`), this is functionally correct today. However it is a subtle inconsistency: if the Engine's clone semantics ever change (e.g. snapshot vs shared), the log would silently report stale resource counts.

---

### N4. `wait_unknown_run_errors` test assertion accepts both "not found" and "timed out" — masks the real behavior

**File**: `crates/konf-tool-runner/tests/tool_surface.rs:128-131`
**Category**: hidden-assumption

```rust
assert!(
    msg.contains("not found") || msg.contains("timed out"),
    "unexpected error: {msg}"
);
```

The test passes a `timeout_secs: 1` for an unknown `run_id`. The test accepts either error message, so it would also pass if the actual behavior changes from "not found" to "wait forever then time out." This masks whether the implementation correctly returns an immediate "not found" or wastes a full second waiting. The correct behavior (not found immediately, no timeout wasted) is not asserted.

---

## Questions the user must answer

1. **`resolve_namespace` intent (M1):** Should `resolve_namespace(None)` fall back to `cfg.namespace` (the product's configured namespace) or to the hardcoded `"default"` string? The current behavior of always using `"default"` means the `namespace` config field has no effect on operations that don't pass an explicit namespace.

2. **`gate` workflow scope (C2):** Was `cargo_test_crate` fan-out supposed to be implemented and then dropped from scope, or was the description just written ahead of the implementation and never corrected? The `input.crates` field being required but unused suggests the former.

3. **`notify_waiters` vs `notify_one` (C1):** The fix for the lost-wakeup race requires replacing `notify_waiters()` with a mechanism that doesn't miss notifications. Is there any design reason `notify_waiters()` was chosen over `tokio::sync::watch`?

4. **Registry eviction (L3):** Is the unbounded registry growth intentional? If konf-mcp runs indefinitely against `products/ci/`, repeated gate calls will accumulate every run forever.

5. **`@0@` operator in SurrealDB 3.x (N2):** The project targets `surrealdb = "3"` — does `@0@ $q` correctly query the `FULLTEXT BM25 HIGHLIGHTS` index in v3, or does v3 require a different syntax?

---

## What was NOT audited (honest scope)

- `crates/konf-backend/` — HTTP transport, auth (JWT), scheduler, SSE streaming. Not touched in the recent commits; only inspected for `.unwrap()` calls.
- `crates/konf-mcp/` — MCP wire protocol, tool-name translation, stdio/SSE transport.
- `crates/konf-runtime/` — `ExecutionScope`, `VirtualizedTool`, `GuardedTool`, capability lattice. Only `workflow_tool.rs` was spot-checked.
- `crates/konflux-core/` — Workflow parser, compiler, executor, DAG engine. Only reviewed for `for_each`/fan-out primitives.
- `crates/konf-tool-embed/`, `crates/konf-tool-http/`, `crates/konf-tool-llm/`, `crates/konf-tool-secret/`, `crates/konf-tool-shell/`, `crates/konf-tool-mcp/` — Not touched in recent commits.
- `crates/konf-init-kell/` — Scaffolding CLI. Not touched recently.
- `products/devkit/` and `products/init/` — Not part of the Phase 1–3 scope.
- `konf-experiments/findings/` 001–010, 012–015 — Only 011 and 017 were verified in detail.
- Security scan of credential handling in `konf-backend/src/auth/` — out of scope for this pass.
- The `surrealdb` crate's `Surreal<Any>` reconnect behavior on transient network failure — the `connect.rs` code has no retry logic, but the surrealdb client's own reconnect semantics were not verified.
