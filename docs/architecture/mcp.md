# Konf MCP Specification

**Status:** Authoritative
**Crates:** `konf-mcp` (server) + `konf-tool-mcp` (client)
**Role:** IPC — bidirectional MCP communication (expose Konf to clients, consume external servers)

---

## Overview

Konf speaks MCP (Model Context Protocol) in both directions:

1. **konf-mcp** (server) — exposes Konf's tools, resources, and prompts to MCP clients (Claude Desktop, Cursor, other Konf instances)
2. **konf-tool-mcp** (client) — connects to external MCP servers (Brave, GitHub, Slack, etc.), discovers their tools, wraps them as Konf tools

These are separate crates with separate concerns. konf-mcp is a transport shell (like konf-backend). konf-tool-mcp is a tool source (like konf-tool-http).

---

## konf-mcp: MCP Server

### What it does

Takes references to the engine's three registries and translates them to the MCP wire protocol. Any MCP client connecting to konf-mcp gets access to everything registered in the engine.

### API

```rust
pub struct KonfMcpServer {
    engine: Arc<Engine>,
    runtime: Arc<Runtime>,
}

impl KonfMcpServer {
    pub fn new(engine: Arc<Engine>, runtime: Arc<Runtime>) -> Self;

    /// Serve MCP over stdio (for CLI / Claude Desktop)
    pub async fn serve_stdio(&self) -> anyhow::Result<()>;

    /// Serve MCP over SSE (for remote clients)
    pub async fn serve_sse(&self, listener: TcpListener) -> anyhow::Result<()>;

    /// Get an axum handler for mounting alongside HTTP routes
    pub fn sse_handler(&self) -> axum::Router;
}
```

### What's exposed

**Tools** — all registered tools:
- Memory tools: `memory:search`, `memory:store`, `state:*`
- LLM tools: `ai:complete`
- HTTP tools: `http:get`, `http:post`
- Embed tools: `ai:embed`
- Workflow tools: `workflow:chat`, `workflow:summarize`, etc.
- MCP-forwarded tools: `brave:search`, `github:create_issue`, etc.

Each tool's `ToolInfo` is translated to MCP's tool definition format. Annotations map directly:
- `read_only` → `readOnlyHint`
- `destructive` → `destructiveHint`
- `idempotent` → `idempotentHint`
- `open_world` → `openWorldHint`

**Resources** — all registered resources:
- Product config files (`konf://config/tools.yaml`)
- Workflow definitions (`konf://workflows/chat.yaml`)
- Memory schema (if backend exposes it)
- Audit journal summary (`konf://audit/recent`)

**Prompts** (planned — not yet implemented):
- Workflow templates from prompts/ directory
- System prompts per product mode

### MCP protocol mapping

| MCP method | konf-mcp handler | Status |
|-----------|-----------------|--------|
| `tools/list` | Read ToolRegistry, map ToolInfo → MCP tool definitions | Implemented |
| `tools/call` | Look up tool by name, build ToolContext, call `tool.invoke()` | Implemented |
| `resources/list` | Read ResourceRegistry, map ResourceInfo → MCP resource definitions | Implemented |
| `resources/read` | Look up resource by URI, call `resource.read()` | Implemented |
| `prompts/list` | Read PromptRegistry, map PromptInfo → MCP prompt definitions | Planned (Phase E+) |
| `prompts/get` | Look up prompt by name, call `prompt.expand(args)` | Planned (Phase E+) |

### Transports

| Transport | Use case | How to start |
|-----------|----------|-------------|
| stdio | CLI, Claude Desktop, local development | `konf --mcp-stdio` |
| SSE over HTTP | Remote MCP clients, Konf-to-Konf | Mounted in konf-backend at `/mcp` |

### Capability scoping

MCP clients get capabilities based on authentication:
- Unauthenticated (local stdio): configurable default capabilities
- Authenticated (SSE with token): capabilities from user's role, same lattice as HTTP API
- Tools not granted are not listed in `tools/list` response

### Standalone operation

konf-mcp can run without konf-backend:

```bash
# CLI mode — Claude Desktop connects via stdio
konf --mcp-stdio --config ./config

# Remote mode — MCP over SSE, no REST API
konf --mcp-sse --port 3001 --config ./config
```

Both modes use konf-init to boot the engine. No HTTP server, no auth middleware, no scheduling.

---

## konf-tool-mcp: MCP Client

### What it does

Connects to external MCP servers, discovers their tools, wraps each as a Konf `Tool` trait object, and registers them in the engine. It's a tool source, not a server.

### Registration

```rust
// konf-tool-mcp/src/lib.rs
pub async fn register(engine: &Engine, config: &Value) -> anyhow::Result<()> {
    let servers: Vec<McpServerConfig> = serde_json::from_value(config.clone())?;
    let manager = McpManager::new(servers);
    manager.discover_and_register(engine).await?;
    Ok(())
}
```

### Configuration

```yaml
# tools.yaml
mcp_servers:
  - name: brave
    transport: stdio
    command: npx
    args: ["-y", "@anthropic/mcp-server-brave"]
    env:
      BRAVE_API_KEY: ${BRAVE_API_KEY}
    capabilities: ["brave:*"]
    idle_timeout: 600

  - name: github
    transport: stdio
    command: npx
    args: ["-y", "@anthropic/mcp-server-github"]
    env:
      GITHUB_TOKEN: ${GITHUB_TOKEN}
    capabilities: ["github:*"]
```

### McpToolWrapper

Each discovered MCP tool is wrapped:

```rust
struct McpToolWrapper {
    name: String,           // "brave:search"
    description: String,
    input_schema: Value,
    output_schema: Option<Value>,
    annotations: ToolAnnotations,  // Preserved from MCP server
    server_name: String,
    tool_name: String,
    client: McpClient,  // rmcp::service::Peer<RoleClient>
}

impl Tool for McpToolWrapper {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            output_schema: self.output_schema.clone(),
            capabilities: vec![self.name.clone()],
            supports_streaming: false,
            annotations: self.annotations.clone(),
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        // JSON-RPC tools/call to MCP server
        // Returns structured content
    }
}
```

### Annotation mapping

MCP server annotations are preserved:

| MCP annotation | Konf ToolAnnotations field |
|---------------|--------------------------|
| `readOnlyHint` | `read_only` |
| `destructiveHint` | `destructive` |
| `idempotentHint` | `idempotent` |
| `openWorldHint` | `open_world` |

### Capability filtering

Only tools matching the configured capability patterns are registered:

```yaml
capabilities: ["brave:*"]  # Only register tools matching brave:*
```

If `capabilities` is empty, all discovered tools are registered.

### Process lifecycle

- MCP servers are spawned at startup via `tokio::process::Command`
- Client handles stored in `McpManager` (not leaked via `mem::forget`)
- On graceful shutdown (SIGTERM): send SIGTERM to all child processes, wait 5s, SIGKILL remaining
- Environment variables resolved from `${VAR}` syntax with warning on missing vars

---

## Composability: Konf-to-Konf

Two Konf instances can compose:

```
Instance A                         Instance B
┌──────────┐                      ┌──────────┐
│ engine   │  konf-tool-mcp       │ konf-mcp │
│ (tools,  │ ◄──── MCP ─────────►│ (server) │
│  wf:*)   │  (client)           │          │
└──────────┘                      └──────────┘
```

Instance A's tools.yaml:
```yaml
mcp_servers:
  - name: instance-b
    transport: sse
    url: "http://instance-b:3001/mcp"
    capabilities: ["workflow:*"]
```

Now Instance A's agents can call `workflow:summarize` which actually executes on Instance B. Transparent to the agent.

---

## Related Specs

- [overview](overview.md) — platform-wide architecture, MCP as IPC
- [engine](engine.md) — Tool/Resource/Prompt traits
- [tools](tools.md) — McpToolWrapper as a tool source
- [init](init.md) — boot sequence (konf-mcp uses konf-init)
- [backend](backend.md) — mounts konf-mcp SSE handler
