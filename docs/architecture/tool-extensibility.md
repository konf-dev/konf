# Tool Extensibility Model

> Scope: the three-tier architecture for adding tools to Konf.

## The OS Analogy

Konf treats tools like an operating system treats code execution:

| OS concept | Konf equivalent | Trust level |
|------------|-----------------|-------------|
| Built-in drivers | Tier 1: Compiled Rust | Highest — infra only |
| Loadable kernel modules | Tier 2: WASM plugins | Medium — admin can add |
| Userspace daemons | Tier 3: MCP servers | Lowest — admin/user can add |

The agent cannot tell which tier a tool belongs to. Every tool implements the same `Tool` trait and appears identical in workflow YAML.

## Tier 1: Compiled Rust (In-Process)

Built-in tools compiled directly into the Konf binary:

- `memory:*` — backed by `konf-tool-memory`
- `ai:complete` — backed by `konf-tool-llm`
- `http:*` — backed by `konf-tool-http`
- `embed:text` — backed by `konf-tool-embed`
- `echo` — built into `konflux-core`

These run in-process with zero serialization overhead. Only the Konf team adds Tier 1 tools — they require a code change and a new release.

### Cross-Platform Compilation

The same codebase compiles for multiple targets using Cargo feature flags:

```toml
[features]
default = ["full"]
full = ["memory", "llm", "http", "embed", "mcp"]
edge = ["llm", "http"]       # minimal build for edge/embedded
```

An edge deployment can exclude memory and MCP support, producing a smaller binary.

## Tier 2: WASM Plugins (Sandboxed, Future)

WASM plugins run inside a `wasmtime` sandbox with explicit capability grants:

```yaml
# Future config (not yet implemented)
plugins:
  sentiment:
    wasm: ./plugins/sentiment.wasm
    capabilities:
      - "ai:complete"       # plugin can call the LLM
    memory_limit: 64MB
    timeout_ms: 5000
```

Key properties:
- **Sandboxed** — no filesystem, network, or syscall access unless explicitly granted.
- **Capability-restricted** — a WASM plugin receives only the capabilities listed in its config. It cannot exceed the product's own capability set.
- **Portable** — compiled once, runs on any platform Konf supports.
- **Admin-installed** — only platform admins can add WASM plugins.

The plugin's exported functions are registered as tools (e.g., `wasm:sentiment:analyze`) and appear in the same tool registry as Tier 1 and Tier 3 tools.

## Tier 3: MCP Servers (Out-of-Process)

MCP (Model Context Protocol) servers run as child processes:

```yaml
mcp_servers:
  github:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_TOKEN: "${GITHUB_TOKEN}"
```

Key properties:
- **Out-of-process** — communicate via stdio using the MCP protocol.
- **Ecosystem access** — any MCP-compatible server works (GitHub, Slack, filesystem, etc.).
- **Lifecycle managed** — Konf starts servers on demand and terminates them with the product.
- **Capability-gated** — MCP tools are subject to the same capability checks as built-in tools.

Tools appear as `mcp:<server>:<tool_name>` in workflows.

## Why the Agent Cannot Tell the Difference

All three tiers implement the same interface:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError>;
}
```

The workflow engine dispatches `do: memory:search` and `do: mcp:github:list_issues` through identical codepaths. The `VirtualizedTool` wrapper handles namespace injection and capability checking regardless of tier.

This means:
- Workflow authors do not care where a tool is implemented.
- A Tier 3 MCP tool can be promoted to Tier 1 (compiled Rust) without changing any workflow YAML.
- Security policies apply uniformly across all tiers.

## Summary

```
┌─────────────────────────────────────────────┐
│              Workflow Engine                 │
│         (same Tool trait for all)            │
├──────────┬──────────────┬───────────────────┤
│ Tier 1   │ Tier 2       │ Tier 3            │
│ Rust     │ WASM         │ MCP               │
│ in-proc  │ sandboxed    │ out-of-proc       │
│ infra    │ admin        │ admin/user        │
└──────────┴──────────────┴───────────────────┘
```
