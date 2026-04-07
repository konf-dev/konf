# Konf Integration Guide

How the crates connect, how requests flow, and how to extend the platform.

---

## Crate Dependency Graph

```
konf-backend ──┐
               ├──► konf-init ──► konf-runtime ──► konflux (engine)
konf-mcp ──────┘        │
                        ├──► konf-tool-http
                        ├──► konf-tool-llm
                        ├──► konf-tool-embed
                        ├──► konf-tool-memory ◄── konf-tool-memory-smrti
                        │                    ◄── konf-tool-memory-surrealdb
                        │                    ◄── konf-tool-memory-sqlite
                        └──► konf-tool-mcp (client)
```

**Key rule:** konf-init is the only crate that imports all tool crates. konf-backend and konf-mcp only import konf-init and get a ready-to-use `KonfInstance`.

---

## Boot Sequence

```
1. konf-backend (or konf-mcp) starts
2. Calls konf_init::boot("./config")
3. konf-init:
   a. Loads konf.toml + KONF_* env vars (platform config)
   b. Loads tools.yaml, workflows/, prompts/ (product config)
   c. Creates Engine with empty registries
   d. Calls each tool crate's register(engine, config):
      - konf_tool_memory_surrealdb::connect(config) → backend
      - konf_tool_memory::register(engine, backend)
      - konf_tool_llm::register(engine, config)
      - konf_tool_http::register(engine, config)
      - konf_tool_embed::register(engine, config)
      - konf_tool_mcp::register(engine, config)  // connects to external MCP servers
   e. Registers workflows as tools (register_as_tool: true)
   f. Registers config files as Resources, templates as Prompts
   g. Creates Runtime (engine + optional EventJournal)
   h. Returns KonfInstance { engine, runtime, config }
4. Transport-specific setup:
   - konf-backend: auth middleware, scheduling, HTTP routes, mount konf-mcp
   - konf-mcp: stdio or SSE server
5. Ready to serve
```

---

## Request Flow (HTTP)

```
Client ──POST /v1/chat──► konf-backend
                              │
                         Auth middleware (JWT → user_id, role)
                              │
                         Build ExecutionScope (namespace, capabilities, limits)
                              │
                         runtime.start_streaming(workflow, input, scope)
                              │
                         Engine executes workflow DAG:
                              │
                         ┌────┴────┐
                    Node A        Node B (concurrent if no dependency)
                    tool: memory:search    tool: ai:complete
                              │                │
                    VirtualizedTool injects    VirtualizedTool injects
                    namespace binding          namespace binding
                              │                │
                    MemoryBackend.search()     rig-core LLM call
                              │                │
                         StreamEvents ──► StreamReceiver ──► SSE to client
```

## Request Flow (MCP)

```
MCP Client ──tools/call──► konf-mcp
                              │
                         Auth (token → capabilities)
                              │
                         Look up tool in Engine.tools
                              │
                         Build ToolContext (capabilities, metadata)
                              │
                         tool.invoke(input, ctx)
                              │
                         Return MCP result
```

---

## How to Add a New Tool

### Path 1: MCP Server (zero Konf code)

Write an MCP server in any language. Configure in tools.yaml:

```yaml
mcp_servers:
  - name: myservice
    command: node
    args: ["my-mcp-server.js"]
    capabilities: ["myservice:*"]
```

konf-tool-mcp auto-discovers tools and registers them. Agent sees `myservice:query`, `myservice:update`, etc.

### Path 2: HTTP in Workflow (zero registration)

Call any API directly from workflow YAML:

```yaml
nodes:
  call_api:
    do: http:post
    input:
      url: "https://api.example.com/data"
      body: { query: "{{message}}" }
```

No tool registration needed. The `http:post` tool is already registered.

### Path 3: Rust Crate (deep integration)

Create a new crate in konf-tools:

```rust
// konf-tool-myservice/src/lib.rs
pub async fn register(engine: &Engine, config: &Value) -> anyhow::Result<()> {
    engine.register_tool(Arc::new(MyTool::new(config)?));
    Ok(())
}
```

Add it to konf-init's dependencies and boot sequence. Full access to namespace injection, streaming, capability scoping.

---

## How to Add a New Memory Backend

1. Create crate implementing `MemoryBackend` trait from `konf-tool-memory`
2. Export `connect(config) -> Result<Arc<dyn MemoryBackend>>`
3. Add to konf-init as feature-gated dependency
4. Configure in tools.yaml: `memory: { backend: mybackend, config: { ... } }`

See [memory-backends.md](../specs/memory-backends.md) for the trait definition and examples.

---

## Deployment Options

| Target | Binary | Transport | Memory Backend |
|--------|--------|-----------|---------------|
| Cloud server | konf-backend + konf-mcp | HTTP + MCP SSE | Postgres (smrti) or SurrealDB cluster |
| Homelab | konf-backend | HTTP | SurrealDB (embedded) or Postgres |
| CLI assistant | konf-mcp | stdio | SurrealDB (embedded) or SQLite |
| Mobile/edge | custom binary | optional konf-mcp | SQLite or SurrealDB (embedded) |
| Library | no binary | direct Rust API | any |

All deployments use the same konf-init boot sequence. Only the transport shell differs.
