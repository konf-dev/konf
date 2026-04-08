# Konf Master Implementation Plan

**Status:** Active
**Architecture:** See [overview.md](../architecture/overview.md)

---

## Phase Summary

| Phase | Status | Description |
|-------|--------|-------------|
| A-D | **Done** | Core engine, runtime, tools, transports (10 crates, 165 tests) |
| G1-G2 | **Done** | CI, Docker |
| **E1** | **Next** | Shell tool + sandbox container — agent gets OS access |
| **E2** | Next | Extend ai_complete for agent-mode (tool selection in workflow YAML) |
| **E3** | Next | products/architect — first product bootstraps the ecosystem |
| E4 | After E3 | SQLite backend — prove portability |
| E5 | After E3 | WASM adapter — prove open architecture |
| G3 | Planned | OpenTelemetry, OpenAPI docs |

> **Monorepo migration complete.** All 10 crates now live under `crates/` in a single Cargo workspace
> (`konf`). The license is BSL-1.1. smrti remains an external dependency at konf-dev/smrti.

---

## Phase C: Engine Foundation

Transform the engine from tool-only to MCP-native with three registries. Extract tools from konf-backend into konf-tools crates. Create the shared init system.

### C1: Enrich ToolInfo
- Add `output_schema: Option<Value>` and `annotations: ToolAnnotations` to ToolInfo
- Add `ToolAnnotations { read_only, destructive, idempotent, open_world }`
- Update all existing Tool implementations to set annotations
- **Files:** `crates/konflux-core/src/tool.rs`, `crates/konflux-core/src/builtin.rs`

### C2: Resource and Prompt traits
- Add `Resource` trait with `info()`, `read()`, `subscribe()`
- Add `Prompt` trait with `info()`, `expand(args)`
- Add `ResourceRegistry` and `PromptRegistry` to Engine
- **Files:** `crates/konflux-core/src/resource.rs`, `crates/konflux-core/src/prompt.rs`, `crates/konflux-core/src/engine.rs`

### C3: Extract tools to konf-tools
- Create konf-tools workspace with crates:
  - `konf-tool-http` (from konf-backend/src/tools/http.rs)
  - `konf-tool-llm` (from konf-backend/src/tools/llm.rs)
  - `konf-tool-embed` (from konf-backend/src/tools/embed.rs)
  - `konf-tool-mcp` (from konf-backend/src/tools/mcp.rs)
- Each crate exports `register(engine, config) -> Result<()>`
- **Files:** `crates/konf-tool-*`

### C4: MemoryBackend trait + smrti wrapper
- Create `konf-tool-memory` with MemoryBackend trait and tool shells
- Create `konf-tool-memory-smrti` wrapping existing smrti::Memory
- **Files:** `crates/konf-tool-*`

### C5: WorkflowTool
- Workflows with `register_as_tool: true` register as `workflow_{name}` tools
- WorkflowTool wraps workflow + runtime, creates child scope
- **Files:** `crates/konf-runtime/src/workflow_tool.rs`

### C6: konf-init
- Create the shared bootstrap crate
- `boot(config_path) → KonfInstance` with full wiring
- Config hot-reload via ArcSwap
- Feature-gated memory backend selection
- **Files:** `crates/konf-init/`

---

## Phase D: Transport Shells

Build the two transport shells over the booted engine.

### D1: konf-mcp (MCP server)
- Standalone crate: reads engine registries, serves MCP wire protocol
- Supports stdio and SSE transports
- Can run standalone or mounted in konf-backend
- **Files:** `crates/konf-mcp/`

### D2: konf-backend rewrite
- Thin HTTP shell using konf-init for bootstrap
- Remove all tool implementations (already extracted in C3)
- Remove smrti dependency
- Auth, scheduling, graceful shutdown
- **Files:** `crates/konf-backend/src/main.rs` and route handlers

### D3: SSE streaming
- Implement `Runtime::start_streaming()` returning (RunId, StreamReceiver)
- Pipe StreamEvent → SSE events in chat endpoint
- Replace 100ms poll loop
- **Files:** `crates/konf-runtime/src/runtime.rs`, `crates/konf-backend/src/api/chat.rs`

### D4: Admin + Monitoring API
- GET /v1/messages (conversation history)
- GET/PUT /v1/admin/config (hot reload via konf-init)
- GET /v1/admin/audit (event journal)
- **Files:** `crates/konf-backend/src/api/`

---

## Phase E: The Architect

The next phase shifts from building infrastructure to using it. The architect agent is the first Konf product — an agent that designs, builds, and maintains other Konf products.

### E1: Shell tool + sandbox container

Give agents OS access through a `shell_exec` tool that runs commands inside a sandboxed container. The tool accepts a command string and returns stdout/stderr. The container is ephemeral, network-isolated, and resource-limited. This is the prerequisite for any agent that needs to read files, run tests, or execute builds.

### E2: Agent-mode ai_complete

Extend `ai_complete` to support tool-use loops. The workflow YAML declares which tools the LLM may call during completion. The runtime manages the tool-call/response cycle, enforcing capability checks on every tool invocation. This turns a single-shot LLM call into an agentic loop — controlled by the kernel, not by application code.

### E3: products/architect

The first Konf product. The architect agent reads specs, writes YAML workflows, validates them against the engine, and iterates. It bootstraps the ecosystem: every future product can be designed by the architect. This proves that self-modification is safe (YAML only, validated, capability-bounded).

### E4: SQLite backend

Implement `MemoryBackend` for SQLite + sqlite-vec + FTS5. Proves the backend-agnostic storage abstraction with a second concrete implementation. Enables edge and mobile deployments.

### E5: WASM adapter

Prove the open architecture by loading tools from WASM modules. Any language that compiles to WASM can extend Konf without recompilation.

---

## Phase G: Production

### G1: CI/CD
- GitHub Actions: build, test, clippy, fmt across all crates
- Pin git dependencies to specific commits

### G2: Docker
- Dockerfile: single binary (konf-backend with konf-mcp)
- docker-compose: pgvector + supabase-auth + konf-backend

### G3: Monitoring + Docs
- OpenAPI docs via utoipa
- Structured tracing with optional OTEL export
- Health check with DB connectivity probe
