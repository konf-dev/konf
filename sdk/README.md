# Konf Plugin SDK

No stakeholder writes code against Konf — tools are added, not coded.

## One Interface, Many Adapters

Every tool in Konf implements the same `Tool` trait. The engine doesn't know or care how a tool is implemented — it dispatches all tools identically.

An **adapter** wraps an execution environment behind this interface. Konf ships several adapters, and the architecture supports any number more.

### Shipped Adapters

| Adapter | How it works | Who can add tools | Status |
|---------|-------------|-------------------|--------|
| **Compiled Rust** | In-process, part of binary | Infra (requires compilation) | Available |
| **MCP client** | Out-of-process, stdio/SSE | Admin or User (config change) | Available |
| **HTTP** | Network call to external API | Admin (config change) | Available |
| **WASM** | Sandboxed runtime (wasmtime) | Admin (drops `.wasm` file) | Planned |

The architecture is open — anyone can write a new adapter (gRPC, Unix sockets, FFI, Python subprocess, etc.) by implementing the `Tool` trait.

## Compiled Rust (Available Now)

Core tools shipped with Konf. See `crates/konf-tool-*` for implementations.

## MCP Servers (Available Now)

Any MCP-compatible server can be connected via `tools.yaml`:

```yaml
tools:
  mcp_servers:
    - name: gmail
      command: npx
      args: ["-y", "@anthropic/mcp-server-gmail"]
```

MCP servers can be written in any language. For Python, [fastmcp](https://github.com/jlowin/fastmcp) makes it ~5 lines of code.

## WASM Plugins (Planned)

Drop a `.wasm` file into a plugins directory. Konf loads it at runtime in a sandboxed WASM runtime (wasmtime). No recompilation needed.

- **Language-agnostic:** Rust, Go, C, Python, JS — anything that compiles to WASM
- **Sandboxed:** Runs in a capability-constrained VM (fits Konf's security model)
- **Hot-loadable:** Load/unload without restarting Konf
- **Portable:** Same plugin runs on any hardware Konf runs on

Status: Architecture designed, implementation pending. See [docs/architecture/tools.md](../docs/architecture/tools.md) for the current tool protocol and adapter model.

## The Agent Can't Tell

Whatever adapter backs a tool, the agent sees the same interface:

```json
{
  "name": "memory_search",
  "description": "Search memory for relevant context",
  "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } } }
}
```

Same metadata. Same invocation. Same result format. This is an architectural property, not a convenience — the engine dispatches all tools through the same codepath.
