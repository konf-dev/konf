# Konf Quickstart Guide

Get the Konf platform running locally in 5 minutes.

---

## Prerequisites

- **Rust** 1.75+ (`rustup` recommended)
- **PostgreSQL** 15+ with pgvector extension (for smrti backend)
- **Docker** (optional, for docker-compose setup)

## Option A: Docker Compose (recommended)

```bash
# Clone the repo
git clone https://github.com/konf-dev/konf-dev-stack.git
cd konf-dev-stack

# Create a minimal config
mkdir -p config
cat > config/tools.yaml <<'EOF'
tools:
  memory:
    backend: smrti
    config:
      dsn: "postgresql://postgres:pass@localhost:5432/konf"
  http:
    enabled: true
EOF

# Start Postgres + backend
docker compose up -d

# Check health
curl http://localhost:8000/v1/health
# → {"status":"ok","version":"0.1.0"}
```

## Option B: From Source

```bash
# Clone all repos
git clone https://github.com/konf-dev/konf-dev-stack.git
cd konf-dev-stack

# Ensure Postgres is running with pgvector
# createdb konf

# Create config
mkdir -p config
cat > config/konf.toml <<'EOF'
[database]
url = "postgresql://localhost/konf"

[server]
host = "0.0.0.0"
port = 8000
EOF

cat > config/tools.yaml <<'EOF'
tools:
  http:
    enabled: true
EOF

# Build and run
cargo run --release --bin konf-backend
```

## Option C: MCP Server (Claude Desktop)

```bash
# Build the MCP server
cargo build --release --bin konf-mcp

# Add to Claude Desktop config (~/.config/claude/claude_desktop_config.json):
{
  "mcpServers": {
    "konf": {
      "command": "/path/to/target/release/konf-mcp",
      "args": ["--config", "/path/to/config"]
    }
  }
}
```

Claude Desktop will connect to Konf and see all registered tools.

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

This workflow is automatically registered as `workflow_hello` and can be called via the chat API or MCP.

---

## Running Tests

```bash
# Unit tests (no external services needed)
cargo test --workspace

# Integration tests (requires Postgres with DATABASE_URL set)
DATABASE_URL=postgresql://localhost/konf_test cargo test --workspace -- --ignored
```

---

## Project Structure

```
konf/                             ← Cargo workspace (monorepo)
├── crates/
│   ├── konflux-core/             ← Engine (kernel): workflow execution
│   ├── konf-runtime/             ← Process management: lifecycle, capabilities
│   ├── konf-init/                ← Init system: config loading, tool registration
│   ├── konf-tool-http/           ← HTTP request tools (reqwest)
│   ├── konf-tool-llm/            ← LLM completion tools (rig-core)
│   ├── konf-tool-embed/          ← Embedding tools (fastembed)
│   ├── konf-tool-memory/         ← Memory tools + MemoryBackend trait
│   ├── konf-tool-mcp/            ← MCP client (consume external MCP servers)
│   ├── konf-mcp/                 ← MCP server (Claude Desktop, CLI)
│   └── konf-backend/             ← HTTP server (REST API)
├── config/                       ← Your product configuration
│   ├── konf.toml                 ← Platform config (server, auth, database)
│   ├── tools.yaml                ← Tool and backend config
│   └── workflows/                ← Workflow YAML files
└── docs/                         ← Architecture and specs
```

## What is smrti?

smrti (Sanskrit: "that which is remembered") is the graph memory engine. It is an **external dependency** maintained in the [konf-dev/smrti](https://github.com/konf-dev/smrti) repo — it is not part of this monorepo. It provides:
- Knowledge graph storage (nodes, edges, metadata)
- Hybrid search (vector + full-text)
- Session state (ephemeral key-value scratchpad)
- Event sourcing (append-only log for audit)

The `konf-tool-memory-smrti` bridge crate (also in the smrti repo) implements the `MemoryBackend` trait for smrti. See [memory-backends.md](../architecture/memory-backends.md).

---

## Next Steps

- Read [overview.md](../architecture/overview.md) for the full platform design
- Read [integration.md](../admin-guide/integration.md) for how crates connect
- Check [master-plan.md](../plans/master-plan.md) for the implementation roadmap
