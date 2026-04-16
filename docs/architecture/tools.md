# Konf Tools Specification

**Status:** Authoritative
**Crate:** `crates/konf-tool-*` (plugin crates in the monorepo workspace)
**Role:** Device drivers — each crate provides tools for one domain

---

## Overview

Tools are the interface between the engine and the outside world. Every external action — searching memory, calling an LLM, making an HTTP request, running a workflow — happens through a tool.

This spec defines:
1. The universal Tool trait (same for all tool sources)
2. The plugin crate structure (how tools are packaged)
3. The tool catalog (what crates exist)
4. Tool sources (Rust, MCP, Python — agent-transparent)
5. How to add new tools

---

## Tool Trait

The universal interface. Every tool — regardless of source — implements this:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    /// Metadata: name, description, schemas, capabilities, annotations
    fn info(&self) -> ToolInfo;

    /// Execute the tool. Context (namespace, actor, capabilities, trace_id,
    /// deadline, etc.) is carried inside the Envelope.
    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError>;

    /// Optional: execute with streaming output
    async fn invoke_streaming(&self, env: Envelope<Value>, sender: StreamSender) -> Result<Envelope<Value>, ToolError> {
        self.invoke(env).await
    }

    /// Optional: return a StateProjection for bisimulation testing
    fn projection(&self) -> Option<&dyn StateProjection> { None }
}
```

See [engine.md](engine.md) for full definitions of `ToolInfo`, `ToolAnnotations`, `Envelope`, and `ToolError`.

---

## Plugin Crate Structure

Each tool category is a separate crate. Every crate exports a single registration function:

```rust
/// Register this crate's tools into the engine.
/// Called by konf-init during boot based on tools.yaml.
pub async fn register(engine: &Engine, config: &Value) -> anyhow::Result<()>;
```

The `config` parameter contains the relevant section from tools.yaml. Each crate handles its own initialization (connecting to backends, loading models, spawning processes).

**Exception:** `konf-tool-memory` uses a two-step pattern: the backend crate's `connect(config)` creates the backend, then `konf_tool_memory::register(engine, backend)` registers the tools. This is because memory backends need a connected instance before tools can be created. See [memory-backends.md](memory-backends.md) for details.

### Crate layout

All tool crates live under `crates/` in the monorepo:

```
crates/
├── konf-tool-http/
│   ├── Cargo.toml                  # deps: reqwest, konflux
│   └── src/lib.rs                  # HttpGetTool, HttpPostTool, register()
├── konf-tool-llm/
│   ├── Cargo.toml                  # deps: rig-core, konflux
│   └── src/lib.rs                  # AiCompleteTool, register()
├── konf-tool-embed/
│   ├── Cargo.toml                  # deps: fastembed, konflux
│   └── src/lib.rs                  # EmbedTool, register()
├── konf-tool-memory/
│   ├── Cargo.toml                  # deps: konflux, async-trait
│   └── src/
│       ├── lib.rs                  # MemoryBackend trait, register()
│       └── tools.rs                # SearchTool, StoreTool, StateSetTool, StateGetTool
└── konf-tool-mcp/
    ├── Cargo.toml                  # deps: rmcp, konflux
    └── src/lib.rs                  # McpManager, McpToolWrapper, register()
```

> **Note:** The default memory backend is `konf-tool-memory-surreal` (SurrealDB, embedded).
> See [memory-backends.md](memory-backends.md) for details.

---

## Tool Catalog

### konf-tool-http

| Tool | Description | Annotations |
|------|-------------|-------------|
| `http:get` | HTTP GET request | open_world, idempotent |
| `http:post` | HTTP POST request with JSON body | open_world |

Backed by reqwest. Configurable max timeout (default 30s, capped at 300s). Returns status, headers, body (JSON or string).

### konf-tool-llm

| Tool | Description | Annotations |
|------|-------------|-------------|
| `ai:complete` | LLM completion with capability-enforced tool-calling loop (ReAct) | open_world, supports_streaming |

`ai:complete` is the keystone agentic tool. The kernel owns the ReAct loop — not the LLM, not application code.

**How it works:**
1. At invocation, tools are resolved dynamically from the engine's live registry
2. Only tools that pass the caller's Envelope capabilities (same lattice as the executor) are exposed to the LLM
3. An optional `tools` whitelist in `with:` further restricts visibility (AND with capabilities)
4. The LLM calls tools → kernel dispatches → feeds results back → repeats until text response or `max_iterations`
5. `ai:complete` itself is excluded from inner tools to prevent unbounded recursion (unless explicitly whitelisted)

**Streaming events emitted per iteration:**
- `Status { iteration, max }` — before each LLM call
- `ToolStart { tool, input, call_id }` — before each inner tool dispatch
- `ToolEnd { tool, call_id, duration_ms, output_preview }` — after each inner tool dispatch
- `TextDelta` — final text response

**Per-node overrides:** `model`, `temperature`, `max_tokens`, `max_iterations`, `provider` can be overridden in `with:` per workflow node.

Backed by rig-core. Supports OpenAI, Anthropic, Google, and any OpenAI-compatible API (ollama, vLLM). Provider and model configurable via tools.yaml.

### konf-tool-embed

| Tool | Description | Annotations |
|------|-------------|-------------|
| `ai:embed` | Generate text embeddings locally | read_only, idempotent |

Backed by fastembed (ONNX runtime). Runs locally — no API calls. Model configurable (default: AllMiniLML6V2).

### konf-tool-memory

| Tool | Description | Annotations |
|------|-------------|-------------|
| `memory:search` | Search the knowledge graph | read_only, idempotent |
| `memory:store` | Add nodes to the knowledge graph | |
| `state:set` | Set a session state key (working memory) | idempotent |
| `state:get` | Get a session state key | read_only, idempotent |
| `state:delete` | Delete a session state key | destructive |
| `state:list` | List all session state keys | read_only, idempotent |
| `state:clear` | Clear all session state | destructive |

Backed by a MemoryBackend implementation (see [memory-backends.md](memory-backends.md)). Backend selected via tools.yaml.

### konf-tool-shell

| Tool | Description | Annotations |
|------|-------------|-------------|
| `shell:exec` | Execute a shell command inside a Docker container sandbox | destructive, open_world |

Requires `tools.shell` config with a `container` name and optional `timeout_ms` (default: 30000). The command runs via `docker exec` against the specified container.

### konf-tool-secret

| Tool | Description | Annotations |
|------|-------------|-------------|
| `secret:get` | Read an environment variable by key (restricted to allowed keys) | read_only |
| `secret:list` | List all allowed secret key names (values not exposed) | read_only, idempotent |

Requires `tools.secret` config with an `allowed_keys` list. Only keys in the allow-list can be read.

### konf-tool-runner

| Tool | Description | Annotations |
|------|-------------|-------------|
| `runner:spawn` | Start a workflow as a background run. Returns a `RunId` immediately. | |
| `runner:status` | Check the status of a background run by `RunId`. | read_only, idempotent |
| `runner:wait` | Block until a background run completes. Returns the output. | read_only |
| `runner:cancel` | Cancel a running background workflow. | destructive |

Runs are tracked in-memory by `RunRegistry`. When `KonfStorage` is configured, spawn intents are persisted to redb for at-least-once replay on restart.

### Schedule and config tools (registered by konf-init)

| Tool | Description | Annotations |
|------|-------------|-------------|
| `schedule:create` | Create a durable timer (once, fixed-delay, or cron) to fire a workflow. | |
| `cancel:schedule` | Cancel a scheduled timer by job ID. | destructive |
| `config:reload` | Hot-reload product config (workflows, prompts, tool guards) from disk. | |

Schedule tools require a configured `[database]` section (redb storage). Without storage, `schedule:create` is still registered but the scheduler is unavailable.

### Architect tools (registered by konf-init, always available)

| Tool | Description | Annotations |
|------|-------------|-------------|
| `system:introspect` | Read-only metadata about the engine: lists all registered tools, resources, and prompts. | read_only, idempotent |
| `yaml:validate_workflow` | Parse and validate a workflow YAML string without executing it. | read_only, idempotent |

### konf-tool-mcp

Not a fixed tool set — discovers and registers tools from external MCP servers at startup. Each external tool is wrapped as a `McpToolWrapper` implementing the Tool trait.

MCP annotations (readOnly, destructive, idempotent, openWorld) are preserved and mapped to ToolAnnotations.

---

## Tool Sources

Three ways to provide tools. All produce identical ToolInfo. The agent cannot tell the difference.

### 1. Rust crate (in-process)

Direct implementation of the Tool trait. Zero serialization overhead. Used for core tools.

```rust
pub struct MyTool { /* state */ }

#[async_trait]
impl Tool for MyTool {
    fn info(&self) -> ToolInfo { /* ... */ }
    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> { /* ... */ }
}
```

**Best for:** Core platform tools, performance-critical tools, tools that need deep integration (namespace injection, streaming).

### 2. MCP server (out-of-process)

External process speaking MCP protocol. Any language. Discovered via `tools/list`, wrapped by `McpToolWrapper`.

```yaml
# tools.yaml
mcp_servers:
  - name: brave
    command: npx
    args: ["-y", "@anthropic/mcp-server-brave"]
    capabilities: ["search:*"]
```

**Best for:** Third-party integrations, tools written in other languages, existing MCP ecosystem servers.

### 3. Python function (opt-in)

Python functions loaded via PyO3 (feature-gated). Wrapped by `PyTool` adapter.

```yaml
# tools.yaml
custom:
  - name: custom:analyze
    module: tools.analyze
    function: run_analysis
    description: "Run custom analysis"
    capabilities: ["custom:analyze"]
```

**Best for:** Rapid prototyping, data science tools, custom product logic.

**Promotion path:** Prototype in Python → promote to MCP server (process isolation) → promote to Rust crate (maximum performance).

### How to add a new tool

| Path | Effort | Integration depth | When to use |
|------|--------|-------------------|-------------|
| MCP server | Zero Konf code | Auto-discovered tools, MCP annotations | Third-party, any language |
| HTTP in workflow | Zero registration | `http:post` in YAML | Simple API calls |
| Python function | Config + Python file | Custom tool name, capabilities | Prototyping |
| Rust crate | New crate in konf-tools | Full integration: streaming, namespace injection | Core tools |

---

## Tool Discovery

Tools are NOT hardcoded. They are discovered at boot time by konf-init based on tools.yaml:

1. konf-init reads tools.yaml
2. For each enabled tool section, calls the corresponding crate's `register(engine, config)`
3. For each MCP server, konf-tool-mcp connects, discovers tools, wraps and registers them
4. For each workflow with `register_as_tool: true`, creates a WorkflowTool and registers it
5. The engine's ToolRegistry now contains all tools from all sources

The agent sees a flat list. It doesn't know which tools are Rust, which are MCP, which are Python. It just sees names, descriptions, and schemas.

**Name collisions:** If two sources register a tool with the same name, the last registration wins. MCP server tools are namespaced by server name (e.g. `brave:search`) to avoid collisions with built-in tools.

---

## Related Specs

- [engine](engine.md) — Tool/Resource/Prompt traits, ToolInfo, Envelope, ToolError
- [overview](overview.md) — platform-wide architecture, crate map
- [memory-backends](memory-backends.md) — MemoryBackend trait, backend implementations
- [mcp](mcp.md) — MCP client (konf-tool-mcp) details
- [init](init.md) — boot sequence, tool registration
- [../product-guide/creating-a-product.md](../product-guide/creating-a-product.md) — tools.yaml format and product layout
