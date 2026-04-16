# What Konf is

Konf is an operating system for AI agents. It provides the runtime — workflow
execution, tool dispatch, capability enforcement, process management, memory —
that agent products run on. An agent product is a directory of YAML and
markdown; there is no per-product Rust code. The same Rust binary runs every
product.

"Operating system" is a structural description, not an analogy. The Rust code
implements an engine, a runtime with a process table, an init system, tool
registries, namespaces, and a capability lattice. Each role is a Rust crate.

---

## The Rust crates (the kernel)

| Crate | Role | Entry point |
|---|---|---|
| `konflux-substrate` | Workflow execution engine. Three registries: tools, resources, prompts. Parses YAML workflows, executes them as DAGs, enforces capabilities at dispatch. Zero I/O. | `Engine` in `crates/konflux-substrate/src/engine.rs` |
| `konf-runtime` | Process manager. Wraps the engine with `ExecutionScope` (namespace, capabilities, limits, actor), process table, `VirtualizedTool` for namespace injection, `GuardedTool` for deny/allow rules, attenuation-only capability lattice. | `Runtime` in `crates/konf-runtime/src/runtime.rs` |
| `konf-init` | Bootstrap. Reads platform config (`konf.toml`) and product config (`tools.yaml`, `workflows/`, `prompts/`), interpolates env vars in YAML, registers built-in and configured tools, connects to Postgres if configured, creates the runtime, registers workflows as tools, applies tool guards. See `docs/architecture/init.md` for the full boot sequence. | `boot(config_dir)` in `crates/konf-init/src/lib.rs` |
| `konf-backend` | HTTP transport. `POST /v1/chat` with SSE streaming. Optional scheduler backed by Postgres. | `crates/konf-backend/src/main.rs` |
| `konf-mcp` | MCP transport. Exposes workflows as MCP tools over stdio or SSE. Translates tool names at the wire: kernel `foo:bar` → MCP `foo_bar` (required by MCP spec SEP-986). | `crates/konf-mcp/src/main.rs` |
| `konf-tool-*` | Plugin crates registering built-in tools: `http`, `llm`, `embed`, `memory`, `mcp` (client), `shell`, `secret`, `runner`. | `crates/konf-tool-*/src/lib.rs` |

Memory is pluggable via the `MemoryBackend` trait in `konf-tool-memory`
(`crates/konf-tool-memory/src/lib.rs:71-125`). Two backends ship today:

- **`konf-tool-memory-surreal`** is the default. Backed by
  [SurrealDB](https://surrealdb.com), it runs in embedded mode (single-file
  RocksDB, no daemon) or remote mode (WebSocket to a Surreal server) with
  identical SurrealQL in both. It stores typed nodes and relation edges,
  exposes HNSW vector search, BM25 full-text search, and Reciprocal Rank
  Fusion hybrid search. Implementation at
  `crates/konf-tool-memory-surreal/src/lib.rs`; schema at
  `crates/konf-tool-memory-surreal/src/schema.rs`.
The memory backend is a dumb storage layer: it does not call LLMs or generate
embeddings. Products pass pre-computed embeddings when they need semantic
search — for SurrealDB via the `metadata_filter.query_vector` escape hatch
inside `SearchParams`.

---

## What a product looks like

```
my-product/
├── config/
│   ├── project.yaml     # name, description, triggers, capabilities
│   ├── tools.yaml       # memory backend, LLM provider, http, embed, MCP clients
│   ├── models.yaml      # LLM provider + model settings (optional)
│   └── workflows/
│       └── chat.yaml    # one or more workflow DAGs
└── prompts/
    └── system.md        # system prompts, personas, templates
```

To run:

```bash
KONF_CONFIG_DIR=path/to/my-product/config konf-backend
# or
konf-mcp --config path/to/my-product/config
```

Konf v2 is **local-first**: the journal, scheduler, and runner-intent
store all live in a single embedded **redb** file managed by
`konf_runtime::KonfStorage`. No external database is required for a
working single-node deployment. Configure the file path in
`konf.toml`:

```toml
[database]
url = "redb:///var/lib/konf/konf.redb"
retention_days = 7
```

Memory backends are independent: the default `konf-tool-memory-surreal`
uses an embedded SurrealDB (RocksDB) or a remote Surreal server. The
Edge deployments can omit the `[database]` section entirely: workflows
still run, but nothing survives a restart. See
[`architecture/storage.md`](architecture/storage.md) for the full
picture and [`architecture/durability.md`](architecture/durability.md)
for the doctrine.

---

## Vocabulary

If a doc uses a term not in this table, the term is either a typo, a code-level
detail not exposed to users, or jargon that should be replaced with a concrete
reference to code or a verified finding per the "Referencing code" rule below.

| Term | What it is | Definition lives at |
|---|---|---|
| **Product** | A directory of YAML + markdown defining one konf agent. Rust type: `ProductConfig`. | `konf-init/src/config.rs` |
| **Workflow** | A DAG of nodes in YAML. Runs to completion. Optionally registered as a callable tool via `register_as_tool: true`. | `konflux-substrate/src/parser/` |
| **Node** | One step inside a workflow. Has `id`, `do`, `with`, `then`, `return`. | Workflow YAML schema |
| **Tool** | A dispatchable action. Implements the `Tool` trait. Tool names use **colons** at the kernel layer (`memory:search`, `ai:complete`, `http:get`). In workflow `do:` fields, write colons. MCP clients see underscore-translated names (`memory_search`). | `konflux-substrate/src/tool.rs` |
| **Resource** | Read-only context exposed to workflows and MCP clients. URIs like `konf://config/tools.yaml`. | `konflux-substrate/src/resource.rs` |
| **Prompt** | A parameterized template that expands into messages for an LLM call. Registered with the engine. | `konflux-substrate/src/prompt.rs` |
| **Registry** | An in-memory map keyed by name. The engine has three: tools, resources, prompts. | `konflux-substrate/src/engine.rs` |
| **ExecutionScope** (or just "scope") | What a workflow runs under: namespace, capabilities, resource limits, actor identity. Attenuates at workflow→child boundaries; never amplifies. | `konf-runtime/src/scope.rs` |
| **Namespace** | A hierarchical string identifying a tenant scope. E.g. `konf:myproduct:user_123`. Injected into memory operations by `VirtualizedTool`. | `konf-runtime/src/scope.rs` |
| **Capability** | A tool-name pattern a scope is allowed to call. E.g. `memory:*`, `ai:complete`, `*`. Checked at dispatch. | `konf-runtime/src/scope.rs` |
| **Tool guard** | A deny/allow rule on a tool's input, evaluated before namespace injection. Configured in `tools.yaml`. Distinct from capabilities: capabilities control *which tools* a scope may call; guards control *what inputs* are allowed to those tools. | `konf-runtime/src/runtime.rs` |
| **Trigger** | An entry point: maps an input source (HTTP chat, MCP call, scheduled job) to a workflow + a capability grant. Defined in `project.yaml`. | `konf-init/src/config.rs` |
| **Init product** | A product whose workflows provision external infrastructure (secrets, shared services). Boots first in deployments that need it. Not a special type — just a convention. | `products/init/` |
| **Run** | An asynchronous workflow invocation started via `runner:spawn`. Tracked in-memory by `RunRegistry` and persisted durably via `RunnerIntentStore`. Replayed from the top on restart with the same `RunId`. | `konf-tool-runner/src/registry.rs`, `konf-runtime/src/runner_intents.rs` |
| **KonfStorage** | Single redb-backed handle that owns the journal, scheduler timers, and runner intent store. One file, three logical stores. | `konf-runtime/src/storage.rs` |
| **Scheduler** | Durable timer store backed by redb. Supports `Once`, `Fixed { delay_ms }`, and `Cron { expr }` modes. Replaces the v1 tokio-timer `schedule:create` and the dead Postgres scheduling module. | `konf-runtime/src/scheduler.rs` |
| **Runner intent** | Persisted record of a `runner:spawn` call: input, scope, session id. Written before the tokio task starts, marked terminal on completion. Replayed on boot if `terminal: None`. | `konf-runtime/src/runner_intents.rs` |
| **Event bus** | `tokio::sync::broadcast` channel owned by `Runtime`. Emits `RunEvent`s for every workflow lifecycle transition, tool invocation, schedule fire, and journal append. Consumed by `/v1/monitor/stream`. | `konf-runtime/src/event_bus.rs` |
| **Interaction** | Typed envelope recording one edge-traversal in the system — a tool dispatch, workflow node lifecycle event, run lifecycle event, error, user input, or LLM response. Serialized into `JournalEntry.payload` with `event_type = "interaction"`. OpenTelemetry-aligned field naming (`id` ↔ span_id, `parent_id` ↔ parent_span_id, `trace_id` ↔ trace_id). Multi-tenant invariant: `namespace`, `actor`, and `edge_rules_fired` are inline on every record for per-row self-auditability. | `konf-runtime/src/interaction.rs` |
| **Trace id** | Optional `Uuid` on `ExecutionScope` that groups related interactions across `runner:spawn` boundaries. Inherited through `child_scope`; minted via `ensure_trace_id()` when a scope emits its first interaction without one. OTel `trace_id` analog. | `konf-runtime/src/scope.rs` |
| **FanoutJournalStore** | Journal middleware that writes each `JournalEntry` to one primary store (redb for durable audit) plus zero or more secondary stores (e.g. SurrealDB's `event` table for long-term queryable graph). Primary-succeeds-only acknowledgment; secondary failures are logged + counted, never propagated. Wired automatically by `konf-init` when both backends are configured. | `konf-runtime/src/journal/fanout.rs` |

---

## Doctrine

Three rules. Each cashes out to code or a verified finding.

1. **The Rust crates are the kernel; YAML + markdown are the configuration.**
   New Rust must be impossible to express as a workflow using existing tools.
   Validated: `konf-experiments/findings/014-only-one-new-rust-tool.md`
   showed the `schedule` tool was the only kernel addition required to prove
   autonomous agents in experiment 004. Every other capability was expressed
   as configuration.

2. **Prompts and configs replace code where possible.** If a decision, rule,
   or state change can be expressed as prompt + LLM + filesystem, don't write
   Rust. Rust requires recompilation; product configs are hot-reloadable via
   the file watcher in `crates/konf-init/src/lib.rs` (see `reload_*`
   handlers).

3. **Products are configurations, not code.** Every product is a directory
   of YAML and markdown. To ship a new product: write YAML. To update: edit
   YAML. To fork: copy the directory. No per-product Rust. The
   `ProductConfig` struct at `crates/konf-init/src/config.rs` defines the
   schema.

---

## Deprecated terms

For a short informational list of dead-name renames (e.g. `kell` → `product`, `cell` → `product`) and retired framings, see `DEPRECATED_TERMS.md` in this directory. That file is informational, not linted.

The previous "banned words / docs-lint kill list" has been removed. The real load-bearing discipline is the "Referencing code" rule below — claims cite code, findings, or explicit TBD. Mechanical word-banning proxied for that rule and caught noise rather than substance.

---

## Referencing code

Every load-bearing claim in konf's documentation must cite one of three things:

1. A Rust file + line (e.g. `crates/konf-runtime/src/scope.rs:87-99`)
2. An experimentally verified finding (e.g. `konf-experiments/findings/014-only-one-new-rust-tool.md`)
3. An explicit "not yet implemented, tracked in `<path>`" note

Docs that say "konf does X" without one of these three get cut.

---

## Status

This document is the single source of truth for konf's architecture, vocabulary,
and doctrine. It is the lint oracle: if another doc contradicts this one, the
other doc is wrong.

Maintained in this repo. Updated only when the code structure changes
(new crate, removed crate, changed primitive) or when an experimentally verified
finding demands it. Slogans, marketing, and speculative framing do not belong
here — they belong nowhere in the repo's public-facing docs.
