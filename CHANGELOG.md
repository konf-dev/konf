# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added — Stigmergic Engine v0 substrate

- `konf-runtime::Interaction`, `InteractionKind`, `InteractionStatus` —
  uniform envelope for every edge-traversal in the system (tool
  dispatch, workflow node lifecycle, run lifecycle, user input, LLM
  response, error). OpenTelemetry-aligned field naming so interactions
  can be exported to Jaeger / Tempo / Honeycomb without translation.
- `konf-runtime::FanoutJournalStore` + `FanoutMetrics` — composes one
  primary `JournalStore` with zero or more secondaries. Primary-succeeds
  acknowledgment semantics; secondary failures are logged-and-counted,
  never propagated. Audit integrity is preserved if any secondary lags
  or fails.
- `konf-tool-memory-surreal::SurrealJournalStore` +
  `connect_journal()` — appends journal entries to the existing
  SurrealDB `event` table so interactions become queryable in the
  long-term graph alongside redb's short-retention audit. No schema
  migration required.
- `ExecutionScope::trace_id: Option<Uuid>` + `with_trace_id()` /
  `ensure_trace_id()` — inherited through `child_scope()` so
  causation DAGs survive spawn boundaries. OTel `trace_id` analog.
- `Runtime::invoke_tool` now records a `ToolDispatch` Interaction into
  the journal for every call (fire-and-forget; recorder failures never
  surface in the tool result). Captures actor, namespace,
  edge_rules_fired (capability + guard), and status inline per record
  for multi-tenant self-auditability.
- `konf-init` auto-wires a `FanoutJournalStore` (redb primary + surreal
  secondary) when both backends are configured. Degrades gracefully to
  primary-only if the surreal secondary fails to connect.

### Changed

- `NodeStatus` enum extended with `Completed { duration_ms }` and
  `Failed { error }` variants, required for the `RunEvent::NodeEnd`
  discriminator.
- `RuntimeHooks` now holds an `Arc<RunEventBus>` and emits
  `RunEvent::NodeStart` / `NodeEnd` at every workflow node transition.
  Previously these variants were defined on the bus but never emitted
  — downstream SSE subscribers and the new interaction recorder both
  depend on them.

### Tests

- 66 new tests across 9 integration files covering all Stigmergic
  Engine substrate work. Clock Challenge benchmark demonstrates
  sub-linear scaling: per-op latency at N=1000 is 0.55× per-op at
  N=10 (amortization under real concurrency).

## [0.1.0] - 2026-04-07

### Added

- Initial release: 10-crate monorepo
- `konflux-core`: workflow execution engine with MCP-native registries (Tools, Resources, Prompts)
- `konf-runtime`: process management with capability-based security and namespace injection
- `konf-init`: config-driven bootstrap system
- `konf-mcp`: MCP server (stdio transport) for Claude Desktop and other MCP clients
- `konf-backend`: HTTP server with SSE streaming (axum)
- `konf-tool-http`: HTTP GET/POST tools with SSRF protection
- `konf-tool-llm`: LLM completion via rig-core (OpenAI, Anthropic, Google)
- `konf-tool-embed`: local text embeddings via fastembed (ONNX)
- `konf-tool-mcp`: MCP client for consuming external MCP servers
- `konf-tool-memory`: MemoryBackend trait for pluggable storage backends
- Workflow-as-tool: workflows register as callable tools for composition
- ToolAnnotations: MCP-parity behavioral hints (read_only, destructive, idempotent, open_world)
- Configurable CORS, optional database, edge-mode operation
- GitHub Actions CI (fmt, clippy, test, cargo-deny)
- Docker multi-stage build with cargo-chef caching
