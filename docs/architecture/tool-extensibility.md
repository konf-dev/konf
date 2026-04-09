# Tool Extensibility

**Status:** Authoritative
**Scope:** How tools are added to Konf — one interface, many adapters

---

## One Interface

Every tool in Konf implements the same trait:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError>;
}
```

The engine dispatches `do: memory:search` and `do: mcp:github:list_issues` through identical codepaths. The `VirtualizedTool` wrapper handles namespace injection and capability checking regardless of how the tool is implemented.

This means:
- The agent cannot tell how a tool is implemented. It sees the same metadata, same schema, same invocation.
- Workflow authors don't care where a tool runs. `do: tool:name` works the same whether the tool is compiled Rust, a WASM module, an MCP server, or something else entirely.
- A tool can be moved between adapters without changing any workflow YAML.
- Security policies apply uniformly across all adapters.

---

## Tool Adapters

An adapter wraps an execution environment behind the Tool trait. Konf ships several, but the architecture supports any number — anyone can write a new adapter.

### Shipped Adapters

| Adapter | How it works | Who can add tools | Status |
|---------|-------------|-------------------|--------|
| **Compiled Rust** | In-process, part of the binary | Infra (requires compilation) | Available |
| **MCP client** | Out-of-process, stdio/SSE | Admin or User (config change) | Available |
| **HTTP** | Network call to external API | Admin (config change) | Available |
| **WASM** | Sandboxed runtime (wasmtime) | Admin (drops `.wasm` file) | Planned |

### Possible Future Adapters

The architecture imposes no limit. Examples of adapters anyone could build:

- gRPC adapter — wraps a gRPC service as a Tool
- Unix socket adapter — IPC via domain sockets
- Python subprocess — runs a Python script, captures output
- FFI adapter — calls a shared library (.so/.dylib)
- Konf-to-Konf — one Konf instance's tools exposed to another (already works via MCP)

Each adapter is just a struct that implements `Tool`. The engine doesn't know or care about the implementation.

---

## Shipped Adapter Details

### Compiled Rust (In-Process)

Built-in tools compiled into the Konf binary. Located in `crates/konf-tool-*`.

| Crate | Tools |
|-------|-------|
| `konf-tool-memory` | memory:store, memory:search, memory:delete, memory:traverse |
| `konf-tool-llm` | ai:complete |
| `konf-tool-http` | http:get, http:post |
| `konf-tool-embed` | ai:embed |
| `konf-tool-mcp` | MCP client (connects to MCP servers) |
| `konflux-core` | echo (builtin) |

Adding a compiled tool requires modifying the Rust codebase and recompiling. This is the only adapter type that requires code changes.

Cross-platform builds use feature flags:

```toml
[features]
default = ["full"]
full = ["memory", "llm", "http", "embed", "mcp"]
edge = ["llm", "http"]       # minimal build for edge/embedded
```

### MCP Client (Out-of-Process)

MCP servers run as child processes or connect over SSE. Configured in `tools.yaml`:

```yaml
mcp_servers:
  github:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_TOKEN: "${GITHUB_TOKEN}"
```

- Communicate via stdio using the MCP protocol
- Lifecycle managed by Konf (started on demand, terminated with the product)
- Any MCP-compatible server works — the ecosystem is large and growing
- Tools appear as `mcp:<server>:<tool_name>` in workflows

### WASM (Sandboxed, Planned)

WASM plugins run inside a `wasmtime` sandbox with explicit capability grants:

```yaml
# Planned config (not yet implemented)
plugins:
  sentiment:
    wasm: ./plugins/sentiment.wasm
    capabilities:
      - "ai:complete"
    memory_limit: 64MB
    timeout_ms: 5000
```

Why WASM is interesting as an adapter:
- **Sandboxed** — no filesystem, network, or syscall access unless explicitly granted
- **Capability-restricted** — receives only the capabilities listed in its config
- **Portable** — compiled once, runs on any platform Konf supports
- **Hot-loadable** — load/unload without restarting Konf
- **Language-agnostic** — Rust, Go, C, Python (componentize-py), JS all compile to WASM

---

## Cross-Platform Compilation

Same Rust codebase, different targets with feature flags:

```
Same source → x86_64-linux (server)
            → aarch64-linux (ARM/RPi)
            → aarch64-apple (macOS)
            → aarch64-ios (iOS framework)
            → aarch64-android (Android NDK)
```

What changes per platform:

| Concern | Server/Desktop | Mobile |
|---------|---------------|--------|
| Memory backend | Postgres, SurrealDB | SQLite |
| LLM | Remote API | Local (llama.cpp) or remote |
| MCP servers | Spawn child processes | Can't spawn arbitrary processes |
| WASM plugins | wasmtime | wasmtime (works on mobile) |
| Transport | HTTP server + MCP | In-process library API |

The engine (`konflux`) and runtime don't change. Only `konf-init` wires different adapters based on the target.

---

## Summary

```
┌───────────────────────────────────────────────┐
│              Workflow Engine                   │
│         (one Tool trait for all)               │
├───────────┬──────┬──────┬──────┬──────────────┤
│ Compiled  │ MCP  │ HTTP │ WASM │  ... any     │
│ Rust      │      │      │      │  future      │
│           │      │      │      │  adapter     │
└───────────┴──────┴──────┴──────┴──────────────┘
```

---

## Related Specs

- [overview](overview.md) — OS analogy, crate map
- [tools](tools.md) — Tool trait, plugin crate structure
- [mcp](mcp.md) — MCP server and client
