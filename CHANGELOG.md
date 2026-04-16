# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [0.2.0] - 2026-04-16

### A+++++ Substrate Rebuild (Stages 0–10)

Complete rebuild of the core substrate. 18 commits, 136 files changed,
9367 insertions, 3510 deletions. 53 test suites, 0 clippy warnings.

#### Substrate (konflux-substrate)

- **Crate renamed** from `konflux-core` to `konflux-substrate`
- **Envelope<P>** — typed dispatch wrapper carrying identity (trace_id,
  parent_id, actor_id, namespace), authority (CapSet), control (deadline,
  idempotency_key, qos_class), and observability (step_index, stream_id).
  Every tool invocation goes through an Envelope.
- **Tool trait** takes `Envelope<Value>` — no more `ToolContext` or
  `metadata: HashMap<String, Value>`. Context is typed.
- **Typed capabilities** — `Capability` (sealed constructor) and `CapSet`
  with `check_access()`, `attenuate()`. Pattern matching: `"*"`,
  `"memory:*"`, exact. Unforgeable by construction.
- **StateProjection trait** + `Projection` type — actors declare their
  observable state for bisimulation (SHA-256 hashed). Opt-in; stateless
  tools return `None`.
- **JournalStore trait** — append-only audit log with `query(filter)`,
  `aggregate(filter, query)`, `delete_expired()`. `JournalFilter` with
  `include_expired` flag. `AggregateQuery` (Count, MostRecent).
- **Parser** accepts `&EngineConfig` for configurable retry defaults.

#### Runtime (konf-runtime)

- **Dispatcher** — extracted single-tool dispatch path (capability check,
  VirtualizedTool + GuardedTool wrapping, envelope construction,
  Interaction recording, event emission).
- **WorkflowTool** — reconstructs caller scope from Envelope at invocation
  time. No more boot-baked `default_scope`.
- **ExecutionContext** — split from ExecutionScope. Carries per-dispatch
  runtime state: trace_id, parent_interaction_id, session_id, deadline,
  idempotency_key.
- **Interaction** — 19-field record for every edge-traversal. OTel-aligned
  (id↔span_id, trace_id, parent_id↔parent_span_id). Includes step_index,
  stream_id, state_before_hash, state_after_hash, references, in_reply_to.
- **Journal TTL** — `valid_to` on entries, expired-invisible invariant on
  all queries, `TtlSweeper` background task, `BY_EXPIRY` redb index.
- **Subscribe** — `JournalSubscription` replays from journal then bridges
  live `JournalAppended` events from the event bus.
- **Aggregate** — Count and MostRecent over filtered journal entries.
- **BudgetTable** — per-trace shared decrementable cells for token budgets.
- **Bisimulation** — `bisimulate(trace_a, trace_b)` compares state-hash
  chains for PTM equivalence.
- **Deadline enforcement** — checked before dispatch and clamped into
  tokio timeout mid-execution.
- **Idempotency** — tool-scoped key lookup in journal before dispatch.
  BY_IDEMPOTENCY redb index.

#### Configuration

- All behavioral constants surfaced to `konf.toml`:
  - `[engine]`: `default_retry_base_delay_ms`, `default_retry_max_delay_ms`
  - `[runtime]`: `event_bus_capacity`
  - `[journal]`: `sweep_interval_secs`, `subscribe_buffer`
  - `[auth]`: `jwks_cache_ttl_secs`
- HTTP tool limits (`max_timeout_secs`, `max_response_bytes`) configurable
  via `tools.yaml`.

#### Security

- Dev mode auth bypass requires explicit `KONF_DEV_MODE=true` (was `is_ok()`)
- Idempotency key scoped to tool name (prevents cross-tool collision)
- JWKS fetch checks response status before parsing
- VirtualizedTool rejects non-object payloads when bindings present
- Secret error messages uniform (no allowlist confirmation)
- Shell command logged at DEBUG, not INFO
- Query limit applied after expiry filter (was before)
- delete_expired cleans BY_IDEMPOTENCY index
- BudgetTable::mint rejects duplicate keys

#### Documentation

- All 20 docs verified against code. Zero false claims.
- `workflow-reference.md` rewritten from parser schema
- `platform-config.md` defaults match actual `Default` impls
- Full tool inventory documented (runner, shell, secret, schedule, config)
- ExecutionContext and Interaction schema documented in runtime.md

### Removed

- `konflux-core` crate (renamed to `konflux-substrate`)
- `konf-init-kell` crate (deprecated)
- `ToolContext` struct (replaced by `Envelope<Value>`)
- `WorkflowTool::default_scope` field
- `docs/plans/konf-v2.md` (superseded by substrate rebuild plan)
- Postgres references from deployment docs (redb is default)

## [0.1.0] - 2026-04-07

### Added

- Initial release: 10-crate monorepo
- `konflux-core`: workflow execution engine with MCP-native registries
- `konf-runtime`: process management with capability-based security
- `konf-init`: config-driven bootstrap system
- `konf-mcp`: MCP server (stdio transport)
- `konf-backend`: HTTP server with SSE streaming (axum)
- `konf-tool-http`: HTTP GET/POST tools
- `konf-tool-llm`: LLM completion via rig-core
- `konf-tool-embed`: local text embeddings via fastembed
- `konf-tool-mcp`: MCP client for external MCP servers
- `konf-tool-memory`: MemoryBackend trait for pluggable storage
- Workflow-as-tool composition
- GitHub Actions CI, Docker multi-stage build
