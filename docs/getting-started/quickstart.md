# Konf Quickstart Guide

Get the Konf platform running locally in under a minute. No Postgres,
no docker-compose, no external services — konf v2 is **local-first**
and ships with an embedded redb storage backend.

---

## Prerequisites

- **Rust** 1.75+ (`rustup` recommended)
- Optional: **Docker** only if your product uses `shell:exec` with
  container sandboxing.

That's it. No database, no broker, no external dependencies.

---

## Option A: HTTP backend with a redb file

```bash
# Clone and build
git clone https://github.com/konf-dev/konf.git
cd konf
cargo build --release --workspace

# Create a minimal config
mkdir -p config
cat > config/konf.toml <<'EOF'
[database]
url = "redb:///tmp/konf.redb"
retention_days = 7

[server]
host = "127.0.0.1"
port = 8000
EOF

cat > config/tools.yaml <<'EOF'
tools:
  http:
    enabled: true
EOF

# Run
KONF_CONFIG_DIR=./config ./target/release/konf-backend

# Health check
curl http://localhost:8000/v1/health
# → {"status":"ok","version":"0.1.0"}
```

The first run creates `/tmp/konf.redb` and initializes the journal,
scheduler, and runner-intent tables. Subsequent runs re-open the same
file — your schedules and unterminated intents survive restarts.

---

## Option B: stdio MCP server (Claude Desktop, Cursor)

```bash
cargo build --release --bin konf-mcp
```

Add to `~/.config/claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "konf": {
      "command": "/path/to/target/release/konf-mcp",
      "args": ["--config", "/path/to/config"]
    }
  }
}
```

Claude Desktop will spawn `konf-mcp` as a subprocess and see all
registered tools and workflows.

---

## Option C: HTTP MCP + REST sharing state (dev only)

When you want your TUI (talking to `konf-backend`'s REST API) and
your MCP client to observe the **same** running workflows and share
the **same** memory, enable the HTTP MCP endpoint:

```bash
KONF_CONFIG_DIR=./config \
KONF_MCP_HTTP=1 \
./target/release/konf-backend
```

`/mcp` is now mounted on the same axum router as `/v1/*`, and both
transports share the same `Arc<Runtime>`. Point Claude Code at
`http://localhost:8000/mcp`, open your TUI against
`/v1/monitor/stream`, and watch a single workflow run appear in both
places simultaneously.

**Dev-only caveats**:
- Every MCP session gets `capabilities = ["*"]`.
- No per-user auth, no per-session scoping.
- Guarded only by your `tool_guards` rules in `tools.yaml` and by
  namespace injection via `VirtualizedTool` (which scopes MCP
  memory ops to the `konf:mcp:http` namespace).
- Never enable `KONF_MCP_HTTP=1` on a network-exposed deployment.
  See [`architecture/mcp-http.md`](../architecture/mcp-http.md) for
  the full security model.

---

## Edge mode (no persistence)

For throwaway tests or ephemeral compute, omit the `[database]`
section entirely:

```toml
# config/konf.toml — edge mode
[server]
host = "127.0.0.1"
port = 8000
```

The runtime boots without a journal, scheduler, or runner-intent
store. Workflows still run. Nothing survives a restart. Good for
CI, bad for anything you care about.

---

## Your First Workflow

Create `config/workflows/hello.yaml`:

```yaml
workflow: hello
description: "A simple echo workflow"
register_as_tool: true
capabilities: []
nodes:
  greet:
    do: echo
    with:
      message: "Hello from Konf!"
    return: true
```

This workflow auto-registers as `workflow:hello` and is callable via
`/v1/chat`, MCP (stdio or HTTP), or another workflow via
`do: workflow:hello`.

## Your First Schedule

Create `config/schedules.yaml`:

```yaml
- name: hourly_hello
  workflow: hello
  cron: "0 0 * * * * *"          # every hour on the hour
  namespace: "konf:quickstart:local"
  capabilities: []
  input: {}
```

On boot, `konf-init` registers this cron with the durable scheduler.
The timer survives restarts. On fire, the scheduler looks up
`workflow:hello` in the live engine registry and invokes it.

Cron syntax is 7 fields: `sec min hour day month weekday year`.

---

## Watching runs live (the TUI path)

```bash
# In another terminal
curl -N http://localhost:8000/v1/monitor/stream
```

You'll see Server-Sent Events stream as workflows start, progress,
and complete:

```
event: hello
data: {"status":"connected"}

event: run_started
data: {"type":"run_started","run_id":"...","workflow_id":"hello",...}

event: run_completed
data: {"type":"run_completed","run_id":"...","duration_ms":12}
```

Filter by namespace prefix via the `?namespace=` query parameter.

---

## Running Tests

```bash
# Full workspace, no external services
cargo test --workspace

# Clippy clean
cargo clippy --workspace --all-targets -- -D warnings
```

---

## Next Steps

- Read [`MENTAL_MODEL.md`](../MENTAL_MODEL.md) for the single source of
  truth on architecture, vocabulary, and doctrine.
- Read [`architecture/durability.md`](../architecture/durability.md)
  to understand the durability model (intent + idempotent retry, not
  checkpoint-and-replay).
- Read [`architecture/storage.md`](../architecture/storage.md) for
  how the redb file is laid out.
- Read [`architecture/mcp-http.md`](../architecture/mcp-http.md) for
  the `/mcp` endpoint's security model.
- Read [`product-guide/creating-a-product.md`](../product-guide/creating-a-product.md)
  to author your own product.
