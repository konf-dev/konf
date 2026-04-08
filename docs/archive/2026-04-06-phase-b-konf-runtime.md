# Phase B: konf-runtime

**Goal:** Build the OS-like workflow management layer.
**Depends on:** Phase A (konflux CancellationToken, hooks, smrti pool exposure)
**Blocks:** Phase C (konf-backend uses runtime via PyO3)

**Research basis:** `docs/research/2026-04-05-runtime-architecture-survey.md`, `docs/research/2026-04-05-runtime-recommendation.md`, `docs/research/2026-04-06-real-world-experiences.md`

---

## Architecture

```
konf-runtime wraps konflux + smrti:

Runtime
├── ProcessTable (papaya HashMap<RunId, WorkflowRun>)
├── Engine (konflux, tool registry)
├── EventJournal (sqlx → Postgres)
└── default ResourceLimits
```

The runtime provides: start/wait/cancel/kill workflows, process tree, namespace-scoped capabilities, monitoring, event logging.

**Key design decisions:**
- Raw tokio (JoinSet + CancellationToken), no actor framework
- papaya for concurrent process table (lock-free, async)
- sqlx for event journal (shared pool with smrti)
- Parameterized CapabilityGrant with namespace injection
- Per-session CancellationTokens (explicitly dropped on cleanup to avoid memory leak)

---

## Crate structure

```
konflux/konf-runtime/
├── Cargo.toml
├── src/
│   ├── lib.rs          # Public API exports
│   ├── runtime.rs      # Runtime struct — main entry point
│   ├── process.rs      # WorkflowRun, RunId, RunStatus, ProcessTable
│   ├── scope.rs        # ExecutionScope, CapabilityGrant, ResourceLimits
│   ├── context.rs      # VirtualizedTool — namespace injection wrapper
│   ├── hooks.rs        # RuntimeHooks — connects executor to process table
│   ├── journal.rs      # EventJournal — append-only Postgres log
│   ├── monitor.rs      # RunSummary, RunDetail, ProcessTree, RuntimeMetrics
│   └── error.rs        # RuntimeError
└── tests/
    └── runtime_tests.rs
```

**Dependencies:**
```toml
[dependencies]
konflux = { path = "../konflux-core" }
tokio = { version = "1", features = ["rt-multi-thread", "sync", "time", "macros"] }
tokio-util = "0.7"
papaya = "0.2"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "json", "chrono", "uuid"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
tracing = { version = "0.1", features = ["attributes"] }
thiserror = "2"
async-trait = "0.1"

[dev-dependencies]
tokio = { version = "1", features = ["full", "test-util"] }
tracing-subscriber = "0.3"
```

---

## Tasks (sequential)

### B1. Error types + process types

**Files:** `error.rs`, `process.rs`

RuntimeError: NotFound, NotRunning, ResourceLimit, CapabilityDenied, Engine(KonfluxError), JoinFailed, Database(sqlx::Error)

RunId = uuid::Uuid

RunStatus: Pending, Running, Completed { output, duration_ms }, Failed { error, duration_ms }, Cancelled { reason, duration_ms }

WorkflowRun: id, parent_id, workflow_id, namespace, status, capabilities, metadata, started_at, completed_at, cancel_token, active_nodes (Mutex<Vec<ActiveNode>>), steps_executed (AtomicUsize)

ActiveNode: node_id, tool_name, started_at, status (Running/Completed/Failed/Retrying)

ProcessTable: papaya::HashMap<RunId, WorkflowRun> with methods: insert, get, remove, list(namespace), children_of(parent_id), active_count, gc(max_age)

RunSummary, RunDetail: serializable views derived from WorkflowRun (no internal state leaked)

**Tests:**
- ProcessTable CRUD
- list with namespace filter
- children_of correctness
- gc removes old completed runs
- active_count accuracy

### B2. Capability routing + context virtualization

**Files:** `scope.rs`, `context.rs`

CapabilityGrant { pattern: String, bindings: HashMap<String, Value> }
- `matches(tool_name) -> Option<&HashMap<String, Value>>`: glob matching with colon separator

ExecutionScope { namespace, capabilities: Vec<CapabilityGrant>, limits: ResourceLimits }
- `check_tool(tool_name) -> Result<HashMap<String, Value>, RuntimeError>`: find matching grant, return bindings
- `child_scope(grants) -> Result<ExecutionScope, RuntimeError>`: validate subset, create child
- `validate_start(table) -> Result<(), RuntimeError>`: check active runs limit

ResourceLimits { max_steps, max_workflow_timeout_ms, max_concurrent_nodes, max_child_depth, max_active_runs_per_namespace } with sensible defaults

VirtualizedTool: wraps Arc<dyn Tool>, injects bindings into input before invocation. Overrides any LLM-set values for bound keys (prevents namespace escape).

**Tests:**
- Capability matching: exact, glob, no match
- Bindings returned correctly
- child_scope validates subset
- child_scope denies escalation
- VirtualizedTool injects namespace
- VirtualizedTool overrides LLM-set namespace
- validate_start respects max_active_runs

### B3. Event journal

**Files:** `journal.rs`

EventJournal wrapping sqlx::PgPool.

Table: `runtime_events (id BIGSERIAL, run_id UUID, session_id TEXT, namespace TEXT, event_type TEXT, payload JSONB, created_at TIMESTAMPTZ DEFAULT NOW())`

Methods: new(pool) with migration, append(entry), query_by_run(run_id), query_by_session(session_id, limit)

Migration is idempotent (CREATE TABLE IF NOT EXISTS + CREATE INDEX IF NOT EXISTS).

**Tests:** (require Postgres, use testcontainers or real DB)
- Append and query by run_id
- Query by session_id with limit
- Migration idempotency (call twice, no error)

### B4. Runtime hooks

**Files:** `hooks.rs`

RuntimeHooks implements konflux::ExecutionHooks.

Updates ProcessTable active_nodes on node start/complete/fail. Appends to EventJournal. Increments steps_executed.

Each RuntimeHooks instance is scoped to a single WorkflowRun (holds run_id, table ref, journal ref).

**Tests:**
- Hooks update active_nodes correctly (add on start, remove on complete)
- Hooks append to journal
- steps_executed increments

### B5. Runtime struct

**Files:** `runtime.rs`, `lib.rs`

```rust
pub struct Runtime {
    engine: Engine,
    table: Arc<ProcessTable>,
    journal: Arc<EventJournal>,
    default_limits: ResourceLimits,
}
```

Methods:
- `new(engine, pool) -> Result<Self>`: create with defaults
- `start(workflow, input, scope) -> Result<RunId>`: validate scope, create run, wrap tools with virtualizer, spawn execution task
- `wait(run_id) -> Result<Value>`: await completion
- `run(workflow, input, scope) -> Result<Value>`: start + wait
- `start_streaming(workflow, input, scope) -> Result<(RunId, StreamReceiver)>`: start with streaming
- `cancel(run_id, reason) -> Result<()>`: graceful cancel (propagates to children)
- `kill(run_id) -> Result<()>`: abort task immediately
- `list_runs(namespace) -> Vec<RunSummary>`
- `get_run(run_id) -> Option<RunDetail>`
- `get_tree(run_id) -> Option<ProcessTree>`
- `metrics() -> RuntimeMetrics`
- `gc(max_age)`: cleanup old runs

**Tests:**
- test_start_and_wait: basic execution
- test_start_streaming: verify stream events received
- test_cancel_running: cancel mid-execution → Cancelled status
- test_kill_running: abort → immediate stop
- test_namespace_isolation: different namespaces don't see each other's runs
- test_resource_limit_max_runs: exceed limit → ResourceLimit error
- test_capability_routing: bound namespace injected correctly
- test_capability_denial: tool not in scope → denied
- test_process_tree: parent → child → grandchild tree correct
- test_metrics: counts accurate during and after execution
- test_gc: old completed runs removed
- test_concurrent_runs: 10 parallel workflows, no race conditions
- test_journal_records: events appear in journal after workflow

### B6. Monitoring types

**Files:** `monitor.rs`

RunSummary, RunDetail, ProcessTree, RuntimeMetrics — serializable structs. These are views, not storage.

ProcessTree is recursive: `{ run: RunSummary, children: Vec<ProcessTree>, active_nodes: Vec<ActiveNode> }`

RuntimeMetrics: active_runs, total_completed, total_failed, total_cancelled, uptime_seconds

### B7. Python bindings

**Files:** `konflux-python/src/lib.rs` (update), `konflux-python/Cargo.toml` (add konf-runtime dep)

Add PyRuntime class:
- `new(dsn)` → connect to Postgres, create engine + runtime
- `register_tool(name, callable, info)` → register into engine
- `parse_yaml(yaml)` → parse workflow
- `start(workflow, input, scope)` → returns run_id string (async)
- `wait(run_id)` → returns result (async)
- `run(workflow, input, scope)` → start + wait (async)
- `start_streaming(...)` → returns (run_id, StreamIterator)
- `cancel(run_id, reason)`, `kill(run_id)`
- `list_runs(namespace)`, `get_run(run_id)`, `get_tree(run_id)`, `metrics()`

Scope passed as Python dict:
```python
scope = {
    "namespace": "user:123",
    "capabilities": [
        {"pattern": "memory:*", "bindings": {"namespace": "user:123"}},
        {"pattern": "ai:complete"},
    ],
    "limits": {"max_steps": 500, "max_workflow_timeout_ms": 60000},
}
```

**Tests (pytest):**
- test_runtime_basic: create, register tool, run workflow
- test_runtime_cancel: start + cancel
- test_runtime_list: verify list_runs returns data

---

## Verification

```bash
cd konflux
cargo test -p konf-runtime
cargo clippy -p konf-runtime -- -D warnings
cargo test --workspace
cargo clippy --workspace -- -D warnings

# Python
cd konflux-python && maturin develop && pytest
```
