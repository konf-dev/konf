# Konf Plugin SDK

Konf tools are extensible through three tiers. No stakeholder writes code against Konf — tools are added, not coded.

## Tool Extensibility Model

| Tier | Mechanism | Who Can Add | Latency | Analogy |
|------|-----------|-------------|---------|---------|
| **1. Compiled Rust** | In-process, part of binary | Infra only | ~0ms | Kernel built-in drivers |
| **2. WASM Plugins** | Sandboxed, loadable at runtime | Admin | ~1-5ms | Loadable kernel modules (.ko) |
| **3. MCP Servers** | Out-of-process, any language | Admin or User | ~10-200ms | Userspace daemons |

The agent cannot tell the difference. All three tiers present the same Tool interface — same metadata, same invocation path.

## Tier 1: Compiled Rust (Available Now)

Core tools shipped with Konf. See `crates/konf-tool-*` for implementations.

These are the "kernel drivers" — memory, LLM, HTTP, embeddings, MCP client.

## Tier 2: WASM Plugins (Planned)

Drop a `.wasm` file into a plugins directory. Konf loads it at runtime in a sandboxed WASM runtime (wasmtime). No recompilation needed.

- **Language-agnostic:** Rust, Go, C, Python, JS — anything that compiles to WASM
- **Sandboxed:** Runs in a capability-constrained VM (fits Konf's security model)
- **Hot-loadable:** Load/unload without restarting Konf
- **Portable:** Same plugin runs on any hardware Konf runs on

Status: Architecture designed, implementation pending. See [docs/architecture/tool-extensibility.md](../docs/architecture/tool-extensibility.md).

## Tier 3: MCP Servers (Available Now)

Any MCP-compatible server can be connected via `tools.yaml`:

```yaml
tools:
  mcp_servers:
    - name: gmail
      command: npx
      args: ["-y", "@anthropic/mcp-server-gmail"]
```

MCP servers can be written in any language. For Python, [fastmcp](https://github.com/jlowin/fastmcp) makes it ~5 lines of code.

## The Agent Can't Tell

Whether a tool is compiled Rust, a WASM plugin, or an MCP server — the agent sees the same interface:

```json
{
  "name": "memory:search",
  "description": "Search memory for relevant context",
  "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } } }
}
```

Same metadata. Same invocation. Same result format. This is by design.
