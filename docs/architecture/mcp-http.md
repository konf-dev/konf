# MCP over HTTP (dev-only)

Konf ships an optional HTTP transport for MCP mounted inside
`konf-backend` at `/mcp`. Its only purpose is to solve the
**split-brain problem**: when you want your TUI (talking to the REST
API) and your MCP client (Claude Code, mcp-inspector, Gemini CLI) to
observe the same running workflows and share the same memory, both
need to talk to the same `Arc<Runtime>`. Two separate processes
can't.

## When to use which transport

Konf exposes MCP two ways. Both are supported and they serve different
needs.

| Transport | Binary | Process | Use for |
|---|---|---|---|
| **Stdio** | `konf-mcp` | Separate, spawned by the MCP client | Claude Desktop, local use of Claude Code where the client manages the server lifecycle. Each client gets its own runtime. |
| **HTTP (Streamable)** | `konf-backend` | Same process as REST API | Dev workflows where TUI + MCP client share state. Integration tests. Web-hosted clients. |

The stdio binary is **not deprecated**. It remains the right choice
when you don't need shared state with another client.

## Enabling `/mcp`

Two conditions must both hold:

1. The `mcp` cargo feature is compiled in (on by default).
2. The environment variable `KONF_MCP_HTTP=1` is set at runtime.

```bash
KONF_MCP_HTTP=1 konf-backend
```

Without both, the `/mcp` route is not mounted and returns 404. On
startup you'll see a loud warning log line:

```
*** KONF_MCP_HTTP enabled — /mcp mounted with capabilities=["*"]. DEV ONLY. Never use in production. ***
```

`KONF_MCP_HTTP` is **separate from** `KONF_DEV_MODE`. The latter
bypasses JWT auth on `/v1/*` routes. The two are orthogonal — you
can enable MCP over HTTP without disabling REST auth, and vice versa.

## Capability model

Every MCP session over HTTP runs with `session_capabilities = ["*"]`.
There is no per-user auth, no JWT verification, no scoped capability
grants. This is the entire reason it's dev-only.

What `["*"]` means in practice depends on which kind of tool is called.

### Direct primitive-tool calls

When an MCP client calls a non-workflow tool — `memory:search`,
`http:get`, `shell:exec`, etc. — the call flows through
`KonfMcpServer::call_tool` → `Runtime::invoke_tool(name, input, scope)`
where `scope` is constructed from the session caps:

```rust
ExecutionScope {
    namespace: "konf:mcp:http",
    capabilities: vec![CapabilityGrant::with_bindings("*", {
        "namespace": "konf:mcp:http",
    })],
    actor: Actor { id: "mcp-http", role: ActorRole::System },
    ...
}
```

Two things happen because we route through `Runtime::invoke_tool`:

1. **`VirtualizedTool` wrapping**: every capability grant carries a
   `{namespace: "konf:mcp:http"}` binding. This binding is injected
   into the tool's input via `VirtualizedTool` before the tool sees
   it. For memory-family tools, that means every MCP-originated
   memory operation is automatically scoped to the
   `konf:mcp:http` namespace — it can't leak into tenant namespaces
   even though the session caps are `["*"]`.
2. **`GuardedTool` wrapping**: any `tool_guards` rules you've
   configured in `tools.yaml` apply to MCP calls the same way they
   apply to workflow nodes. Configure a deny rule for `shell:exec
   { contains: "sudo" }` and MCP sessions inherit it for free.

This is the "loose by default, guard-enforced" posture. `["*"]`
relaxes the capability check but does not bypass guards or namespace
injection.

### Workflow-tool calls

When an MCP client calls a registered workflow (e.g.
`workflow:morning_brief`), the call lands at `WorkflowTool::invoke`
which **ignores the caller's capabilities** and uses its own
registration-time default scope. That default scope comes from the
workflow YAML's `capabilities:` field.

So if `workflows/morning-brief.yaml` declares:

```yaml
workflow: morning_brief
register_as_tool: true
capabilities: [memory:read, ai:complete]
```

…then MCP can invoke `workflow:morning_brief` but the workflow runs
under exactly `[memory:read, ai:complete]`, not `["*"]`. Any
sub-workflows it spawns attenuate from there. The MCP caller cannot
escalate a workflow's privileges by virtue of having `["*"]` at the
session layer.

**Recommended pattern**: expose workflows, not primitives. Design
`workflows/*.yaml` with narrow capability declarations, and let MCP
clients call workflows by name. This gives you per-feature least
privilege without needing per-session auth.

## The split-brain fix in practice

Before Phase 4: running `konf-backend` and `konf-mcp` pointed at the
same product directory would boot **two separate `KonfInstance`s**.
Each had its own `Runtime`, its own in-memory `ProcessTable`, its own
`Arc<RunEventBus>`. A workflow started from Claude Code via the
stdio server was invisible to your TUI because the TUI was looking at
a different runtime's process table.

After Phase 4: enable `KONF_MCP_HTTP=1` on `konf-backend`, point
Claude Code at `http://localhost:8000/mcp`, and the MCP session runs
*inside* `konf-backend`. The factory closure that creates each new
`KonfMcpServer` captures `Arc<Runtime>` from the backend's `AppState`:

```rust
pub fn routes(runtime: Arc<Runtime>) -> Router {
    let service = StreamableHttpService::new(
        move || {
            let engine = Arc::new(runtime.engine().clone());
            Ok(konf_mcp::KonfMcpServer::new(engine, runtime.clone()))
        },
        Arc::new(LocalSessionManager::default()),
        Default::default(),
    );
    Router::new().nest_service("/mcp", service)
}
```

One runtime, one process table, one event bus. Workflow runs started
via MCP appear in `/v1/monitor/runs`, stream through
`/v1/monitor/stream`, are recorded in the journal, and can be
cancelled via `DELETE /v1/monitor/runs/{id}` — all the same as runs
started via `/v1/chat`.

## When to build something better

For production deployments (multi-user, network-exposed, or compliance
workloads), the dev-only posture is not appropriate. The path to a
production-ready HTTP MCP endpoint is tracked as future work in
[`docs/plans/konf-v2.md` §16](../plans/konf-v2.md) and involves:

1. A custom `rmcp::SessionManager` implementation that reads a JWT or
   API key from the `Authorization` / `Mcp-Session-Id` headers.
2. Per-session `ExecutionScope` construction mapping the auth claims
   to a capability set and a tenant namespace.
3. Per-session rate limiting and usage tracking (shared with the
   REST API's middleware).

None of this exists in v2. Until it does, `KONF_MCP_HTTP=1` is a
local-only convenience — treat it like `rails s` or `next dev`.
