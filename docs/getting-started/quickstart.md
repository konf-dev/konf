# Konf Quickstart Guide

Get the Konf platform running locally in 5 minutes.

---

## Running Konf

To run Konf against a product, point `KONF_CONFIG_DIR` at a product's `config/` directory and start one of the two transport shells:

- **`konf-backend`** — HTTP server. `POST /v1/chat` with SSE streaming.
- **`konf-mcp`** — stdio MCP server. Used by Claude Desktop, Cursor, Claude Code, and any other MCP client.

Products that need external infrastructure (Postgres for memory, secret store, etc.) can bring their own via a workflow that calls `shell:exec` to run `docker compose`. The [`products/init/`](../../products/init/) product is a reference example.

---

## Prerequisites

- **Rust** 1.75+ (`rustup` recommended)
- **Docker** (to run the `init` product's containers)
- **Infisical CLI** (recommended for secret management)

---

## Option A: Standalone Docker (The "Old Way")

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
konf/                             ← Cargo workspace
├── crates/                       ← 13 Rust crates
│   ├── konflux-core/             ← Engine: workflow execution, 3 registries
│   ├── konf-runtime/             ← Process management, ExecutionScope, capability lattice
│   ├── konf-init/                ← Bootstrap: config loading, tool registration
│   ├── konf-init-kell/           ← CLI scaffolder for new products
│   ├── konf-backend/             ← HTTP server (REST API + SSE)
│   ├── konf-mcp/                 ← MCP server (stdio + SSE)
│   ├── konf-tool-http/           ← HTTP GET/POST tools
│   ├── konf-tool-llm/            ← LLM completion (rig-core)
│   ├── konf-tool-embed/          ← Local embeddings (fastembed)
│   ├── konf-tool-memory/         ← MemoryBackend trait
│   ├── konf-tool-mcp/            ← MCP client (consume external servers)
│   ├── konf-tool-shell/          ← Sandboxed shell execution
│   └── konf-tool-secret/         ← Secret retrieval with allowlist
├── products/                     ← Reference products (YAML + markdown)
│   ├── _template/                ← Minimal starter
│   ├── devkit/                   ← Canonical reference (VCS workflows)
│   └── init/                     ← Init product example
├── docs/                         ← Architecture, product guide, MENTAL_MODEL.md
├── sandbox/                      ← E2E testing infrastructure
└── sdk/                          ← Plugin SDK (WASM planned)
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

- Read [MENTAL_MODEL.md](../MENTAL_MODEL.md) for the single source of truth on architecture, vocabulary, and doctrine
- Read [overview.md](../architecture/overview.md) for the full platform design
- Read [creating-a-product.md](../product-guide/creating-a-product.md) to author your own product
