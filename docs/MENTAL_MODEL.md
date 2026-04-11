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
| `konflux-core` | Workflow execution engine. Three registries: tools, resources, prompts. Parses YAML workflows, executes them as DAGs, enforces capabilities at dispatch. Zero I/O. | `Engine` in `crates/konflux-core/src/engine.rs` |
| `konf-runtime` | Process manager. Wraps the engine with `ExecutionScope` (namespace, capabilities, limits, actor), process table, `VirtualizedTool` for namespace injection, `GuardedTool` for deny/allow rules, attenuation-only capability lattice. | `Runtime` in `crates/konf-runtime/src/runtime.rs` |
| `konf-init` | Bootstrap. Reads platform config (`konf.toml`) and product config (`tools.yaml`, `workflows/`, `prompts/`), interpolates env vars in YAML, registers built-in and configured tools, connects to Postgres if configured, creates the runtime, registers workflows as tools, applies tool guards. See `docs/architecture/init.md` for the full boot sequence. | `boot(config_dir)` in `crates/konf-init/src/lib.rs` |
| `konf-backend` | HTTP transport. `POST /v1/chat` with SSE streaming. Optional scheduler backed by Postgres. | `crates/konf-backend/src/main.rs` |
| `konf-mcp` | MCP transport. Exposes workflows as MCP tools over stdio or SSE. Translates tool names at the wire: kernel `foo:bar` → MCP `foo_bar` (required by MCP spec SEP-986). | `crates/konf-mcp/src/main.rs` |
| `konf-tool-*` | Plugin crates registering built-in tools: `http`, `llm`, `embed`, `memory`, `mcp` (client), `shell`, `secret`, `runner`. | `crates/konf-tool-*/src/lib.rs` |
| `konf-init-kell` | CLI that scaffolds a new product directory. Binary name is vestigial; the term it refers to ("kell") is deprecated. | `crates/konf-init-kell/src/main.rs` |

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
- **`konf-tool-memory-smrti`** is opt-in behind the `memory-smrti` feature.
  Backed by [smrti](https://github.com/konf-dev/smrti), a separate Rust crate
  storing nodes, edges, and embeddings in Postgres + pgvector. Requires SSH
  access to the private smrti repository at build time.

Both backends are dumb storage layers: they do not call LLMs or generate
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

Postgres with pgvector is required for memory-backed products. Everything else
(scheduler, journal) is optional and degrades gracefully.

---

## Vocabulary

If a doc uses a term not in this table, the term is either a typo, a code-level
detail not exposed to users, or jargon that belongs in the kill list below.

| Term | What it is | Definition lives at |
|---|---|---|
| **Product** | A directory of YAML + markdown defining one konf agent. Rust type: `ProductConfig`. | `konf-init/src/config.rs` |
| **Workflow** | A DAG of nodes in YAML. Runs to completion. Optionally registered as a callable tool via `register_as_tool: true`. | `konflux-core/src/parser/` |
| **Node** | One step inside a workflow. Has `id`, `do`, `with`, `then`, `return`. | Workflow YAML schema |
| **Tool** | A dispatchable action. Implements the `Tool` trait. Tool names use **colons** at the kernel layer (`memory:search`, `ai:complete`, `http:get`). In workflow `do:` fields, write colons. MCP clients see underscore-translated names (`memory_search`). | `konflux-core/src/tool.rs` |
| **Resource** | Read-only context exposed to workflows and MCP clients. URIs like `konf://config/tools.yaml`. | `konflux-core/src/resource.rs` |
| **Prompt** | A parameterized template that expands into messages for an LLM call. Registered with the engine. | `konflux-core/src/prompt.rs` |
| **Registry** | An in-memory map keyed by name. The engine has three: tools, resources, prompts. | `konflux-core/src/engine.rs` |
| **ExecutionScope** (or just "scope") | What a workflow runs under: namespace, capabilities, resource limits, actor identity. Attenuates at workflow→child boundaries; never amplifies. | `konf-runtime/src/scope.rs` |
| **Namespace** | A hierarchical string identifying a tenant scope. E.g. `konf:myproduct:user_123`. Injected into memory operations by `VirtualizedTool`. | `konf-runtime/src/scope.rs` |
| **Capability** | A tool-name pattern a scope is allowed to call. E.g. `memory:*`, `ai:complete`, `*`. Checked at dispatch. | `konf-runtime/src/scope.rs` |
| **Tool guard** | A deny/allow rule on a tool's input, evaluated before namespace injection. Configured in `tools.yaml`. Distinct from capabilities: capabilities control *which tools* a scope may call; guards control *what inputs* are allowed to those tools. | `konf-runtime/src/runtime.rs` |
| **Trigger** | An entry point: maps an input source (HTTP chat, MCP call, scheduled job) to a workflow + a capability grant. Defined in `project.yaml`. | `konf-init/src/config.rs` |
| **Init product** | A product whose workflows provision external infrastructure (Postgres, secrets). Boots first in deployments that need it. Not a special type — just a convention. | `products/init/` |
| **Run** | An asynchronous workflow invocation started via `runner:spawn`. Tracked in `RunRegistry` by a `RunId`; queried via `runner:status`/`runner:wait`; aborted via `runner:cancel`. Ships with an `InlineRunner` backend today; `SystemdRunner` and `DockerRunner` are planned. | `konf-tool-runner/src/registry.rs` |

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

## Banned words (docs-lint kill list)

If any of these appear in any doc or config file in the konf repos, the
docs-lint hook fails. Replace with a concrete description that points at code
or an experimentally verified finding.

- **"The Grand Experiment"** — mantra with no operational cash-out
- **"autonomous agent civilization"** — aspirational framing, no code
- **"civilization primitives"** — undefined
- **"honeycomb primitives"** — referenced, never defined
- **"PID 1"** — analogy; konf is not a Unix kernel with a process table at boot
- **"Hovercraft", "Construct", "Zion", "Operator", "Operatives"** — Matrix metaphor; replace with the actual term (product, runtime, namespace, etc.)
- **"cell"** — concept dropped; use "product"
- **"kell"** — deprecated term; use "product" (may be reintroduced later if it earns its keep)
- **"Second-in-Command"** — marketing framing with no operational definition
- **"self-modification"**, **"self-healing bureaucracy"** — no code implements this
- **"operational honesty"** (as emotional LLM error messages) — no code; the real principle is "errors propagate loudly; runtime events are journaled in `runtime_events`"
- **"smart bubble"** — metaphor without purpose
- **"Building Box"** — vague brainstorm
- **"proper abstractions"**, **"first principles"** — slogans without cash-out; replace with the concrete thing
- **"production-grade"**, **"best industry practices"** — vague; replace with: tests pass, clippy clean, no `.unwrap()` in production code, errors propagated via `?`
- **"Broker"** as a doctrine term — not a concept; may still appear as a persona prompt file inside one specific product (e.g. `products/my-product/prompts/broker.md`)
- **"curiosity"**, **"freedom"**, **"quality"** as a doctrine triplet — founder's orientation, not technical doctrine

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
here — they belong in the kill list above.
