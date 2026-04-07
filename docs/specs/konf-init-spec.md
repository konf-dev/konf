# Konf Init Specification

**Status:** Authoritative
**Crate:** `konf-init`
**Role:** Init system (systemd) — reads config, boots engine, registers tools, wires runtime

---

## Overview

konf-init is the shared bootstrap crate. It reads configuration, creates the engine, registers all tools/resources/prompts, and wires the runtime. Both konf-backend and konf-mcp use it — no duplicated initialization logic.

Like Linux's systemd: it reads declarative config files and boots the system. Transport shells (HTTP, MCP) then serve their respective protocols over the booted instance.

---

## KonfInstance

The output of a successful boot:

```rust
pub struct KonfInstance {
    /// The engine with all tools, resources, and prompts registered
    pub engine: Arc<Engine>,

    /// The runtime with process management and optional journal
    pub runtime: Arc<Runtime>,

    /// The loaded and validated configuration
    pub config: Arc<PlatformConfig>,

    /// Product config (tools.yaml, workflows, prompts) — hot-reloadable
    pub product_config: Arc<ArcSwap<ProductConfig>>,
}
```

---

## Boot Sequence

```rust
pub async fn boot(config_path: &Path) -> anyhow::Result<KonfInstance>;
```

1. **Load platform config** — `konf.toml` + `KONF_*` env vars via figment. Fail-fast on parse errors.
2. **Validate platform config** — all required fields present, sane values (non-zero ports, valid URLs). Fail-fast on validation errors.
3. **Load product config** — read `tools.yaml`, `workflows/`, `prompts/` from config directory.
4. **Validate product config** — all referenced backends exist, capability patterns are valid, workflow YAML parses. Fail-fast on invalid config.
5. **Create engine** — `Engine::with_config(engine_config)` with three empty registries.
6. **Register builtin tools** — echo, json_get, concat, log, template.
7. **Register tools from tools.yaml** — for each enabled section:
   - `memory` → call `konf_tool_memory_*::connect(config)` then `konf_tool_memory::register(engine, backend)`
   - `llm` → call `konf_tool_llm::register(engine, config)`
   - `http` → call `konf_tool_http::register(engine, config)`
   - `embed` → call `konf_tool_embed::register(engine, config)`
   - `mcp_servers` → call `konf_tool_mcp::register(engine, config)`
   - `custom` (Python) → call `konf_tool_python::register(engine, config)` (if feature enabled)
8. **Register workflows as tools** — for each `.yaml` file in `workflows/` with `register_as_tool: true`, create a `WorkflowTool` and register it.
9. **Register resources** — config files, workflow definitions, memory schema as readable Resources.
10. **Register prompts** — templates from `prompts/` directory as expandable Prompts.
11. **Create runtime** — `Runtime::new(engine, optional_journal)`. If database URL is configured, create EventJournal and run zombie reconciliation. If no database, journal is None.
12. **Return KonfInstance** — all registries populated, runtime ready.

---

## Config Hot-Reload

```rust
impl KonfInstance {
    /// Reload product config (tools.yaml, workflows/, prompts/).
    /// Platform config (konf.toml) is NOT reloadable — requires restart.
    pub async fn reload(&self) -> Result<(), Vec<String>>;
}
```

Reload behavior:
- Re-reads product config files from disk
- Validates new config (returns errors if invalid, keeps old config)
- Swaps product config atomically via ArcSwap
- Re-registers workflows (add new, update changed, remove deleted)
- Updates resource registry (config file contents)
- **Tool toggling:** if a tool section is added/removed in tools.yaml, the corresponding tools are registered/unregistered in the engine's ToolRegistry. The registry supports thread-safe add and remove. This means enabling/disabling tools (e.g. turning off `http:*` for a restricted product) takes effect without restart.
- Does NOT reconnect memory backends or restart MCP server processes. To change backend DSN or add a new MCP server, restart is required.

**Why not reconnect backends on reload?** Reconnecting a database or respawning MCP processes mid-flight risks dropping in-progress workflow tool calls. Backends are long-lived connections — changing them is a deployment operation, not a config tweak. Tool toggling (which tools are *available*) is safe because it only affects future workflow starts, not in-progress executions (which have their own per-execution engine snapshot).

---

## Dependencies

konf-init is the **only crate that imports all tool crates**:

```toml
# konf-init/Cargo.toml
[dependencies]
konflux = { path = "../konflux-core" }
konf-runtime = { path = "../konf-runtime" }

# Tool crates (all in this monorepo)
konf-tool-http = { path = "../konf-tool-http" }
konf-tool-llm = { path = "../konf-tool-llm" }
konf-tool-embed = { path = "../konf-tool-embed" }
konf-tool-memory = { path = "../konf-tool-memory" }
konf-tool-mcp = { path = "../konf-tool-mcp" }

# Config
figment = { ..., features = ["toml", "env", "yaml"] }
arc-swap = "1"

# Database (feature-gated)
sqlx = { ..., optional = true }

[features]
default = ["postgres"]
postgres = ["sqlx"]
```

> **Note:** Memory backend implementations (konf-tool-memory-smrti, konf-tool-memory-surrealdb,
> konf-tool-memory-sqlite) are **external dependencies**, not part of this monorepo. They live in
> their own repos (e.g., konf-dev/smrti) and are added as git or registry dependencies when needed.

---

## Usage by Transport Shells

### konf-backend

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let instance = konf_init::boot(Path::new("./config")).await?;

    // Backend-specific setup
    let auth = setup_auth(&instance.config.auth);
    let scheduler = setup_scheduler(&instance.config, &instance.runtime);

    // Mount konf-mcp if enabled
    let mcp_handler = if instance.config.mcp.enabled {
        Some(KonfMcpServer::new(instance.engine.clone(), instance.runtime.clone()))
    } else {
        None
    };

    // Build HTTP router and serve
    let app = build_router(instance, auth, scheduler, mcp_handler);
    axum::serve(listener, app).await?;
}
```

### konf-mcp (standalone)

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let instance = konf_init::boot(Path::new("./config")).await?;
    let server = KonfMcpServer::new(instance.engine.clone(), instance.runtime.clone());
    server.serve_stdio().await?;
}
```

### Library embedding

```rust
let instance = konf_init::boot(Path::new("./config")).await?;
// Use instance.engine and instance.runtime directly
let workflow = instance.runtime.parse_yaml(&yaml)?;
let result = instance.runtime.run(&workflow, input, scope, session_id).await?;
```

---

## Related Specs

- [konf-architecture](konf-architecture.md) — crate map, init system role
- [konf-engine-spec](konf-engine-spec.md) — Engine struct, registries
- [konf-tools-spec](konf-tools-spec.md) — tool crate `register()` functions
- [konf-runtime-spec](konf-runtime-spec.md) — Runtime creation, optional journal
- [memory-backends](memory-backends.md) — MemoryBackend trait, backend connect() functions
- [configuration-strategy](configuration-strategy.md) — config file formats, validation, hot-reload
