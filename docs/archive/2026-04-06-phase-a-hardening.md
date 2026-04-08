# Phase A: Harden Existing Crates

**Goal:** Prepare smrti and konflux for konf-runtime integration.
**Depends on:** Nothing (can start immediately)
**Blocks:** Phase B (konf-runtime), Phase C (konf-tools needs smrti PyO3)

---

## A1. konflux-core: CancellationToken

**Files:** `konflux-core/src/executor.rs`, `konflux-core/src/error.rs`, `konflux-core/Cargo.toml`

Add `tokio-util = "0.7"` to dependencies.

Add to `ExecutionError`:
```rust
#[error("[{workflow_id}] workflow cancelled")]
Cancelled { workflow_id: String },
```

Add `cancel_token: CancellationToken` to `Executor`. Check in:
- Main event loop (`tokio::select!` — add `_ = cancel_token.cancelled()` branch)
- Top of `spawn_node` inner loop (before each step)
- Between retry attempts in `invoke_with_retry`

Update `Engine::run` and `Engine::run_streaming` signatures:
```rust
pub async fn run(
    &self,
    workflow: &Workflow,
    input: Value,
    granted_capabilities: &[String],
    execution_metadata: HashMap<String, Value>,
    cancel_token: Option<CancellationToken>,
) -> Result<Value, KonfluxError>
```

Default to `CancellationToken::new()` (never cancelled) if None.

**Tests:**
- `test_cancellation`: slow tool (500ms), cancel after 100ms → `Cancelled`
- `test_cancellation_propagates`: parent token cancels → spawned child tasks stop

---

## A2. konflux-core: ExecutionHooks

**Files:** `konflux-core/src/hooks.rs` (new), `konflux-core/src/executor.rs`, `konflux-core/src/lib.rs`

```rust
pub trait ExecutionHooks: Send + Sync {
    fn on_node_start(&self, node_id: &str, tool: &str) {}
    fn on_node_complete(&self, node_id: &str, tool: &str, duration_ms: u64, output: &serde_json::Value) {}
    fn on_node_failed(&self, node_id: &str, tool: &str, error: &str) {}
    fn on_tool_retry(&self, node_id: &str, tool: &str, attempt: u32, error: &str) {}
}

pub struct NoopHooks;
impl ExecutionHooks for NoopHooks {}
```

Pass `Arc<dyn ExecutionHooks>` through `StepContext`. Call at appropriate points in executor.

Add to `Engine::run` signature as optional parameter (default NoopHooks).

**Tests:**
- `test_hooks_receive_events`: mock hooks struct, run workflow, assert events received in order

---

## A3. konflux-core: Global workflow timeout

**Files:** `konflux-core/src/engine.rs`

Add to `EngineConfig`:
```rust
pub max_workflow_timeout_ms: u64,  // default 300_000 (5 min), 0 = no limit
```

In `Engine::run`, wrap execution:
```rust
if self.config.max_workflow_timeout_ms > 0 {
    match timeout(Duration::from_millis(self.config.max_workflow_timeout_ms), exec).await {
        Ok(result) => result,
        Err(_) => Err(KonfluxError::Execution(ExecutionError::Timeout { ... })),
    }
}
```

**Tests:**
- `test_global_timeout`: 2s tool, 500ms global timeout → timeout error

---

## A4. konflux-core: Thread-safe tool registration

**Files:** `konflux-core/src/engine.rs`, `konflux-core/src/tool.rs`

Replace `Arc<ToolRegistry>` with `Arc<std::sync::RwLock<ToolRegistry>>`.

```rust
pub fn register_tool(&self, tool: Arc<dyn Tool>) {
    self.registry.write().unwrap().register(tool);
}
```

Update all `.get()` calls to acquire read lock. This is a mechanical refactor.

**Tests:**
- `test_register_after_clone`: clone engine, register on clone, both see tool
- `test_concurrent_registration`: 10 tasks registering simultaneously

---

## A5. konflux-core: Config exposure + YAML limits

**Files:** `konflux-core/src/engine.rs`, `konflux-core/src/parser.rs`

Add to Engine: `pub fn config(&self) -> &EngineConfig`

Add to EngineConfig:
```rust
pub max_yaml_size: usize,           // default 10_485_760 (10MB)
pub finished_channel_size: usize,   // default 100
pub default_retry_backoff_ms: u64,  // default 250
```

Add size check to `parse()` or `Engine::parse_yaml()`.

Update executor to read channel size and backoff from config instead of hardcoded values.

**Tests:**
- `test_yaml_size_limit`: 11MB YAML → error
- `test_config_accessible`: engine.config() returns correct values

---

## A6. smrti: Runtime integration prep

**Files:** `smrti-core/src/provider/postgres.rs`, `smrti-core/src/memory.rs`

1. Add `pub fn pool(&self) -> &sqlx::PgPool` to PostgresProvider (or Memory)
2. Add concurrent stress tests:
   - `test_concurrent_get_or_create`: 20 tokio tasks, same node_key → exactly 1 node created
   - `test_concurrent_state_set`: 20 tasks writing different keys in same session → all succeed
   - `test_concurrent_search_during_write`: search while add_nodes is running → no errors

**Tests:** 3 new concurrent tests.

---

## A7. CI/CD for both crates

**Files:** 
- `konflux/.github/workflows/ci.yml`
- `smrti/.github/workflows/ci.yml` (may exist, verify/update)

```yaml
name: CI
on: [push, pull_request]
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo fmt --check
      - run: cargo clippy --workspace -- -D warnings
  
  test:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: pgvector/pgvector:pg17
        ports: ['5432:5432']
        env:
          POSTGRES_PASSWORD: test
          POSTGRES_DB: test
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --workspace
        env:
          DATABASE_URL: postgresql://postgres:test@localhost/test
  
  python:  # konflux only
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/setup-python@v5
        with: { python-version: '3.11' }
      - run: pip install maturin pytest pytest-asyncio
      - run: cd konflux-python && maturin develop && pytest
```

---

## A8. Cargo.toml + packaging completeness

Add to both crates' Cargo.toml:
```toml
repository = "https://github.com/konf-dev/..."
homepage = "https://konf.dev"
keywords = ["ai", "agents", "workflow"]
readme = "README.md"
```

Add to `konflux-python/pyproject.toml`:
```toml
requires-python = ">=3.11"
readme = "README.md"
```

---

## Verification

```bash
# konflux
cd konflux
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
grep -r 'allow(clippy' konflux-core/src/  # must be empty
# New tests pass:
cargo test -- test_cancellation test_global_timeout test_hooks test_register_after test_concurrent_registration test_yaml_size

# smrti
cd smrti
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
# New tests pass:
cargo test -- test_concurrent_get_or_create test_concurrent_state_set test_concurrent_search
```
