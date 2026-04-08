# Detailed Implementation Plan

**Status:** Completed (all phases executed, monorepo consolidated at konf-dev/konf)
**Architecture:** [overview.md](../architecture/overview.md)
**Roadmap:** [master-plan.md](master-plan.md)

No backward compatibility. Old code/structure is deleted, not preserved.

### Production-grade requirements (apply to ALL new code)

- **Tracing:** Every `Tool::invoke()` must have a `tracing::instrument` span with `run_id`, `node_id`, `workflow_id` from ToolContext
- **Error handling:** `thiserror` for library crates (konflux, runtime, tools), `anyhow` for application crates (backend, mcp, init)
- **Docs:** `#![warn(missing_docs)]` on all new crates. Every public trait/struct has a doc comment.
- **`llms.txt`:** Generate for each new crate (architecture summary for AI-assisted development)
- **Version:** Expose crate versions in `/v1/health` and MCP `initialize` response via `env!("CARGO_PKG_VERSION")`

---

## Current State (inventory)

| Crate | Files | LOC | Tests | Status |
|-------|-------|-----|-------|--------|
| konflux-core | 18 | ~2,358 | 49 | Stable, keep |
| konf-runtime | 9 | ~1,584 | 39 | Stable, edit |
| konf-backend | 17 | ~1,954 | 16 | Rewrite |
| smrti-core | 7 | ~1,852 | 40 | Stable, wrap |
| **Total** | **51** | **~7,748** | **144** | |

New crates to create: **8** (konf-init, konf-mcp, konf-tool-http, konf-tool-llm, konf-tool-embed, konf-tool-memory, konf-tool-memory-smrti, konf-tool-mcp)

---

## Step 0: Clean up Python legacy

**Delete** the existing Python projects that conflict with new Rust crates:
- `konf-tools/` (Python) — replaced by Rust konf-tools workspace
- `konf-gateway/` (Python) — replaced by konf-mcp + konf-backend

These are Python-era remnants. No code from them is reused.

---

## Phase C: Engine Foundation

### C1: Enrich ToolInfo + ToolAnnotations + ToolRegistry.remove()

**What changes:**

`konflux/konflux-core/src/tool.rs` — add fields to ToolInfo:
```rust
// ADD these fields to existing ToolInfo struct:
pub output_schema: Option<Value>,
pub annotations: ToolAnnotations,

// ADD new struct:
pub struct ToolAnnotations {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}
impl Default for ToolAnnotations { /* all false */ }
```

`konflux/konflux-core/src/builtin.rs` — update all 5 builtins:
- echo: `annotations: ToolAnnotations { read_only: true, idempotent: true, ..default }`
- json_get: same
- concat: same
- log: same
- template: same
- All builtins: add `output_schema: None` (can refine later)

`konflux/konflux-core/src/engine.rs` — no structural changes, but `registry_snapshot()` now copies the richer ToolInfo.

**Every existing Tool impl across all crates** must add the two new fields. Affected:
- `konf-backend/src/tools/http.rs` (HttpGetTool, HttpPostTool) — `open_world: true, idempotent: true` for GET
- `konf-backend/src/tools/llm.rs` (AiCompleteTool) — `open_world: true`
- `konf-backend/src/tools/embed.rs` (EmbedTool) — `read_only: true, idempotent: true`
- `konf-backend/src/tools/memory.rs` (4 tools) — search: `read_only, idempotent`; store: none; state_set: `idempotent`; state_get: `read_only, idempotent`
- `konf-backend/src/tools/mcp.rs` (McpToolWrapper) — map from MCP annotations

Add `remove()` to ToolRegistry (needed for hot-reload tool toggling):
```rust
impl ToolRegistry {
    pub fn remove(&mut self, name: &str) -> bool;
}
```

**Tests to add:**
- `konflux/konflux-core/src/tool.rs` — test ToolAnnotations Default, test ToolInfo serialization with new fields, test ToolRegistry remove()
- Verify all existing tests still pass (no breaking changes — new fields have defaults)

**Estimated:** ~60 lines new, ~80 lines edited across 8 files

---

### C2: Resource and Prompt traits + registries

**New files:**

`konflux/konflux-core/src/resource.rs`:
```rust
pub trait Resource: Send + Sync { fn info() -> ResourceInfo; async fn read() -> Result<Value>; fn subscribe() -> Option<Receiver<ResourceChanged>>; }
pub struct ResourceInfo { uri, name, description, mime_type }
pub struct ResourceChanged { uri }
pub struct ResourceRegistry { /* HashMap<String, Arc<dyn Resource>> */ }
```
~60 lines

`konflux/konflux-core/src/prompt.rs`:
```rust
pub trait Prompt: Send + Sync { fn info() -> PromptInfo; async fn expand(args) -> Result<Vec<Message>>; }
pub struct PromptInfo { name, description, arguments: Vec<PromptArgument> }
pub struct PromptArgument { name, description, required }
pub struct Message { role, content }
pub struct PromptRegistry { /* HashMap<String, Arc<dyn Prompt>> */ }
```
~70 lines

**Edit existing:**

`konflux/konflux-core/src/engine.rs` — add two new registries:
```rust
pub struct Engine {
    tools: Arc<RwLock<ToolRegistry>>,
    resources: Arc<RwLock<ResourceRegistry>>,   // ADD
    prompts: Arc<RwLock<PromptRegistry>>,       // ADD
    config: EngineConfig,
}
// ADD: register_resource(), resources(), register_prompt(), prompts()
```
~30 lines added

`konflux/konflux-core/src/lib.rs` — add `pub mod resource; pub mod prompt;`

**Tests:**
- `resource.rs` — test register/read/list resources
- `prompt.rs` — test register/expand/list prompts
- `engine.rs` — test engine has all three registries

**Estimated:** ~160 lines new, ~40 lines edited

---

### C3: Extract tools from konf-backend to konf-tools

**Create workspace:**

`konf-tools/Cargo.toml`:
```toml
[workspace]
members = ["konf-tool-http", "konf-tool-llm", "konf-tool-embed", "konf-tool-mcp", "konf-tool-memory", "konf-tool-memory-smrti"]
```

**For each tool crate:**

#### konf-tool-http
- **Create** `konf-tools/konf-tool-http/Cargo.toml` — deps: konflux, reqwest, async-trait, serde_json, tracing
- **Move** `konf-backend/src/tools/http.rs` → `konf-tools/konf-tool-http/src/lib.rs`
- **Add** `pub async fn register(engine: &Engine, config: &Value) -> anyhow::Result<()>`
- **Delete** `konf-backend/src/tools/http.rs`
- **Tests:** move/create test for http_get and http_post tool info + error cases

#### konf-tool-llm
- **Create** `konf-tools/konf-tool-llm/Cargo.toml` — deps: konflux, rig-core, async-trait, serde_json, serde, tracing
- **Move** `konf-backend/src/tools/llm.rs` → `konf-tools/konf-tool-llm/src/lib.rs`
- **Add** `pub async fn register(engine: &Engine, config: &Value) -> anyhow::Result<()>`
- **Delete** `konf-backend/src/tools/llm.rs`
- **Keep** LlmConfig, KonfluxToolBridge — they move with the file
- **Tests:** existing llm tests move to new crate

#### konf-tool-embed
- **Create** `konf-tools/konf-tool-embed/Cargo.toml` — deps: konflux, fastembed, async-trait, serde_json, tracing
- **Move** `konf-backend/src/tools/embed.rs` → `konf-tools/konf-tool-embed/src/lib.rs`
- **Add** `pub async fn register(engine: &Engine, config: &Value) -> anyhow::Result<()>`
- **Delete** `konf-backend/src/tools/embed.rs`

#### konf-tool-mcp
- **Create** `konf-tools/konf-tool-mcp/Cargo.toml` — deps: konflux, rmcp, konf-runtime (for CapabilityGrant), async-trait, serde_json, serde, tracing, tokio
- **Move** `konf-backend/src/tools/mcp.rs` → `konf-tools/konf-tool-mcp/src/lib.rs`
- **Add** `pub async fn register(engine: &Engine, config: &Value) -> anyhow::Result<()>`
- **Edit** McpToolWrapper: preserve MCP annotations → ToolAnnotations mapping
- **Delete** `konf-backend/src/tools/mcp.rs`
- **Tests:** existing mcp tests move to new crate

**After extraction, DELETE from konf-backend:**
- `src/tools/http.rs`
- `src/tools/llm.rs`
- `src/tools/embed.rs`
- `src/tools/mcp.rs`
- `src/tools/memory.rs`
- `src/tools/registry.rs`
- `src/tools/mod.rs`

konf-backend's `src/tools/` directory is completely removed.

---

### C4: MemoryBackend trait + smrti wrapper

#### konf-tool-memory (trait + tool shells)

**Create** `konf-tools/konf-tool-memory/Cargo.toml` — deps: konflux, async-trait, serde_json, serde, tracing, thiserror

**Create** `konf-tools/konf-tool-memory/src/lib.rs`:
- `MemoryBackend` trait (search, add_nodes, state_set/get/delete/list/clear, supported_search_modes)
- `MemoryBackendExt` trait (traverse, aggregate, update_node, retract_node, add_edges, retract_edge, merge_nodes)
- `SearchParams` struct
- `MemoryError` enum
- `register(engine, backend: Arc<dyn MemoryBackend>) -> Result<()>` — registers all 7 tools

**Create** `konf-tools/konf-tool-memory/src/tools.rs`:
- `SearchTool { backend: Arc<dyn MemoryBackend> }` — implements Tool
- `StoreTool`, `StateSetTool`, `StateGetTool`, `StateDeleteTool`, `StateListTool`, `StateClearTool`
- Each delegates to `self.backend.*()` method
- SearchTool dynamically builds input_schema based on `backend.supported_search_modes()`

**Tests:** test tool registration, test tool info, test delegation to mock backend

~300 lines new

#### konf-tool-memory-smrti (wrapper)

**Create** `konf-tools/konf-tool-memory-smrti/Cargo.toml` — deps: konf-tool-memory, smrti-core, serde_json, anyhow

**Create** `konf-tools/konf-tool-memory-smrti/src/lib.rs`:
- `SmrtiBackend { memory: Arc<Memory> }` — implements MemoryBackend
- `pub async fn connect(config: &Value) -> Result<Arc<dyn MemoryBackend>>` — creates SmrtiConfig, calls Memory::connect(), wraps
- Each trait method delegates to `self.memory.*()` with parameter mapping

**Tests:** test connect with mock config, test trait method delegation

~150 lines new

---

### C5: WorkflowTool

**Create** `konf-runtime/src/workflow_tool.rs`:
- `WorkflowTool { workflow, runtime: Arc<Runtime>, default_scope }` — implements Tool
- `info()` returns ToolInfo from workflow YAML header (name, description, input_schema, capabilities)
- `invoke()` creates child scope via `default_scope.child_scope()`, runs workflow via `self.runtime.run()`
- Name: `workflow_{workflow.id}`

**Edit** `konf-runtime/src/lib.rs` — add `pub mod workflow_tool;`

**Tests:** test WorkflowTool info generation, test invocation creates child scope, test capability attenuation

~80 lines new

---

### C6: konf-init

**Create** `konf-init/Cargo.toml`:
```toml
[dependencies]
konflux = { ... }
konf-runtime = { ... }
konf-tool-http = { path = "../konf-tools/konf-tool-http" }
konf-tool-llm = { path = "../konf-tools/konf-tool-llm" }
konf-tool-embed = { path = "../konf-tools/konf-tool-embed" }
konf-tool-mcp = { path = "../konf-tools/konf-tool-mcp" }
konf-tool-memory = { path = "../konf-tools/konf-tool-memory" }
konf-tool-memory-smrti = { path = "../konf-tools/konf-tool-memory-smrti", optional = true }
figment = { version = "0.10", features = ["toml", "env", "yaml"] }
arc-swap = "1"
notify = "7"  # file system watcher for config hot-reload
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
anyhow = "1"

[features]
default = ["memory-smrti"]  # Use memory-surrealdb once E1 is complete
memory-smrti = ["konf-tool-memory-smrti"]
memory-surrealdb = ["konf-tool-memory-surrealdb"]
memory-sqlite = ["konf-tool-memory-sqlite"]
```

**Create** `konf-init/src/lib.rs`:
- `KonfInstance { engine: Arc<Engine>, runtime: Arc<Runtime>, config: Arc<PlatformConfig>, product_config: Arc<ArcSwap<ProductConfig>> }`
- `pub async fn boot(config_path: &Path) -> anyhow::Result<KonfInstance>` — full 12-step boot sequence
- `impl KonfInstance { pub async fn reload(&self) -> Result<(), Vec<String>> }` — hot-reload

**Create** `konf-init/src/config.rs`:
- **Move** `PlatformConfig` and sub-configs from `konf-backend/src/config.rs`
- Add `ProductConfig { tools, workflows, prompts }` struct
- Add `ToolsConfig { memory, llm, http, embed, mcp_servers }` struct

**Delete** `konf-backend/src/config.rs` — replaced by konf-init config

**Tests:** test boot with minimal config, test boot with all tools, test reload

~400 lines new

---

## Phase D: Transport Shells

### D1: konf-mcp (MCP server)

**Create** `konf-mcp/Cargo.toml`:
```toml
[dependencies]
konf-init = { path = "../konf-init" }
konf-runtime = { ... }
konflux = { ... }
rmcp = { version = "1.3", features = ["server", "transport-stdio", "transport-sse"] }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
anyhow = "1"
```

**Create** `konf-mcp/src/lib.rs`:
- `KonfMcpServer { engine, runtime }` — implements rmcp server traits
- `tools/list` → reads ToolRegistry, maps ToolInfo → MCP format
- `tools/call` → looks up tool, builds ToolContext, calls invoke()
- `resources/list` → reads ResourceRegistry
- `resources/read` → calls resource.read()
- `prompts/list` → reads PromptRegistry
- `prompts/get` → calls prompt.expand()
- Annotation mapping: read_only → readOnlyHint, etc.

**Create** `konf-mcp/src/main.rs`:
- `konf_init::boot()` → `KonfMcpServer::new()` → `serve_stdio()` or `serve_sse()`
- CLI args: `--stdio`, `--sse --port 3001`, `--config ./config`

**Create** `konf-mcp/src/transport.rs`:
- `serve_stdio()` — rmcp stdio transport
- `serve_sse(listener)` — rmcp SSE transport
- `sse_handler() -> Router` — axum handler for mounting in konf-backend

**Tests:** test tools/list returns all registered tools, test tools/call dispatches correctly, test annotation mapping

~400 lines new

---

### D2: konf-backend rewrite

**Rewrite** `konf-backend/src/main.rs` (~80 lines, down from ~180):
```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();
    let instance = konf_init::boot(Path::new("./config")).await?;
    let verifier = Arc::new(JwtVerifier::new(&instance.config.auth));

    // Scheduling (only if DB)
    if let Some(ref db) = instance.config.database {
        let scheduler = Scheduler::new(db, instance.runtime.clone());
        scheduler.migrate().await?;
        scheduler.start_polling(10);
    }

    // MCP (optional)
    let mcp = if instance.config.mcp_enabled {
        Some(KonfMcpServer::new(instance.engine.clone(), instance.runtime.clone()))
    } else { None };

    let app = build_router(&instance, verifier, mcp);
    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}
```

**Rewrite** `konf-backend/Cargo.toml`:
- **Remove** deps: smrti-core, rig-core, fastembed, rmcp (client features)
- **Add** deps: konf-init, konf-mcp (optional)
- **Keep** deps: axum, tower-http, tokio, sqlx (for scheduler), jsonwebtoken, reqwest (for JWKS), serde, tracing

**Delete** entire `konf-backend/src/tools/` directory (7 files)
**Delete** `konf-backend/src/config.rs` (moved to konf-init)

**Keep unchanged:**
- `src/auth/` (jwt.rs, middleware.rs, mod.rs) — 272 lines
- `src/api/health.rs`, `src/api/me.rs` — 26 lines
- `src/api/monitor.rs` — 74 lines (already fixed in audit)
- `src/error.rs` — 52 lines (already fixed in audit)
- `src/scheduling/mod.rs` — 232 lines
- `src/templates/mod.rs` — 54 lines

**Edit:**
- `src/api/chat.rs` — rewrite for proper SSE streaming (Phase D3)
- `src/api/mod.rs` — remove tools route if any

---

### D3: SSE Streaming

**Edit** `konf-runtime/src/runtime.rs`:
- Implement `start_streaming()` method:
  ```rust
  pub async fn start_streaming(&self, workflow, input, scope, session_id)
      -> Result<(RunId, StreamReceiver), RuntimeError>
  ```
- Uses `engine.run_streaming()` (already exists in konflux) instead of `engine.run()`
- Returns the stream receiver to the caller

**Rewrite** `konf-backend/src/api/chat.rs`:
- Replace 100ms poll loop with proper streaming:
  ```rust
  let (run_id, mut rx) = state.runtime.start_streaming(&workflow, input, scope, session_id).await?;
  let stream = async_stream::stream! {
      yield Ok(Event::default().event("start").data(json!({"run_id": run_id}).to_string()));
      while let Some(event) = rx.recv().await {
          match event {
              StreamEvent::Progress { event_type, data, .. } => { yield Ok(sse_event(event_type, data)); }
              StreamEvent::Done { output } => { yield Ok(Event::default().event("done").data(...)); break; }
              StreamEvent::Error { message, .. } => { yield Ok(Event::default().event("error").data(...)); break; }
          }
      }
  };
  Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
  ```

**Tests:** integration test sending chat request and receiving SSE events

---

### D4: Admin + Monitoring API

**Create** `konf-backend/src/api/admin.rs`:
- `GET /v1/admin/config` — read product config from KonfInstance
- `PUT /v1/admin/config` — trigger `instance.reload()`, return validation result
- `GET /v1/admin/audit` — query EventJournal (if available)

**Create** `konf-backend/src/api/messages.rs`:
- `GET /v1/messages?session_id=X` — query conversation history from memory backend

**Edit** `konf-backend/src/api/mod.rs` — add admin and messages routes

~200 lines new

---

## Phase G: CI/CD, Docker, Tests (moved up — should be done alongside C/D)

### G1: GitHub Actions CI

**Create** `.github/workflows/ci.yml`:
```yaml
name: CI
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { submodules: recursive }
      - uses: dtolnay/rust-toolchain@stable
        with: { components: clippy, rustfmt }
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --workspace
      - run: cargo build --release
```

**Create** `.github/workflows/docker.yml` — build and push Docker image on tag

### G2: Dockerfile

**Create** `Dockerfile`:
```dockerfile
FROM rust:1.83-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin konf-backend

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/konf-backend /usr/local/bin/
EXPOSE 8000
CMD ["konf-backend"]
```

**Create** `docker-compose.yml`:
```yaml
services:
  backend:
    build: .
    ports: ["8000:8000"]
    environment:
      KONF__DATABASE__URL: postgresql://postgres:pass@postgres/konf
      KONF__AUTH__SUPABASE_URL: http://supabase-auth:9999
    volumes: ["./config:/config"]
    depends_on: [postgres]
  postgres:
    image: pgvector/pgvector:pg17
    environment: { POSTGRES_PASSWORD: pass, POSTGRES_DB: konf }
    volumes: [pgdata:/var/lib/postgresql/data]
volumes:
  pgdata:
```

### G3: Test Coverage Requirements

**Every crate must have tests:**

| Crate | Required tests |
|-------|---------------|
| konflux-core | Existing 49 tests + new ToolAnnotations/Resource/Prompt tests |
| konf-runtime | Existing 39 tests + WorkflowTool tests + start_streaming tests |
| konf-init | Boot sequence test (minimal config), boot with all tools, reload test |
| konf-tool-http | Tool info, invoke with mock server, timeout, error handling |
| konf-tool-llm | Config deserialization, tool info, mock LLM call |
| konf-tool-embed | Tool info, embed text (requires fastembed model, can be ignored in CI) |
| konf-tool-memory | Register all 7 tools with mock backend, test delegation |
| konf-tool-memory-smrti | Connect, search delegation, state delegation (requires Postgres) |
| konf-tool-mcp | Config deserialization, env var resolution, tool wrapping |
| konf-mcp | tools/list, tools/call, annotation mapping |
| konf-backend | Health endpoint, auth middleware, chat SSE (integration) |

Tests requiring external services (Postgres, MCP servers) are marked `#[ignore]` and run separately in CI with `docker compose`.

---

## Execution Order

```
Step  What                                    Deps    Delete/Create
────  ──────────────────────────────────────  ──────  ──────────────
 0    Delete Python konf-tools/ + konf-gateway/ none   DELETE dirs
 1    C1: ToolInfo + ToolAnnotations + remove() none   EDIT konflux
 2    C2: Resource + Prompt traits              none   ADD to konflux
 3    C3: Create konf-tools workspace + root    1      CREATE workspace + root Cargo.toml
 4    C3: Move http.rs → konf-tool-http         3      DELETE from backend
 5    C3: Move llm.rs → konf-tool-llm           3      DELETE from backend
 6    C3: Move embed.rs → konf-tool-embed       3      DELETE from backend
 7    C3: Move mcp.rs → konf-tool-mcp           3      DELETE from backend
 8    C4: Create konf-tool-memory (trait)        1      CREATE crate
 9    C4: Create konf-tool-memory-smrti          8      DELETE memory.rs from backend
10    C5: WorkflowTool                           1      ADD to konf-runtime
11    C6: Create konf-init                       3-9    CREATE crate, DELETE backend/config.rs
12    D2: Rewrite konf-backend main.rs           11     DELETE src/tools/ dir, registry.rs, mod.rs
13    D3: SSE streaming                          12     REWRITE chat.rs
14    D1: Create konf-mcp                        11     CREATE crate
15    D4: Admin + messages API                   12     ADD to backend
16    G1: GitHub Actions CI                      12     CREATE .github/workflows/
17    G2: Dockerfile + docker-compose            12     CREATE Dockerfile, docker-compose.yml
```

Steps 1 and 2 are independent (can run in parallel).
Steps 4-7 are independent (can run in parallel after 3).
Steps 13-15 are independent (can run in parallel after 12).
Steps 16-17 are independent (can run in parallel after 12).

After step 12, konf-backend has zero tool code. After step 17, the platform is deployable.

---

## What gets deleted (complete list)

| File | Why |
|------|-----|
| `konf-backend/src/tools/http.rs` | Moved to konf-tool-http |
| `konf-backend/src/tools/llm.rs` | Moved to konf-tool-llm |
| `konf-backend/src/tools/embed.rs` | Moved to konf-tool-embed |
| `konf-backend/src/tools/mcp.rs` | Moved to konf-tool-mcp |
| `konf-backend/src/tools/memory.rs` | Replaced by konf-tool-memory + konf-tool-memory-smrti |
| `konf-backend/src/tools/registry.rs` | Replaced by konf-init boot sequence |
| `konf-backend/src/tools/mod.rs` | No more tools directory |
| `konf-backend/src/config.rs` | Moved to konf-init |
| `konf-tools/` (Python) | Replaced by Rust konf-tools workspace |
| `konf-gateway/` (Python) | Replaced by konf-mcp + konf-backend |

## Explicitly deferred (not in this plan)

| Item | Why deferred | When |
|------|-------------|------|
| `konf-tool-python` | Python tools are opt-in, no current users. Create when needed. | Phase F or later |
| Admin endpoints from multi-tenancy spec (`/v1/admin/products`, `/v1/admin/users`) | Admin console is post-MVP | Phase G |
| Config version hash (SHA-256) | Nice-to-have for auditability, not blocking | Phase G |
| OpenAPI docs via utoipa | Documentation, not blocking | Phase G |
| Root workspace Cargo.toml | Create at step 3 (C3) to tie all crates together for `cargo test --workspace` |

---

## What stays unchanged

| Crate/File | Why |
|-----------|-----|
| `konflux/` (all 18 files) | Engine is stable, only additive changes (C1, C2) |
| `smrti/` (all 7 files) | Wrapped by konf-tool-memory-smrti, not modified |
| `konf-runtime/` (8 of 9 files) | Only `runtime.rs` gets `start_streaming()`, `workflow_tool.rs` is new |
| `konf-backend/src/auth/` | Auth middleware unchanged |
| `konf-backend/src/api/health.rs` | Unchanged |
| `konf-backend/src/api/me.rs` | Unchanged |
| `konf-backend/src/api/monitor.rs` | Already fixed in audit |
| `konf-backend/src/error.rs` | Already fixed in audit |
| `konf-backend/src/scheduling/` | Stays in backend (server-only concern) |
| `konf-backend/src/templates/` | Stays (workflow templates) |
