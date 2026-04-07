# Konf Architecture

**Status:** Authoritative
**Scope:** Platform-wide architecture and design principles

---

## What is Konf

Konf is a self-hostable, local-first operating system for AI agents. Products are configurations, not code. An agent's behavior, tools, memory, and security are defined entirely through YAML — no application code needed.

Konf runs the same engine on a phone, a laptop, a homelab server, or a cloud cluster. The agent doesn't know (or care) where it's running, what database backs its memory, or whether a tool is a compiled Rust function or an external MCP server.

---

## Core Principles

### Everything is a tool

Memory search, LLM completion, HTTP requests, embeddings, workflows — they're all tools. They implement the same interface, publish the same metadata, and are discovered the same way. The engine dispatches them identically.

### Everything does one thing well

Each crate, each tool, each spec has a single purpose. konf-backend serves HTTP. konf-mcp speaks MCP. konflux executes workflows. konf-tool-memory provides memory. They don't overlap.

### Everything is composable

Workflows call tools. Workflows ARE tools (callable from other workflows). Tools can be chained in DAGs. A Konf instance can connect to another Konf instance and use its workflows as tools. Composition is unlimited.

### Everything is configurable

Products define their tools, memory backends, capabilities, and workflows through YAML. Switching from Postgres to SurrealDB is a config change, not a code change. Adding a new MCP server is one line in tools.yaml.

### MCP-native

Konf adopts the Model Context Protocol's three primitives as first-class concepts:

| Primitive | Controlled by | Purpose | Examples |
|-----------|--------------|---------|----------|
| **Tools** | The model (LLM) | Actions — do something | memory:search, ai:complete, http:get |
| **Resources** | The application | Context — know something | config files, workflow definitions, memory schema |
| **Prompts** | The user | Instructions — how to approach something | workflow templates, system prompts |

All three have registries in the engine. All three are exposed via MCP to external clients.

### Agent-transparent

The agent sees a flat list of tools with names, descriptions, and schemas. It cannot tell whether a tool is:
- A compiled Rust function (in-process, zero overhead)
- An external MCP server (out-of-process, any language)
- A Python function (opt-in, GIL-bound)
- A workflow registered as a tool

Same interface. Same metadata. Same invocation path.

---

## The OS Analogy

Konf follows the same design patterns as Linux, Plan 9, and Fuchsia — not by metaphor, but because they solve the same fundamental problems: dispatch, isolation, composition, and security.

| OS Pattern | Konf Equivalent | Crate |
|-----------|-----------------|-------|
| Kernel | Engine — all dispatch routes through it | konflux |
| Init system (systemd) | konf-init — reads config, boots engine, registers tools | konf-init |
| Device drivers | Tools — standard interface, pluggable, hot-loadable | crates/konf-tool-* |
| VFS (filesystem abstraction) | MemoryBackend trait — pluggable storage | crates/konf-tool-memory |
| `/proc`, `/sys` | Resources — engine state readable by agents | konflux |
| Shell scripts | Prompts — parameterized templates | konflux |
| IPC (pipes, sockets) | konf-mcp — MCP protocol, separate from shell | konf-mcp |
| Shell (bash) | konf-backend — HTTP transport, separate from kernel | konf-backend |
| Linux capabilities | CapabilityGrant — fine-grained, composable permissions | konf-runtime |
| Fuchsia capability routing | child_scope() — attenuation only, never amplification | konf-runtime |
| Plan 9 per-process namespaces | VirtualizedTool — per-execution parameter injection | konf-runtime |
| cgroups | ResourceLimits — per-scope step/time/concurrency limits | konf-runtime |
| Unix pipes | StreamReceiver/StreamSender — event channels | konflux |
| systemd lifecycle | Runtime start/wait/cancel/kill | konf-runtime |
| K8s reconciliation | Zombie reconciliation — cleanup on startup | konf-runtime |

**The boundary:** Everything that's agent-facing runs through the engine (workflows, tools, resources, prompts). Everything that's infrastructure does NOT (auth, HTTP routing, MCP wire protocol, scheduling). The engine is the kernel. Transport shells are just shells.

---

## Crate Map

Every crate does one thing.

```
┌─────────────┐     ┌──────────┐
│ konf-backend │     │ konf-mcp │     ← Transport shells (HTTP / MCP)
│ (HTTP/REST)  │     │ (server) │       Either can run standalone or together
└──────┬───────┘     └────┬─────┘
       │                  │
       └────────┬─────────┘
                │
     ┌──────────▼───────────┐
     │      konf-init        │     ← Init system: reads config, boots engine,
     │  (config → engine)    │       registers tools, wires runtime
     └──────────┬───────────┘
                │
     ┌──────────▼───────────┐
     │    konf-runtime       │     ← Process management: lifecycle, scoping,
     │  (optional journal)   │       capabilities, monitoring
     └──────────┬───────────┘
                │
     ┌──────────▼───────────┐
     │   konflux engine      │     ← Kernel: workflow execution, three registries
     │  (tools, resources,   │       (ToolRegistry, ResourceRegistry, PromptRegistry)
     │   prompts)            │
     └──────────┬───────────┘
                │
  ┌─────────────┼──────────────────┐
  │             │                  │
┌─▼──────┐ ┌───▼──────┐ ┌─────────▼────────┐
│ tools  │ │  tools   │ │     tools        │
│ memory │ │  llm     │ │     mcp (client) │
│ http   │ │  embed   │ │                  │
└────────┘ └──────────┘ └──────────────────┘
```

| Crate | One thing | OS equivalent |
|-------|-----------|---------------|
| `konflux` | Execute workflows, manage registries | Kernel |
| `konf-runtime` | Process lifecycle, capabilities, scoping | Process manager |
| `konf-init` | Read config, boot engine, register tools | systemd |
| `konf-tool-memory` | Memory tools + MemoryBackend trait | VFS |
| `konf-tool-llm` | LLM completion tools (rig-core) | GPU driver |
| `konf-tool-http` | HTTP request tools (reqwest) | Network driver |
| `konf-tool-embed` | Embedding tools (fastembed, local ONNX) | Crypto driver |
| `konf-tool-mcp` | MCP client — consume external MCP servers | USB driver (external devices) |
| `konf-mcp` | MCP server — expose engine to MCP clients | IPC subsystem |
| `konf-backend` | HTTP server — REST API, auth | Shell |

---

## Deployment Combinations

The same crates compose differently for different deployment targets:

| Scenario | What runs | No transport needed? |
|----------|-----------|---------------------|
| SaaS server | konf-backend + konf-mcp + konf-init + runtime + tools | Both HTTP and MCP |
| CLI assistant | konf-mcp (stdio) + konf-init + runtime + tools | MCP only |
| Library embedding | konf-init + runtime + tools | Yes — direct Rust API |
| Phone / edge | konf-init + runtime + tools + optional konf-mcp | Optional MCP |
| Konf-to-Konf | Instance A: konf-tool-mcp → Instance B: konf-mcp | MCP between instances |

---

## Security Model: Capability Lattice

Konf uses structural security, not prompt-based trust. The LLM never sees or controls sensitive parameters like namespace.

### How it works

1. **Product config** grants capabilities to user roles:
   ```yaml
   roles:
     user:
       capabilities:
         - pattern: "memory:*"
           bindings: { namespace: "konf:myproduct:${user_id}" }
         - pattern: "ai:complete"
   ```

2. **Runtime** creates an ExecutionScope for each workflow invocation with the user's capabilities.

3. **VirtualizedTool** wraps each tool, injecting bound parameters (like `namespace`) into the input before the tool sees it. The LLM requests `memory:search(query="exercise")`. The tool receives `memory:search(query="exercise", namespace="konf:myproduct:user_123")`.

4. **Child workflows** can only attenuate capabilities, never amplify. A workflow with `memory:search` cannot grant `memory:*` to a sub-workflow. This is Fuchsia's capability routing model.

5. **ResourceLimits** enforce per-scope quotas: max steps, timeout, concurrent nodes, child depth, active runs per namespace.

6. **Namespace separator is strictly enforced.** The pattern `konf:unspool:*` matches `konf:unspool:user_123` but NOT `konf:unspool_pro:user_123`. The matching logic requires a colon boundary after the prefix — no naming tricks can bypass it.

### What this prevents

- LLM cannot access another user's data (namespace is injected, not LLM-controlled)
- Prompt injection cannot escalate privileges (capabilities are structural, not prompt-based)
- Child workflows cannot exceed parent's authority (lattice only attenuates)
- Runaway workflows are killed by resource limits (not dependent on LLM cooperation)

---

## Configuration Model

Two levels of configuration:

| Level | Format | Loaded | Purpose |
|-------|--------|--------|---------|
| **Platform** | TOML + env vars (`KONF_` prefix) | Once at startup | Server settings, auth, database URLs |
| **Product** | YAML directory | Hot-reloadable | Tools, workflows, prompts, capabilities |

Platform config (`konf.toml`):
```toml
[server]
host = "0.0.0.0"
port = 8000

[database]
url = "postgresql://localhost/konf"

[auth]
supabase_url = "http://localhost:9999"
```

Product config (`config/tools.yaml`):
```yaml
memory:
  backend: surrealdb
  config:
    dsn: "rocksdb://local.db"

llm:
  provider: anthropic
  model: claude-sonnet-4-20250514
  api_key: ${ANTHROPIC_API_KEY}

http:
  enabled: true

embed:
  enabled: true

mcp_servers:
  - name: brave
    command: npx
    args: ["-y", "@anthropic/mcp-server-brave"]
    capabilities: ["search:*"]

# Per-tool backend overrides (see memory-backends.md for details)
# overrides:
#   state:set:
#     backend: redis
#     config: { dsn: "redis://localhost" }
```

Product workflows (`config/workflows/chat.yaml`):
```yaml
workflow: chat
description: "Handle a user chat message"
register_as_tool: true
capabilities: ["memory:*", "ai:complete"]
nodes:
  search:
    do: memory:search
    input:
      query: "{{message}}"
  respond:
    do: ai:complete
    input:
      messages:
        - role: system
          content: "You are a helpful assistant."
        - role: user
          content: "{{message}}"
      context: "{{search.results}}"
    return: true
```

---

## What Konf is NOT

- **Not a framework.** You don't extend Konf with code. You configure it with YAML. Tools are plugins, not subclasses.
- **Not cloud-only.** The same engine runs on a phone (SQLite + local LLM) or a cloud cluster (Postgres + OpenAI).
- **Not LLM-specific.** Works with any model from any provider, including local models via ollama/llama.cpp.
- **Not Postgres-specific.** Memory backends are pluggable. SurrealDB, SQLite, Postgres, or a custom MCP server.
- **Not a wrapper around LangChain.** Konf is infrastructure written in Rust. No Python on the hot path. Single binary deployment.

---

## Related Specs

| Spec | What it covers |
|------|----------------|
| [konf-engine-spec](konf-engine-spec.md) | Engine internals: Tool/Resource/Prompt traits, workflow execution, capability validation |
| [konf-tools-spec](konf-tools-spec.md) | Tool protocol, plugin crate structure, tool sources |
| [konf-mcp-spec](konf-mcp-spec.md) | MCP server (konf-mcp) and MCP client (konf-tool-mcp) |
| [konf-init-spec](konf-init-spec.md) | Init system: config loading, boot sequence, KonfInstance |
| [konf-backend-spec](konf-backend-spec.md) | HTTP server shell: REST API, auth, scheduling |
| [memory-backends](memory-backends.md) | MemoryBackend trait, implementations (smrti, SurrealDB, SQLite) |
| [konf-runtime-spec](konf-runtime-spec.md) | Process management, ExecutionScope, capabilities, streaming |
| [multi-tenancy](multi-tenancy.md) | Namespace hierarchy, capability lattice, actor roles |
| [configuration-strategy](configuration-strategy.md) | Platform vs product config, hot-reload, validation |
| [workflow-yaml-schema](workflow-yaml-schema.md) | YAML format for defining workflows (nodes, edges, conditions, retries) |
| [session-state](session-state.md) | Ephemeral KV store for agent working memory |
