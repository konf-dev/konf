# Konf v2 — Local-first, Durable, Unified Runtime

**Status**: Planning complete, implementation pending Phase 0 spike.
**Scope**: This plan covers Phase 0 through Phase 6. Additional phases (approval flow, FS tools, multi-tenant hardening, production MCP auth, hard storage isolation) are listed under [Future Work](#future-work) and explicitly out of scope for v2.
**Backward compatibility**: Explicitly discarded. Breaking changes are enumerated per phase.

---

## 1. What this plan is and is not

### Goals

1. **Remove Postgres as a hard dependency** for local deployments. Konf v2 runs on a single binary + a single redb file. No external database, no docker-compose, no setup friction.
2. **Fix the split-brain problem**: TUI (HTTP) and MCP clients (Claude Code, Gemini CLI) should see the same running workflows, the same memory, the same journal, by sharing one `Arc<Runtime>`.
3. **Make scheduled jobs and `runner:spawn` invocations durable across restarts** without violating konf's rejection of checkpoint-and-replay durability.
4. **Give the TUI a live event stream** so observability doesn't require polling.
5. **Delete dead code** and tighten the architecture.

### Non-goals

1. **Multi-node konf-backend / horizontal scaling.** Single-node by design. `FOR UPDATE SKIP LOCKED`-style multi-worker scheduler safety is over-engineering for the deployment model.
2. **Checkpoint-and-replay durable execution.** Explicitly rejected by `docs/architecture/runtime.md:242-254`. AI workflows are non-deterministic; replaying from a mid-workflow checkpoint produces different results. We do not pretend otherwise.
3. **Production remote MCP auth.** The `/mcp` endpoint added in Phase 4 is dev-mode-only. Multi-tenant MCP hosting is future work (noted in §[Future Work](#future-work)).
4. **Postgres compatibility.** After this refactor, the journal is redb or nothing. The `smrti` / `memory-surreal` memory backends are unaffected — memory is separate from the journal.
5. **Tool-runner OS-level isolation** (`SystemdRunner`, `DockerRunner`). Still planned but outside v2. `InlineRunner` with durable intents gets us the durability goal.
6. **Data migration from existing Postgres journals.** Greenfield switch. Existing deployments start a fresh redb file.

---

## 2. Philosophy: what "durable" means in konf

This section is the mental model every later phase depends on. If something in a later phase feels wrong, check it against this section first.

### 2.1 Workflows are short-lived executions, not long-running processes

A workflow is a DAG that runs to completion. Typical workflows run for 1-30 seconds. Long workflows (research tasks, batch processing) run for minutes. Konf does not have a concept of a workflow that sleeps and resumes — if a workflow needs to wait for something, it either polls in a tight loop with `schedule:create` or it's re-invoked on a trigger.

**"Persistent" in konf means persistent intent to run, not persistent running state.**

Examples:

- An email watcher is not "one workflow that runs for a week". It's a cron entry that fires a 2-second workflow every 5 minutes. The cron entry is durable in redb; the workflow execution is ephemeral.
- A research task is not "a workflow paused at step 5 waiting for an API call". It's a workflow that runs to completion (possibly minutes long) and stores intermediate results in memory. If it crashes, `runner_intents` replays it from the top; the workflow reads memory to skip work it already did.

### 2.2 Durability = durable intent + idempotent retry

Konf's durability model has two parts:

1. **The fact that "workflow X should run with input Y" is persisted** to redb (as a scheduler timer, a runner intent, or a cron entry).
2. **Retry on crash re-runs the workflow from the top** with the original input. Never mid-flight resume.

Workflow authors are responsible for making workflows idempotent when they have side effects. The idiomatic pattern is to use the memory backend for idempotency keys and cursors:

```yaml
# workflows/check-inbox.yaml
workflow: check_inbox
nodes:
  load_cursor:
    do: memory:search
    with: { namespace: "inbox_cursor", key: "last_checked_at" }
  fetch:
    do: mcp:gmail:list_since
    with: { since: "{{ load_cursor.value }}" }
  store:
    do: workflow:store_new_emails
    with: { emails: "{{ fetch.results }}" }
  save_cursor:
    do: memory:store
    with: { namespace: "inbox_cursor", key: "last_checked_at", value: "{{ now }}" }
```

If the workflow crashes between `store` and `save_cursor`, replay re-fetches the same emails — but `store_new_emails` checks memory per email id and skips already-stored ones. Safe retry by construction.

### 2.3 Workflows and triggers are separate concerns

Workflows define *what* to do. Triggers define *when* to do it and *how exposed*. Four layers:

| Layer | File | Purpose |
|---|---|---|
| Capability | `workflows/*.yaml` | Pure capability; takes input, produces output, has side effects |
| Time trigger | `schedules.yaml` | Cron and interval triggers referencing workflows by id |
| Protocol trigger | `project.yaml` | MCP and HTTP entry points referencing workflows by id |
| Imperative trigger | runtime API | `runner:spawn`, `/v1/chat`, `/mcp` calls made ad-hoc |

All four paths end up at `Runtime::start(workflow, input, scope, session_id)`. Workflows do not know how they were invoked. This is what allows LLM-authored workflows to drop into `workflows/` and become usable from every transport without any wiring.

### 2.4 Config resolution: live code, snapshotted input

When a scheduled workflow fires:

- **Workflow code is resolved live from the current engine registry.** If you edit `check-inbox.yaml` and call `config:reload`, the next cron fire uses the new version. This is Option B from the design discussion and it matches the existing `konf-init/src/schedule.rs:42-44` semantics.
- **Input is snapshotted at schedule time.** The input JSON is stored in the timer entry and reused on every fire unchanged. Edits to `schedules.yaml` input fields only take effect after `config:reload` re-registers the schedule.

If you need immutability of workflow code (pinning to a known-good version), version by name: `morning_brief_v1`, `morning_brief_v2`. Schedules point at specific ids. Analogous to pinning a Docker image tag.

---

## 3. Research verified

The plan rests on three external research findings, all verified against primary sources.

### 3.1 rmcp 1.3 Streamable HTTP

**Source**: `docs.rs/rmcp/1.3` feature list; Shuttle.dev blog example (Oct 2025); MCP spec v2025-03-26.

- Feature name: `transport-streamable-http-server` exists in rmcp 1.3 today; no upgrade required.
- API: `StreamableHttpService::new(service_factory, session_manager, config)` where `service_factory` is `impl Fn() -> Result<S, Error>` and `session_manager` is `Arc<dyn SessionManager>`.
- Axum integration: `Router::new().nest_service("/mcp", service)`. No `rmcp-axum` crate needed — `StreamableHttpService` implements Tower's `Service` trait, which axum accepts natively.
- Dependencies: rmcp 1.3 pulls `hyper`, `tower-service`, `http`, `http-body`. Does **not** pull `axum` transitively; you add axum separately.
- Session model: `LocalSessionManager::default()` manages per-session handler instances. Each session gets a fresh `ServerHandler` via the factory closure.
- MCP spec calls this transport **Streamable HTTP**, not SSE. Single endpoint handles POST (client→server JSON-RPC) and GET (server→client SSE stream). Session id in `Mcp-Session-Id` header.

**Verdict**: konf-backend can mount `/mcp` directly with no rmcp upgrade and no custom adapter.

### 3.2 redb for journal + scheduler

**Source**: `docs.rs/redb/latest`; Iroh blog on tokio integration; production usage in Lighthouse and Iroh.

- redb v4.0 is post-1.0 stable. Pure Rust, only transitive C dep is `libc`.
- Data model: `TableDefinition<K, V>` with typed keys/values; range iteration via `table.range(lower..upper)` returning a double-ended iterator.
- Transactions: sync `begin_write()` / `begin_read()`, MVCC (readers don't block writer, writer doesn't block readers). Crash-safe with configurable `Durability`.
- Tokio integration: no native async. Pattern is `tokio::task::spawn_blocking` for transactions, or a dedicated writer task with mpsc for batching.
- Binary size: ~172 KiB crate.
- Alternatives dismissed: sled (stale beta, last release 2021), fjall (pre-1.0), jammdb (less idiomatic), heed (LMDB C dep).

**Verdict**: redb is the right tool for both the journal (append-heavy log) and the scheduler (time-ordered range queries).

### 3.3 Dead code discovered in konf-backend/src/scheduling/

**Source**: exhaustive grep of the workspace; read of `konf-backend/src/scheduling/mod.rs`.

- `Scheduler::schedule_at` has **zero call sites** anywhere in the workspace. Marked `#[allow(dead_code)]`.
- The module doc comment claims "Postgres-backed job queue via apalis" but `konf-backend/Cargo.toml` has no apalis dependency. The comment is simply wrong.
- `CronJobConfig` is defined with `#[allow(dead_code)]`. No code parses `schedules.yaml` anywhere — it's a stub.
- No zombie reconciliation: crashed `running` jobs hang forever in the `scheduled_jobs` table. The migrate function creates the table but nothing else in the module is reachable.

**Verdict**: Phase 2 deletes the entire module rather than porting it. This is a pure win.

---

## 4. Dead code inventory (to be deleted)

| Target | Location | Why |
|---|---|---|
| Entire scheduling module | `crates/konf-backend/src/scheduling/` | Zero callsites; misleading docs; replaced by redb scheduler in konf-runtime |
| Postgres `scheduled_jobs` table creation | `scheduling/mod.rs::migrate` | Table is never used |
| `AppError::Database` | `konf-backend/src/error.rs:24` | No sqlx in konf-backend after scheduling removal |
| `sqlx` optional dep | `konf-backend/Cargo.toml:29` | Unused |
| `scheduling` feature | `konf-backend/Cargo.toml:52` | Unused |
| Postgres `EventJournal` impl | `konf-runtime/src/journal.rs` | Replaced by `RedbJournal` |
| `RuntimeError::Database(sqlx::Error)` | `konf-runtime/src/error.rs:30` | Replaced by `Journal(JournalError)` |
| `use sqlx::PgPool` imports | `konf-runtime/src/runtime.rs:13, 60, 67` | Replaced by trait-object journal |
| `sqlx` dependency | `crates/konf-runtime/Cargo.toml:14` | No longer used |
| `postgres` feature gate | `crates/konf-init/Cargo.toml:50-51` | No longer meaningful |
| `KonfInstance.pool` field | `konf-init/src/lib.rs:43-44` | No longer exists |
| `DatabaseConfig.pool_min` / `pool_max` | `konf-init/src/config.rs:82-86` | redb has no pool concept |
| Static `LazyLock<Mutex<HashMap<u64, JoinHandle>>>` | `konf-init/src/schedule.rs:35-36` | Replaced by redb scheduler |
| `"via apalis"` comment | `konf-backend/src/scheduling/mod.rs:2` | Factually wrong; module deleted anyway |

Total LoC deleted: ~800 lines across six files. Most is in `scheduling/mod.rs` itself.

---

## 5. New dependencies

| Crate | Version | Where | Purpose |
|---|---|---|---|
| `redb` | `"4"` | workspace + konf-runtime | Embedded KV store for journal, scheduler, runner intents |
| `cron` | `"0.15"` | already in workspace at line 78, not yet imported | Cron expression parsing for scheduler |
| `postcard` | `"1"` | workspace + konf-runtime | Compact binary serialization for redb values (smaller and faster than JSON) |
| `rmcp` features | add `"transport-streamable-http-server"` | konf-mcp | Server-side Streamable HTTP transport |

Removed: `sqlx` workspace-wide (keep dep but no crate imports it).

---

## 6. Phase 0 — Spike (throwaway branch)

**Goal**: de-risk three design assumptions with throwaway code before committing to the plan. Total effort: approximately 1 day.

### 6.1 Spike 0a — `JournalStore` trait swap

On a throwaway branch, make the minimal change needed to let `Runtime::new` take `Option<Arc<dyn JournalStore>>`. Keep the existing Postgres impl as `PostgresJournal` behind the trait. Do not add redb yet. Run `cargo test --workspace` and fix whatever breaks.

**Success criterion**: existing test suite passes with the trait indirection in place and the Postgres impl unchanged.

**What this verifies**: the error-type and `Runtime::new` signature fallout is surfaced before Phase 1a starts.

### 6.2 Spike 0b — rmcp Streamable HTTP proof-of-concept

In a single-file example binary, mount a minimal `ServerHandler` at `/mcp` on a trivial axum router. Use `curl` or `@modelcontextprotocol/inspector` to verify:

1. `tools/list` returns expected tools
2. Calling a simple tool (`echo`) works end-to-end
3. `notifications/tools/list_changed` fires when `engine.notify_tools_changed()` is called on the shared engine
4. The dep tree doesn't explode — verify `cargo tree` shows no unexpected axum additions beyond what rmcp needs

**Success criterion**: a working single-file example that mounts rmcp on axum and handles one tool call from `mcp-inspector`.

**What this verifies**: rmcp 1.3 feature flag works as advertised; per-session capability scoping via `with_capabilities` works inside the factory closure.

### 6.3 Spike 0c — redb scheduler range-query proof-of-concept

In a throwaway example:

1. Open a redb file
2. Insert 1000 fake timer entries with key `(run_at_unix_ms: u64, job_id: [u8;16])`
3. Run `table.range(0..now_ms)` in a loop, measuring latency
4. Run concurrent reads during writes to verify MVCC works in practice
5. Measure `spawn_blocking` overhead for a typical 10-row poll

**Success criterion**: polling latency well below 1 second (the polling interval), with concurrent reads not blocking writes.

**What this verifies**: redb's range iteration is fast enough for scheduler polling; the `spawn_blocking` pattern doesn't have surprise latency; the table layout generalizes.

### 6.4 Spike exit criteria

All three spikes land in the same throwaway branch. If any spike reveals an unexpected wall, the plan reshapes before Phase 1 starts. The branch is then discarded; no spike code is reused in production.

---

## 7. Phase 1 — Journal on redb

### 7.1 Phase 1a — Decouple `konf-runtime` from sqlx

**Prerequisite**: `konf-runtime`'s `sqlx` dependency is currently **non-optional**. `use sqlx::PgPool` appears at module scope in `runtime.rs:13` and `journal.rs:8`. This must be decoupled before redb can be introduced.

**Changes**:

1. **New file** `crates/konf-runtime/src/journal/mod.rs` (replaces `journal.rs`):

```rust
use std::error::Error as StdError;

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("storage: {0}")]
    Storage(#[source] Box<dyn StdError + Send + Sync>),
    #[error("serialization: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("not found")]
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub run_id: RunId,
    pub session_id: String,
    pub namespace: String,
    pub event_type: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalRow {
    pub id: u64,           // was i64 — Postgres BIGSERIAL artifact, now unsigned monotonic
    pub run_id: RunId,
    pub session_id: String,
    pub namespace: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait JournalStore: Send + Sync + 'static {
    async fn append(&self, entry: JournalEntry) -> Result<u64, JournalError>;
    async fn query_by_run(&self, run_id: RunId) -> Result<Vec<JournalRow>, JournalError>;
    async fn query_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<JournalRow>, JournalError>;
    async fn recent(&self, limit: usize) -> Result<Vec<JournalRow>, JournalError>;
    async fn reconcile_zombies(&self) -> Result<u64, JournalError>;
}
```

2. **`crates/konf-runtime/src/error.rs`**:

```rust
pub enum RuntimeError {
    // ... existing variants (NotFound, NotRunning, ResourceLimit, CapabilityDenied, Engine, JoinFailed) ...

    #[error("journal: {0}")]
    Journal(#[from] JournalError),

    // REMOVED: Database(#[from] sqlx::Error)
}
```

3. **`crates/konf-runtime/src/runtime.rs`**:
   - Delete `use sqlx::PgPool;` (line 13)
   - Signature: `Runtime::new(engine: Engine, journal: Option<Arc<dyn JournalStore>>) -> Result<Self, RuntimeError>`
   - Signature: `Runtime::with_limits(engine, journal, default_limits) -> Result<Self, RuntimeError>`
   - Field: `journal: Option<Arc<dyn JournalStore>>`
   - Accessor: `pub fn journal(&self) -> Option<&dyn JournalStore>` (was `Option<&EventJournal>`)

4. **`crates/konf-runtime/src/hooks.rs`**:
   - `RuntimeHooks.journal: Option<Arc<dyn JournalStore>>` (was `Option<Arc<EventJournal>>`)

5. **`crates/konf-runtime/Cargo.toml`**:
   - Remove `sqlx = { workspace = true }` entirely

6. **`crates/konf-init/src/lib.rs`**:
   - Update step 8 (connect to database) to construct `Arc<dyn JournalStore>` — still uses the existing Postgres impl at this stage, but through the trait
   - Remove `#[cfg(feature = "postgres")]` gates from `KonfInstance` and `boot()` where they were used purely to gate `PgPool` access

7. **`crates/konf-init/Cargo.toml`**:
   - Make `sqlx` optional: `sqlx = { workspace = true, optional = true }`
   - Rename `postgres` feature to `postgres-journal` for clarity (or remove it and drop Postgres entirely in 1b — see below)

**Tests**: all existing `Runtime::new(Engine::new(), None)` callers in tests continue to compile because `None` type-infers. The handful of callers that build a real pool need a one-line update to wrap it in `Arc::new(PostgresJournal::from_pool(pool))`.

**Deliverable**: `cargo test --workspace` passes with no sqlx imports in `konf-runtime/src/**`. `cargo check -p konf-runtime --no-default-features` succeeds.

**Breaking changes introduced**:
- `Runtime::new` signature
- `Runtime::with_limits` signature
- `RuntimeError::Database` → `RuntimeError::Journal`
- `Runtime::journal()` return type
- `RuntimeHooks::journal` field type

### 7.2 Phase 1b — Add `RedbJournal`, delete Postgres impl

**Changes**:

1. Add `redb = "4"` and `postcard = "1"` to workspace `Cargo.toml`.

2. Add redb and postcard to `crates/konf-runtime/Cargo.toml`:
   ```toml
   redb = { workspace = true }
   postcard = { workspace = true }
   ```

3. **New file** `crates/konf-runtime/src/journal/redb.rs`:

```rust
pub struct RedbJournal {
    db: Arc<redb::Database>,
    next_seq: Arc<AtomicU64>,
}

// Table schema:
// events:      (u64 sequence) -> (Vec<u8> postcard-encoded JournalEntry + created_at + id)
// by_run:      (RunIdBytes, u64 sequence) -> ()    [multimap via compound key]
// by_session:  (SessionId bytes, u64 sequence) -> ()
// meta:        (&str "next_seq") -> u64
```

- `append`: one write transaction. Increments `next_seq`, inserts into `events`, updates both indices, commits.
- `query_by_run`: read transaction, `by_run.range((run_id, 0)..=(run_id, u64::MAX))`, collect sequences, fetch entries.
- `query_by_session`: same pattern.
- `recent`: `events.iter().rev().take(limit)`.
- `reconcile_zombies`: two-pass scan — find `workflow_started` run_ids with no matching terminal event, append synthetic `workflow_failed` for each.
- All sync redb calls wrapped in `tokio::task::spawn_blocking`.

**Optional optimization (deferrable)**: a dedicated writer task with mpsc for batching multiple `append` calls into one transaction. Do not build in 1b. Add only if spike 0c or production measurements show append latency is bad.

4. **Delete** the Postgres `PostgresJournal` impl. Delete the Postgres-specific SQL in `journal.rs` (move it to git history).

5. **Delete** `sqlx` from `konf-init/Cargo.toml`. Delete the `postgres-journal` feature flag. konf-init no longer knows about sqlx.

6. **New module** `crates/konf-runtime/src/storage.rs` — `KonfStorage`:

```rust
pub struct KonfStorage {
    db: Arc<redb::Database>,
    journal: Arc<RedbJournal>,
    // Phase 2 will add: scheduler: Arc<RedbScheduler>
    // Phase 3 will add: runner_intents: Arc<RunnerIntentStore>
    pub retention_days: u32,  // default 7
}

impl KonfStorage {
    pub async fn open(path: impl AsRef<Path>, retention_days: u32) -> Result<Self, StorageError>;
    pub fn journal(&self) -> &RedbJournal;
    pub fn journal_arc(&self) -> Arc<dyn JournalStore> { self.journal.clone() }
    // Phase 2: pub fn scheduler(&self) -> &RedbScheduler;
    // Phase 3: pub fn runner_intents(&self) -> &RunnerIntentStore;
}
```

One redb file, one `Database` handle, three logical stores sharing it. This is the single source of truth for all persistent konf state.

7. **Update** `konf-init/src/lib.rs` boot step 8:

```rust
let storage: Option<Arc<KonfStorage>> = match &config.database {
    Some(db) => {
        let path = parse_db_url(&db.url)?;  // accepts redb://, file://, or plain path
        Some(Arc::new(KonfStorage::open(path, db.retention_days).await?))
    }
    None => None,
};

let runtime = Arc::new(
    Runtime::new(
        engine.clone(),
        storage.as_ref().map(|s| s.journal_arc()),
    )
    .await?,
);
```

8. **Update** `konf-init/src/config.rs::DatabaseConfig`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// Path or URL. Supported: "redb:///path", "file:///path", or bare path.
    pub url: String,
    /// Retention window in days for journal entries and terminated runner intents.
    #[serde(default = "default_retention")]
    pub retention_days: u32,
}
fn default_retention() -> u32 { 7 }
```

Remove `pool_min` and `pool_max`.

9. **Update** `docs/architecture/runtime.md:242-279` — keep the checkpoint rejection paragraph, rewrite the backing-store paragraph to reference redb. The doctrine is unchanged; only the implementation changed.

**Test strategy**:

- **Unit tests**: every current journal test runs against `RedbJournal` using `tempfile::tempdir` for the db path. In-memory backend via `redb::backends::InMemoryBackend` where available.
- **Integration test**: start konf-backend, fire a workflow, cleanly shut down, restart, confirm `/v1/admin/audit` returns the events from the previous run, and `reconcile_zombies` marked any in-flight run as failed.
- **Dirty crash test**: SIGKILL konf-backend during workflow execution, restart, confirm `workflow_failed` with reason "System restart" is present for the crashed run.

**Deliverable**: `cargo test --workspace` passes. `konf-backend` runs end-to-end with `KONF__DATABASE__URL=/tmp/konf.redb konf-backend`. Postgres is not anywhere in the build tree of konf-runtime, konf-init, or konf-backend.

**Breaking changes introduced**:
- `KonfInstance.pool: Option<PgPool>` field removed
- `DatabaseConfig.pool_min` / `pool_max` fields removed
- `DatabaseConfig.url` now dispatches on redb URL schemes, not Postgres
- `postgres` feature flag on `konf-init` removed
- Any user-written Rust code that constructed `konf_init::KonfInstance { pool, .. }` or called `.pool` breaks

---

## 8. Phase 2 — Unified redb scheduler

**Goal**: replace BOTH the dead Postgres scheduler AND the ephemeral tokio-timer `schedule:create` with a single durable scheduler in `konf-runtime`.

### 8.1 Data model

One redb table in the shared `KonfStorage`:

```rust
// timers: (next_run_at_unix_ms: u64, job_id: [u8; 16]) -> postcard(TimerRecord)
```

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct TimerRecord {
    pub job_id: Uuid,
    pub workflow: String,              // workflow id, looked up live at fire time
    pub input: serde_json::Value,      // snapshot at schedule time, reused on every fire
    pub namespace: String,             // scope namespace
    pub capabilities: Vec<String>,     // capability patterns granted to each fire
    pub actor: Actor,                  // attribution
    pub mode: TimerMode,
    pub created_at: DateTime<Utc>,
    pub created_by: String,            // who scheduled (for audit)
}

#[derive(Serialize, Deserialize, Clone)]
pub enum TimerMode {
    Once,                              // fire once, then delete
    Fixed { delay_ms: u64 },           // fire every delay_ms, reschedule after each fire
    Cron { expr: String },             // next fire computed from cron expression
}
```

**Why the key encodes fire time**: polling is `timers.range(..now_ms)`. Due jobs sort themselves automatically. Insertion is O(log n); polling is O(due).

### 8.2 `RedbScheduler` (concrete type, no trait)

```rust
pub struct RedbScheduler {
    db: Arc<redb::Database>,
    runtime: Weak<Runtime>,           // break reference cycle; upgraded per fire
    shutdown: CancellationToken,
    poll_interval: Duration,           // default 1s
}

impl RedbScheduler {
    pub async fn schedule_once(&self, record: TimerRecord, run_at: DateTime<Utc>) -> Result<JobId, SchedulerError>;
    pub async fn schedule_fixed(&self, record: TimerRecord, delay_ms: u64) -> Result<JobId, SchedulerError>;
    pub async fn schedule_cron(&self, record: TimerRecord, expr: String) -> Result<JobId, SchedulerError>;
    pub async fn cancel(&self, id: JobId) -> Result<bool, SchedulerError>;
    pub async fn list(&self, namespace_prefix: Option<&str>) -> Result<Vec<JobSummary>, SchedulerError>;

    /// Start the background polling loop. Called by KonfStorage::open.
    pub(crate) fn start_polling(self: Arc<Self>);
}
```

**Polling loop**:

```
loop {
    sleep(poll_interval);
    let now = Utc::now().timestamp_millis() as u64;
    let due = timers.range(..now_ms)   // in spawn_blocking
        .collect::<Vec<(key, record)>>();
    for (key, record) in due {
        // Spawn the workflow via runtime.start() with a freshly constructed scope.
        // Reschedule (Fixed/Cron) or delete (Once) in a write transaction.
        // See §8.4 for the ordering with runner intents.
    }
}
```

**Cron parsing**: `cron = "0.15"` is already in workspace dependencies but unused. Import it now. Parse at schedule time to validate; store the string; reparse at each fire to compute the next fire time.

Note: `cron` crate may be unmaintained — if so, switch to `croner` or `saffron`. Evaluate in Phase 2 when we first try to import it. Fallback is a 1-hour swap.

**Bounds**: `MIN_DELAY_MS = 1000`, `MAX_DELAY_MS = 7 * 24 * 3600 * 1000` (7 days). Cron has no upper bound — a yearly cron just sits in redb until it's due.

### 8.3 `Runtime::scheduler()` accessor

```rust
impl Runtime {
    pub fn scheduler(&self) -> Option<&RedbScheduler>;
}
```

The `Runtime` holds `Option<Arc<RedbScheduler>>` obtained from `KonfStorage`. The scheduler is absent only when the runtime is constructed without a `KonfStorage` (pure unit tests, edge deployments that opt out).

The `Runtime::new` signature in Phase 1a already took `Option<Arc<dyn JournalStore>>`. Phase 2 extends it:

```rust
pub async fn new(
    engine: Engine,
    storage: Option<Arc<KonfStorage>>,
) -> Result<Self, RuntimeError>
```

`KonfStorage` now owns the journal, the scheduler, and (in Phase 3) the runner intents. `Runtime` is constructed from storage, pulls whatever accessors it needs, holds them as fields. This collapses the two separate arguments from 1a into one and simplifies `konf-init::boot` step 9.

### 8.4 Firing order: atomicity vs duplication

When a timer fires, the sequence is:

1. Look up workflow in live registry
2. Construct `ExecutionScope` from record
3. Call `runtime.start()` which (in Phase 3) writes a `RunnerIntent` to redb
4. Reschedule or delete the timer entry

If the process crashes between steps 3 and 4, the same timer fires again next poll — the intent is duplicated. Idempotent-by-contract workflows handle this cleanly.

Alternative: do steps 3 and 4 in a single redb write transaction. More atomic but couples the scheduler's internals to the runner intent store's internals. **Rejected** — the "run twice rather than zero times" tradeoff is explicitly chosen. At-least-once semantics is the documented contract.

### 8.5 Rewrite `konf-init/src/schedule.rs`

The old `ScheduleTool` uses a static `LazyLock<Mutex<HashMap<u64, JoinHandle>>>` for in-process timers. Replace with a thin wrapper over `runtime.scheduler()`:

```rust
pub struct ScheduleTool { runtime: Arc<Runtime> }

#[async_trait::async_trait]
impl Tool for ScheduleTool {
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let scheduler = self.runtime.scheduler()
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "no scheduler configured (runtime missing storage)".into(),
                retryable: false,
            })?;
        let record = build_record_from_input(input, ctx)?;
        let id = match parse_mode(&input)? {
            TimerMode::Once => scheduler.schedule_once(record, parse_run_at(&input)?).await?,
            TimerMode::Fixed { delay_ms } => scheduler.schedule_fixed(record, delay_ms).await?,
            TimerMode::Cron { expr } => scheduler.schedule_cron(record, expr).await?,
        };
        Ok(json!({ "schedule_id": id }))
    }
}
```

The `schedule:create` tool now supports `{ cron: "..." }` input in addition to `{ delay_ms, repeat }`. This is a new capability — cron was previously impossible from the tool. Cancel tool delegates to `scheduler.cancel(id)`.

### 8.6 Wire `schedules.yaml` parsing

Bring back the stubbed `schedules.yaml` parsing. Add to `konf-init::boot` a new step after scheduler initialization:

```rust
// step 9d — register declarative cron jobs
let schedules_path = config_dir.join("schedules.yaml");
if schedules_path.exists() {
    let yaml = fs::read_to_string(&schedules_path)?;
    let entries: Vec<CronEntry> = serde_yaml::from_str(&yaml)?;
    for entry in entries {
        // Validate: workflow must exist in registry
        if !runtime.engine().registry().contains(&format!("workflow:{}", entry.workflow)) {
            warn!(workflow = %entry.workflow, "schedules.yaml references unknown workflow — timer will fail at fire time");
        }
        runtime.scheduler().unwrap().schedule_cron(entry.into_record(), entry.cron).await?;
    }
}
```

**Deduplication on reload**: cron entries are keyed by `(workflow_id, cron_expr, namespace)` as logical identity. On `config:reload` or boot, if an entry with the same identity already exists, skip. Users who want two independent identical crons must differentiate by namespace or use distinct workflow ids.

### 8.7 Delete `konf-backend/src/scheduling/`

Delete the directory. Remove the `scheduling` feature from `konf-backend/Cargo.toml`. Remove `sqlx` optional dep from konf-backend. Remove `AppError::Database`. Remove the scheduler startup block from `main.rs:51-60`.

**Test strategy**:

- Unit test: schedule a Once timer 50ms in the future, poll, confirm it fires once and the entry is deleted
- Unit test: schedule a Fixed timer with 200ms delay, let it run for 1 second, confirm 4-5 fires
- Unit test: schedule a Cron timer with `* * * * *` (every minute), mock time, confirm fire computation
- **Live-code test**: schedule a workflow, edit `workflows/foo.yaml`, `config:reload`, fire the timer, confirm the NEW workflow version ran (via tool call log assertions)
- **Missing-workflow test**: schedule a workflow, delete its YAML file, fire, confirm the scheduler emits a visible error event on the bus and keeps the timer for future retries
- **Restart test**: schedule a Cron timer, kill konf-backend, restart, confirm the timer is loaded from redb and still fires at the next interval
- **Schedules.yaml test**: start with a `schedules.yaml` containing 3 entries, confirm 3 timers in `scheduler.list()`, `config:reload`, confirm still 3 (not 6)

**Deliverable**: `konf-backend/src/scheduling/` is gone. One durable scheduler lives in `konf-runtime`. `schedule:create` works and is durable. `schedules.yaml` is a real config file, not a stub.

**Breaking changes introduced**:
- Old `schedule:create` input schema stays working; new `cron` field added as alternative to `delay_ms`
- `konf-backend` no longer has a `scheduling` feature
- `schedules.yaml` now actually does something — users who had the file but weren't using it will see their entries wake up

---

## 9. Phase 3 — Durable runner intent

**Goal**: `runner:spawn` runs survive `konf-backend` restart via intent replay, without violating the doctrine.

### 9.1 Data model

New redb table in `KonfStorage`:

```rust
// runner_intents: ([u8; 16] run_id) -> postcard(RunnerIntent)
```

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct RunnerIntent {
    pub run_id: RunId,
    pub parent_id: Option<RunId>,
    pub workflow: String,                   // workflow id, looked up live on replay
    pub input: serde_json::Value,           // snapshot at spawn time
    pub namespace: String,
    pub capabilities: Vec<String>,
    pub actor: Actor,
    pub session_id: String,
    pub spawned_at: DateTime<Utc>,
    pub terminal: Option<TerminalStatus>,   // None = running or crashed
    pub replay_count: u32,                  // incremented each time we replay this intent
}

#[derive(Serialize, Deserialize, Clone)]
pub enum TerminalStatus {
    Succeeded,
    Failed { error: String },
    Cancelled { reason: String },
}
```

### 9.2 `RunnerIntentStore`

```rust
pub struct RunnerIntentStore {
    db: Arc<redb::Database>,
}

impl RunnerIntentStore {
    pub async fn insert(&self, intent: RunnerIntent) -> Result<(), IntentError>;
    pub async fn mark_terminal(&self, run_id: RunId, status: TerminalStatus) -> Result<(), IntentError>;
    pub async fn list_unterminated(&self) -> Result<Vec<RunnerIntent>, IntentError>;
    pub async fn list_by_namespace(&self, prefix: &str) -> Result<Vec<RunnerIntent>, IntentError>;
    pub async fn get(&self, run_id: RunId) -> Result<Option<RunnerIntent>, IntentError>;
    pub async fn gc(&self, older_than_days: u32) -> Result<u64, IntentError>;
}
```

### 9.3 `InlineRunner` changes

Modify `konf-tool-runner/src/runners/inline.rs`:

1. `InlineRunner::new` takes an optional `Arc<RunnerIntentStore>` in addition to the existing runtime + registry.
2. **Before** spawning the tokio task in `spawn()`:
   - Build `RunnerIntent { terminal: None, replay_count: 0, .. }`
   - `intent_store.insert(intent).await?`
3. **After** the tokio task completes (success/failure/cancel):
   - `intent_store.mark_terminal(run_id, status).await?`
4. The in-memory `RunRegistry` stays. It's the source of truth for live state and wait semantics. redb is the source of truth for intent and crash recovery.

### 9.4 Replay on boot

Add to `konf-init::boot` a new step after runtime initialization:

```rust
// step 10c — replay unterminated runner intents
if let Some(store) = &storage {
    let intents = store.runner_intents().list_unterminated().await?;
    if !intents.is_empty() {
        info!(count = intents.len(), "replaying unterminated runner intents");
        for mut intent in intents {
            intent.replay_count += 1;
            // Increment replay_count before re-running so a crash loop is visible
            store.runner_intents().insert(intent.clone()).await?;

            let scope = reconstruct_scope(&intent);
            if let Err(e) = runtime.replay_intent_as(intent.run_id, &intent.workflow, intent.input.clone(), scope, intent.session_id.clone()).await {
                warn!(run_id = %intent.run_id, error = %e, "intent replay failed to start; will retry on next boot");
            }
        }
    }
}
```

`runtime.replay_intent_as(run_id, ...)` is a new method that behaves like `Runtime::start` but uses the provided `run_id` instead of generating a new UUID. This preserves TUI bookmark continuity — if the TUI had a link to run `0x7f2a`, that link still resolves after replay.

**Idempotency contract**: the workflow's first invocation and its replay both use the same `run_id` and the same input. If the workflow is properly idempotent (reads cursor from memory, skips done work), the replay is cheap and correct. If not, it re-runs the whole thing — author's problem.

**Runaway replay loop protection**: if `replay_count > 10`, the intent is marked `TerminalStatus::Failed { error: "replay loop exceeded" }` and an error is journaled. Prevents a broken workflow from eating the boot time forever.

### 9.5 Garbage collection

New background task in `KonfStorage::open`:

```rust
// Every 1 hour, delete runner intents where terminal.is_some() && spawned_at < now - retention_days
// Same task also GCs journal entries where created_at < now - retention_days
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        if let Err(e) = storage.gc().await {
            warn!(error = %e, "storage gc failed");
        }
    }
});
```

Retention defaults to 7 days and is configurable via `DatabaseConfig.retention_days` from `konf.toml`.

### 9.6 Documentation

New file `docs/architecture/durability.md`. Short doc explaining:

- The durability doctrine (intent + idempotent retry, not checkpoint-and-replay)
- How runner intents work, with a worked example
- How scheduler timers work
- The relationship between the two (scheduler timers fire → spawn creates intent → replay on crash)
- Idempotency patterns for workflow authors (use memory for cursors and dedup keys)
- What NOT to rely on (mid-workflow state, variable persistence, "resume from step N")

**Test strategy**:

- Unit test: `insert` an intent, `list_unterminated` returns it; `mark_terminal`, `list_unterminated` no longer returns it
- Unit test: `gc` deletes only terminal entries older than cutoff
- **Replay test**: start a workflow via `runner:spawn`, confirm intent is in redb with `terminal: None`, kill the tokio task via runtime internals (simulating partial crash), restart the runtime, confirm `list_unterminated` finds it and the replay path runs the workflow again with the same `run_id`
- **Crash loop test**: a workflow that always panics — confirm replay_count increments, and after 10 replays the intent is marked `Failed { error: "replay loop exceeded" }`
- **Retention test**: set `retention_days: 0`, mark an intent terminal, run gc, confirm deletion

**Deliverable**: `runner:spawn` invocations survive `konf-backend` restart. Workflow authors have clear guidance on idempotency. Admin API can list a user's in-flight runs from redb (not just from the in-memory ProcessTable).

**Breaking changes introduced**:
- `InlineRunner::new` signature takes an extra optional argument
- Any code constructing an `InlineRunner` directly needs updating (internal; `konf-init::boot` is the only caller)

---

## 10. Phase 4 — `/mcp` HTTP endpoint

**Goal**: solve the split-brain problem by letting MCP clients share the same `Arc<Runtime>` as the HTTP REST API.

### 10.1 Dev-mode flag split

Today `KONF_DEV_MODE=1` bypasses JWT auth. This should **not** also gate MCP HTTP — they're independent concerns. Introduce:

```rust
let mcp_http_enabled = std::env::var("KONF_MCP_HTTP").is_ok();
```

`KONF_MCP_HTTP=1` enables the `/mcp` route. Nothing else.
`KONF_DEV_MODE=1` still only bypasses JWT auth for `/v1/*`.

You can enable MCP without bypassing auth, and vice versa. For v1, when `KONF_MCP_HTTP=1` is set, all MCP sessions get `capabilities = ["*"]`. There is no per-session scoping.

**Loud warning**: when `KONF_MCP_HTTP=1` is set, log at WARN level: `"/mcp mounted with capabilities=[\"*\"] — DEV ONLY, never use in production."`

### 10.2 Wiring

**New file** `crates/konf-backend/src/api/mcp.rs`:

```rust
use std::sync::Arc;
use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    StreamableHttpService,
};

pub fn routes(runtime: Arc<konf_runtime::Runtime>) -> Router {
    let rt = runtime.clone();
    let service = StreamableHttpService::new(
        move || {
            let engine = Arc::new(rt.engine().clone());
            Ok(konf_mcp::KonfMcpServer::new(engine, rt.clone()))
        },
        Arc::new(LocalSessionManager::default()),
        Default::default(),
    );
    Router::new().nest_service("/mcp", service)
}
```

The factory closure captures `Arc<Runtime>` — every session-created `KonfMcpServer` shares the same runtime. Runs started via MCP appear in `ProcessTable`, in the journal, and in the TUI's event stream.

**Update** `crates/konf-backend/src/main.rs` after the existing router setup:

```rust
let mut app = Router::new()
    .route("/v1/health", get(api::health::health))
    .merge(protected);

if std::env::var("KONF_MCP_HTTP").is_ok() {
    warn!("/mcp mounted with capabilities=[\"*\"] — DEV ONLY, never use in production.");
    app = app.merge(api::mcp::routes(instance.runtime.clone()));
}

let app = app.layer(cors).layer(TraceLayer::new_for_http());
```

### 10.3 Dependency changes

**Update** `crates/konf-mcp/Cargo.toml`:

```toml
rmcp = { version = "1.3", features = [
    "server",
    "transport-io",
    "transport-streamable-http-server",
] }
```

Adding the feature does not pull axum (verified via Phase 0 spike 0b). `konf-backend` already depends on `konf-mcp` as an optional dep with feature `mcp` enabled by default — no Cargo.toml changes needed on the backend side beyond the new module.

### 10.4 Tool-list change notifications

The stdio path spawns a task watching `engine.subscribe_tool_changes()` and calling `peer.notify_tool_list_changed()`. The Streamable HTTP path creates a fresh handler per session, so the watcher needs to live inside the handler.

**Add** to `KonfMcpServer`:

```rust
pub fn spawn_tool_change_watcher(&self, peer: rmcp::Peer<RoleServer>) {
    let mut rx = self.engine.subscribe_tool_changes();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let _ = peer.notify_tool_list_changed().await;
        }
    });
}
```

Called from `ServerHandler::initialize` where the `Peer` is first available. Spike 0b confirms this is the right integration point.

**Fallback**: if spike 0b reveals the watcher-in-handler pattern has issues in rmcp 1.3, ship v1 without live tool-list notifications. Clients reconnect to see new tools. Minor regression from the stdio experience; acceptable for dev mode.

### 10.5 Tests

- **Unit test**: mount `/mcp` on a test router, verify `StreamableHttpService` is correctly constructed
- **Integration test**: boot a minimal konf-backend with a fake workflow, connect via `reqwest` to `/mcp`, initialize a session, call `tools/list`, confirm the fake workflow appears, call it as a tool, confirm the response
- **Split-brain-fixed test**: same integration test, but additionally poll `/v1/monitor/runs` and assert the MCP-originated run appears in the process table — this is the core regression test for Phase 4

**Deliverable**: `KONF_MCP_HTTP=1 konf-backend` exposes a working Streamable HTTP MCP endpoint at `/mcp`. Claude Code or `@modelcontextprotocol/inspector` can connect and invoke tools. Those invocations appear in the TUI's event stream (Phase 5) and in `/v1/admin/audit`.

**Breaking changes**: none. This is pure addition.

---

## 11. Phase 5 — Live monitor stream for TUI

**Goal**: the TUI subscribes to real-time events for runs in a namespace, without polling.

### 11.1 `RunEventBus`

**New file** `crates/konf-runtime/src/event_bus.rs`:

```rust
use serde::Serialize;
use tokio::sync::broadcast;

pub struct RunEventBus {
    tx: broadcast::Sender<RunEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    RunStarted {
        run_id: RunId,
        workflow_id: String,
        namespace: String,
        parent_id: Option<RunId>,
        started_at: DateTime<Utc>,
    },
    NodeStart { run_id: RunId, node_id: String, tool: String, at: DateTime<Utc> },
    NodeEnd { run_id: RunId, node_id: String, status: NodeStatus, at: DateTime<Utc> },
    TextDelta { run_id: RunId, node_id: String, delta: String },
    RunCompleted { run_id: RunId, duration_ms: u64, output: serde_json::Value },
    RunFailed { run_id: RunId, duration_ms: u64, error: String },
    RunCancelled { run_id: RunId, reason: String },
    ScheduleFired { job_id: JobId, workflow_id: String, fired_at: DateTime<Utc> },
    ScheduleFailed { job_id: JobId, reason: String },
    JournalAppended { sequence: u64, event_type: String, namespace: String, run_id: RunId },
    IntentReplayed { run_id: RunId, replay_count: u32 },
}

impl RunEventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> { self.tx.subscribe() }
    pub(crate) fn emit(&self, event: RunEvent) {
        let _ = self.tx.send(event);  // ignore "no subscribers" error
    }
}
```

`Runtime` holds `Arc<RunEventBus>`, created in `Runtime::new` with a default capacity of 1024. Capacity is configurable via `PlatformConfig.runtime.event_bus_capacity`.

### 11.2 Emission points

Add `bus.emit(...)` calls at:

- `Runtime::start` → `RunStarted`
- `Runtime::start_streaming` → `RunStarted`
- Completion arm in both spawned tasks → `RunCompleted` / `RunFailed` / `RunCancelled`
- `RuntimeHooks::on_node_start` → `NodeStart`
- `RuntimeHooks::on_node_end` → `NodeEnd`
- `RuntimeHooks::on_text_delta` (if exists; otherwise extend `ExecutionHooks`) → `TextDelta`
- `RedbJournal::append` → `JournalAppended`
- `RedbScheduler` fire loop → `ScheduleFired` or `ScheduleFailed`
- `konf-init::boot` replay loop → `IntentReplayed`

These are fire-and-forget from the emitter's perspective — broadcast channel never blocks the sender.

### 11.3 Broadcast semantics

Slow subscribers may get `RecvError::Lagged(n)`. The TUI should handle this by:

1. Showing a "lagged N events" indicator
2. Refetching current state from `/v1/monitor/runs` / `/v1/monitor/runs/{id}/tree`
3. Resuming the stream

Capacity 1024 is comfortable for a local dev tool. Raise if production telemetry shows steady lag.

### 11.4 `GET /v1/monitor/stream` SSE endpoint

Extend `crates/konf-backend/src/api/monitor.rs` with a new handler. Reuse the existing SSE pattern from `api/chat.rs`:

```rust
#[derive(Deserialize)]
pub struct StreamParams {
    /// Optional namespace prefix filter.
    pub namespace: Option<String>,
}

pub async fn stream(
    State(state): State<AppState>,
    Query(params): Query<StreamParams>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    use tokio::sync::broadcast::error::RecvError;
    let mut rx = state.runtime.event_bus().subscribe();
    let filter = params.namespace;

    let stream = async_stream::stream! {
        yield Ok(Event::default().event("hello").data(r#"{"status":"connected"}"#));
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(ref prefix) = filter {
                        if !event_namespace(&event).starts_with(prefix.as_str()) {
                            continue;
                        }
                    }
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    let kind = event_type(&event);
                    yield Ok(Event::default().event(kind).data(json));
                }
                Err(RecvError::Lagged(n)) => {
                    yield Ok(Event::default().event("lagged").data(n.to_string()));
                }
                Err(RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}
```

**Route registration** in `main.rs`:

```rust
.route("/v1/monitor/stream", get(api::monitor::stream))
```

Under the same JWT middleware as other `/v1/monitor/*` routes. Namespace filtering honors the authenticated user's scope in production — for v1 dev-mode, any namespace prefix is accepted.

### 11.5 Tests

- **Unit test**: bus emits an event, subscriber receives it; subscribe-then-emit both arrivals are visible
- **Unit test**: lagged subscriber gets `Lagged(n)` correctly
- **Integration test**: open an SSE connection, fire a workflow via `/v1/chat`, confirm `RunStarted` → `NodeStart` × N → `NodeEnd` × N → `RunCompleted` arrive in order
- **Namespace filter test**: two subscribers with different namespace prefixes, fire two workflows in different namespaces, confirm each subscriber sees only their own
- **Schedule fire test**: schedule a timer 500ms in the future, subscribe to the stream, confirm `ScheduleFired` arrives followed by `RunStarted` / `RunCompleted`

**Deliverable**: a TUI consuming `/v1/monitor/stream` via SSE sees every workflow start, every node transition, every schedule fire, in real time, filtered to its namespace. Polling is no longer required. The pattern matches `/v1/chat`'s existing SSE implementation.

**Breaking changes**: none. Pure addition.

---

## 12. Phase 6 — Documentation and cleanup

**Goal**: bring docs into alignment with the v2 implementation. Delete stale references. Add new architecture docs.

### 12.1 Edits to existing docs

1. **`docs/MENTAL_MODEL.md`**:
   - Line 72: delete "Postgres with pgvector is required for memory-backed products." Replace with: "The journal is backed by an embedded redb file. Memory backends (SurrealDB default, smrti optional) are independent and have their own storage requirements."
   - Line 97: update the `Run` vocabulary entry to mention `runner_intents` persistence
   - Update the crate list at lines 17-26 to note that `konf-runtime` now owns the scheduler and storage

2. **`docs/architecture/runtime.md`**:
   - Lines 242-279: rewrite the durability section. Keep the checkpoint rejection as doctrine (it's still correct). Describe redb as the backing store. Describe runner intent replay as the concrete mechanism. Link to `durability.md`.

3. **`docs/architecture/overview.md`**:
   - Remove any `konf-backend → Postgres` arrows
   - Add the redb file as the state backing store
   - Add the `/mcp` mount (marked dev-only)
   - Add the monitor SSE stream

4. **`docs/architecture/init.md`**:
   - Update the 12-step boot sequence to reflect:
     - Step 8: open KonfStorage (redb file)
     - Step 9: create runtime from engine + storage
     - Step 10c: replay unterminated runner intents
     - Step 10d: register schedules.yaml entries

5. **`docs/architecture/mcp.md`**:
   - Add a section on the `/mcp` HTTP endpoint, separate from the stdio server
   - Document `KONF_MCP_HTTP=1` flag
   - Loud warning about dev-mode-only caps

6. **`docs/admin-guide/platform-config.md`**:
   - Update `[database]` section: accepted URL schemes (`redb://`, `file://`, bare path), `retention_days`
   - Remove `pool_min` / `pool_max`

7. **`docs/getting-started/quickstart.md`**:
   - Remove docker-compose + Postgres setup
   - Replace with: `KONF__DATABASE__URL=/tmp/konf.redb konf-backend`
   - Add a note about enabling `/mcp` for dev: `KONF_MCP_HTTP=1`

8. **`docs/product-guide/workflow-reference.md`**:
   - Add an idempotency patterns section (memory-backed cursors, dedup keys)
   - Link to `durability.md`

### 12.2 New docs

1. **`docs/architecture/durability.md`** — the single source of truth for what durability means in konf. Organized around the four mental-model clarifications from §2. Includes the worked email-watcher example. Includes the idempotency patterns. Calls out what konf does NOT do (mid-workflow resume).

2. **`docs/architecture/mcp-http.md`** — the HTTP MCP endpoint story. Dev-mode-only, session model, capability grant (`["*"]` in v1), rationale for why production auth is future work.

3. **`docs/architecture/storage.md`** — the three redb tables (journal, timers, runner_intents) in one file, how they share a `Database` handle via `KonfStorage`, retention model, GC.

### 12.3 Banned-word lint check

Run `docs-lint` against the new docs. The banned-word list at `docs/MENTAL_MODEL.md:126-148` includes things like "self-modification", "self-healing", various marketing phrases. The word "checkpoint" is implicitly banned via the runtime doctrine — verify none of the new docs use it except in the explicit "why we don't do this" sense.

### 12.4 Deliverable

All docs reference only the v2 architecture. No stale Postgres references. No stale apalis references. Durability is a first-class doc. New users reading `quickstart.md` have a one-line path to a running local instance.

---

## 13. Success criteria (all phases)

The plan is complete when all of these are simultaneously true:

1. `cargo test --workspace` passes with zero warnings
2. `cargo clippy --workspace --all-targets -- -D warnings` passes
3. `cargo check --workspace --no-default-features` succeeds (verifying feature gating)
4. `konf-backend` boots successfully with `KONF__DATABASE__URL=/tmp/konf.redb konf-backend` — no docker-compose, no Postgres
5. `konf-mcp --config path/to/product` still works as a standalone stdio server
6. `KONF_MCP_HTTP=1 konf-backend` exposes a working `/mcp` endpoint; `@modelcontextprotocol/inspector` can connect and invoke tools
7. A workflow scheduled via `schedules.yaml` fires after konf-backend restart
8. A workflow spawned via `runner:spawn` that crashes mid-flight replays on restart with the same `run_id`
9. A curl against `/v1/monitor/stream` emits SSE events for workflow starts and completions in real time
10. `grep -r "sqlx" crates/konf-runtime crates/konf-backend crates/konf-init` returns zero matches
11. `grep -r "PgPool" crates/` returns zero matches
12. `grep -r "apalis" docs/ crates/` returns zero matches
13. `docs/architecture/durability.md` exists and is linked from `MENTAL_MODEL.md`
14. `docs/getting-started/quickstart.md` no longer mentions Postgres or docker-compose
15. `konf-backend/src/scheduling/` does not exist

---

## 14. Risks and how they're handled

| Risk | Mitigation |
|---|---|
| redb append latency is too high under load | Spike 0c measures this; if bad, add batching writer task per §7.2 optional section |
| `cron` crate 0.15 is unmaintained | Evaluate at Phase 2 start; swap to `croner` or `saffron` if needed (1-hour fix) |
| rmcp 1.3 `transport-streamable-http-server` feature has hidden constraints | Spike 0b verifies with a working example before Phase 4 starts |
| Error-type cascade from `RuntimeError::Database` removal surprises more callers than expected | Spike 0a catches this without committing to the full refactor |
| Tokio `broadcast` lagging causes missed TUI events | Documented behavior; subscribers refetch state on `Lagged`; capacity can be raised |
| Runner intent replay loop for a broken workflow eats boot time | `replay_count > 10` marks terminal `Failed`, halts replay |
| Scheduler duplicate fire across crash boundaries | Documented at-least-once semantics; author idempotency handles it |
| Users had data in the old Postgres journal they expected to carry over | Documented as non-goal; migration is not supported |
| `schedules.yaml` references a workflow that's been deleted | Scheduler logs a visible error event, keeps the timer, retries next fire |

---

## 15. Implementation ordering

Phases can be merged as separate PRs in this order. Each phase is independently revertible.

```
Phase 0 (spike, throwaway)        ── 1 day
   │
   ▼
Phase 1a (sqlx decoupling)        ── 2 days
   │
   ▼
Phase 1b (redb journal)           ── 3 days
   │
   ▼
Phase 2 (redb scheduler)          ── 4 days (includes schedules.yaml wiring)
   │
   ▼
Phase 3 (runner intents)          ── 3 days
   │
   ├──▶ Phase 4 (/mcp endpoint)   ── 2 days [independent, can run in parallel]
   │
   └──▶ Phase 5 (monitor stream)  ── 2 days [independent, can run in parallel]
                │
                ▼
           Phase 6 (docs)          ── 2 days
```

Phases 4 and 5 have no dependency on each other or on Phase 3 and can proceed in parallel once Phase 2 lands (they only need the runtime to be storage-backed, which happens in Phase 1b).

Time estimates are engineering days, not calendar days. No hard deadline — konf should be at its best version after this update.

---

## 16. Future Work

Explicitly out of scope for v2, but the architecture supports them as future phases:

### 16.1 Approval / human-in-the-loop tool (target: post-v2, small)

Add an `approval:request` tool that pauses a workflow and waits for external confirmation via an HTTP/SSE round-trip. Requires a small engine primitive: "suspend this node on external signal." Enables coding-agent approval flows and any human-in-the-loop workflow pattern. Estimated effort: 2-3 days.

### 16.2 FS tool crate (target: post-v2, small)

New `konf-tool-fs` crate with sandboxed `fs:read`, `fs:write`, `fs:edit`, `fs:list`, `fs:glob`, `fs:grep` tools. Config-based root allowlist. Prerequisite for a full coding-agent experience. Estimated effort: 1 day.

### 16.3 Multi-tenant hardening (target: when first SaaS deployment is attempted)

Per-user concurrency quotas, HTTP rate limiting via `tower-governor`, usage tracking table in redb, namespace-filtered monitor APIs, billing aggregation endpoints. Unlocks hosted multi-user deployments beyond trusted small teams. Estimated effort: 1-2 weeks.

### 16.4 Production MCP auth (target: when remote MCP is needed)

Custom `SessionManager` implementation that reads JWT from Streamable HTTP headers, maps claims to a user_id and a capability set, creates `KonfMcpServer::with_capabilities(engine, runtime, user_caps)` per session. Removes the `KONF_MCP_HTTP` dev-only restriction. Estimated effort: 1 week.

### 16.5 Hard storage isolation (target: compliance workloads)

Pluggable backend factory so per-tenant redb files / per-tenant SurrealDB databases become possible. Unlocks HIPAA-style deployments where logical isolation is insufficient. Estimated effort: 2-3 weeks.

### 16.6 Tool-runner OS-level isolation

`SystemdRunner` and `DockerRunner` backends for `konf-tool-runner`. Each runner:spawn gets an OS process boundary, crash isolation, resource limits enforced by cgroups. `InlineRunner` remains for low-overhead local workflows. Estimated effort: 1-2 weeks.

---

## 17. Appendix: decisions log

Resolved questions from the design phase, recorded for future reference:

| Question | Decision | Rationale |
|---|---|---|
| Run-id reuse on replay? | Same run_id | TUI bookmark continuity |
| Retention window? | Configurable, default 7 days | Balances audit trail with disk pressure |
| `schedules.yaml` location? | Product-level (`config/schedules.yaml`) | Matches `tools.yaml` pattern |
| MCP HTTP flag? | `KONF_MCP_HTTP=1` env var | Separate from `KONF_DEV_MODE` to avoid coupling |
| Phase 0 spike? | Kept | Error-type fallout tends to surprise |
| Journal store as a trait? | Yes | Enables test fakes; low cost |
| Scheduler as a trait? | No | Only one impl; YAGNI |
| `KonfStorage` crate? | No | Lives in `konf-runtime`; no need for a separate crate |
| redb vs SQLite vs sled vs fjall? | redb | Post-1.0 stable, pure Rust, production-proven, has range queries |
| sqlx or trait objects? | Trait objects | sqlx AnyPool can't handle Postgres-specific types cleanly |
| Port Postgres scheduler or delete it? | Delete | Zero call sites; cron parsing was a stub |
| Mid-workflow checkpoint-and-replay? | Rejected | Doctrine; LLM non-determinism |
| Config resolution for scheduled jobs? | Live code, snapshotted input | Matches existing `schedule:create` behavior |
| Broadcast vs watch channel for events? | Broadcast | Multi-subscriber, history buffer, well-known `Lagged` semantics |
| Cron crate? | `cron = "0.15"` (already in workspace) | Already declared but unused; swap if unmaintained |

---

## 18. Quick reference — file change map

| File | Status | Phase |
|---|---|---|
| `crates/konf-runtime/src/journal/mod.rs` | new (renamed from journal.rs) | 1a |
| `crates/konf-runtime/src/journal/redb.rs` | new | 1b |
| `crates/konf-runtime/src/scheduler.rs` | new | 2 |
| `crates/konf-runtime/src/runner_intents.rs` | new | 3 |
| `crates/konf-runtime/src/storage.rs` | new | 1b (extended in 2, 3) |
| `crates/konf-runtime/src/event_bus.rs` | new | 5 |
| `crates/konf-runtime/src/runtime.rs` | modified (signature, fields) | 1a, 1b, 2, 5 |
| `crates/konf-runtime/src/error.rs` | modified (variant swap) | 1a |
| `crates/konf-runtime/src/hooks.rs` | modified (field type, emit events) | 1a, 5 |
| `crates/konf-runtime/Cargo.toml` | modified (remove sqlx, add redb) | 1a, 1b |
| `crates/konf-init/src/lib.rs` | modified (boot sequence) | 1a, 1b, 2, 3 |
| `crates/konf-init/src/schedule.rs` | rewritten (thin wrapper) | 2 |
| `crates/konf-init/src/config.rs` | modified (DatabaseConfig) | 1b |
| `crates/konf-init/Cargo.toml` | modified (remove postgres feature) | 1a, 1b |
| `crates/konf-backend/src/scheduling/` | **deleted** | 2 |
| `crates/konf-backend/src/api/mcp.rs` | new | 4 |
| `crates/konf-backend/src/api/monitor.rs` | modified (stream endpoint) | 5 |
| `crates/konf-backend/src/error.rs` | modified (remove Database variant) | 2 |
| `crates/konf-backend/src/main.rs` | modified (router, flag, scheduler-free) | 2, 4, 5 |
| `crates/konf-backend/Cargo.toml` | modified (remove scheduling feature) | 2 |
| `crates/konf-mcp/Cargo.toml` | modified (add transport feature) | 4 |
| `crates/konf-tool-runner/src/runners/inline.rs` | modified (intent store) | 3 |
| `crates/konf-tool-runner/src/runner.rs` | possibly modified (constructor) | 3 |
| `Cargo.toml` (workspace) | modified (add redb, postcard) | 1b |
| `docs/MENTAL_MODEL.md` | modified | 6 |
| `docs/architecture/runtime.md` | modified | 6 |
| `docs/architecture/overview.md` | modified | 6 |
| `docs/architecture/init.md` | modified | 6 |
| `docs/architecture/mcp.md` | modified | 6 |
| `docs/architecture/durability.md` | new | 6 |
| `docs/architecture/mcp-http.md` | new | 6 |
| `docs/architecture/storage.md` | new | 6 |
| `docs/admin-guide/platform-config.md` | modified | 6 |
| `docs/getting-started/quickstart.md` | modified | 6 |
| `docs/product-guide/workflow-reference.md` | modified | 6 |

Approximately 20 files modified, 11 files added, 1 directory deleted. Total change is focused and contained.

---

*End of plan. See §15 for implementation ordering, §16 for future work, §17 for decisions log.*
