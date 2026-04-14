# Storage in konf

Konf keeps all persistent state in **one redb file** managed by
[`konf_runtime::KonfStorage`](../../crates/konf-runtime/src/storage.rs).
No external database. No docker-compose for local dev.

## Why redb

- Pure Rust (only transitive C dep is `libc`). No SQLite, no RocksDB,
  no LMDB.
- Post-1.0 stable (currently v4). Used in production by Lighthouse and
  Iroh.
- MVCC: concurrent readers don't block the writer.
- Range queries via `table.range(lower..upper)` — exactly what the
  scheduler polling loop needs.
- Small binary footprint (~170 KiB crate).

Alternatives considered and rejected:

- **SQLite**: relational semantics we don't need, requires C deps or
  a heavy pure-Rust port, wrong tool for append-only logs.
- **sled**: stuck in beta, last release 2021, known issues.
- **fjall**: LSM, still pre-1.0, less mature than redb.
- **jammdb**: BoltDB port, works but less idiomatic.

## The three logical stores

One `redb::Database`, three disjoint sets of tables:

| Store | Tables | Purpose |
|---|---|---|
| **Journal** | `journal_events`, `journal_by_run`, `journal_by_session` | Append-only audit log of workflow lifecycle events |
| **Scheduler** | `scheduler_timers`, `scheduler_timers_by_id` | Durable timers for cron + delayed jobs |
| **Runner intents** | `runner_intents`, `runner_intents_by_namespace` | Spawn-intent records for restart replay |

Each store has its own module in `konf-runtime`:

- [`journal/mod.rs`](../../crates/konf-runtime/src/journal.rs) and
  [`journal/redb.rs`](../../crates/konf-runtime/src/journal/redb.rs)
- [`scheduler.rs`](../../crates/konf-runtime/src/scheduler.rs)
- [`runner_intents.rs`](../../crates/konf-runtime/src/runner_intents.rs)

They never read or write each other's tables. `KonfStorage` owns the
single `Arc<Database>` and hands out references.

## Journal fan-out (Stigmergic Engine)

The journal is a `JournalStore` trait, not a single implementation.
When a deployment configures both the redb primary and a SurrealDB
memory backend, `konf-init` wraps them in a
[`FanoutJournalStore`](../../crates/konf-runtime/src/journal/fanout.rs)
so every entry lands in both:

- **Primary (redb)** — synchronous write; the write path acknowledges
  only after primary success. Audit integrity lives here.
- **Secondary (SurrealDB `event` table)** — fire-and-forget `tokio::spawn`
  per entry. Populates the long-term queryable interaction graph.
  Failures are logged and counted via `FanoutMetrics`; they never block
  or fail the primary write.

The SurrealDB secondary lives in
[`konf-tool-memory-surreal::SurrealJournalStore`](../../crates/konf-tool-memory-surreal/src/journal_store.rs)
and writes through a `Surreal<Any>` handle directly — not through the
`MemoryBackend` tool trait — so no journal append can dispatch a tool,
structurally preventing recorder recursion.

See `konf-genesis/docs/STIGMERGIC_ENGINE.md` for the broader design and
`INTERACTION_SCHEMA.md` for the envelope those entries carry.

## Serialization

All values are postcard-encoded bytes. Postcard is compact, fast, and
works through `serde::{Serialize, Deserialize}`. One caveat:
postcard does not support `serde_json::Value` directly. Any field that
needs to carry arbitrary JSON (workflow input, event payloads) is
round-tripped through a JSON **string** in an intermediate `Stored*`
struct. See `StoredTimer` and `StoredIntent` for the pattern.

## Async integration

redb is synchronous. All reads and writes wrap in
`tokio::task::spawn_blocking`, which keeps the tokio runtime
responsive. For a single-node local deployment the latency overhead
is negligible (~100µs). A future batching-writer optimization is
available if write rates get high enough to matter.

## Configuration

```toml
# konf.toml
[database]
url = "redb:///var/lib/konf/konf.redb"
retention_days = 7
```

Accepted URL forms:

- `redb:///absolute/path/konf.redb`
- `file:///absolute/path/konf.redb`
- `./relative/path/konf.redb` (bare path)

`retention_days` controls how long terminal journal entries and
terminated runner intents are kept before the background GC task
deletes them. Default 7.

If `[database]` is omitted entirely, konf runs in edge mode: no
journal, no scheduler, no runner-intent persistence. Workflows still
run but nothing survives a restart.

## Operational notes

- **Backup**: `cp /var/lib/konf/konf.redb /backup/konf.redb.$(date +%s)`.
  Take the backup while `konf-backend` is stopped or use
  `redb::Database::compact()` first for a consistent snapshot.
- **Inspection**: `redb` ships a CLI (`redb-cli`) that can dump tables
  for debugging. Point it at the file.
- **Schema migration**: on breaking changes to `Stored*` structs, we
  version the layout inside the postcard payload (a `u8` discriminator
  at the front of each value) or bump a table name. v1 starts clean.

## What does NOT live in this file

Memory-backed state (user notes, conversation history, embeddings)
lives in **SurrealDB** via the `konf-tool-memory-surreal` backend, not
in the redb file. The two are deliberately separate:

- **redb file** = runtime plumbing (audit, schedules, spawn intents).
  Small, hot, read every second. Owned by `konf-runtime`.
- **SurrealDB** = application-level memory. Larger, queried via tool
  calls, addressable by namespace. Owned by `konf-tool-memory`.

They use different file locations and different backup strategies.
Memory is the user's data; the redb file is konf's bookkeeping.
