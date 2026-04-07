<div align="center">

# Konf

**The Agent OS — an operating system for AI agents**

[![CI](https://github.com/konf-dev/konf/actions/workflows/ci.yml/badge.svg)](https://github.com/konf-dev/konf/actions/workflows/ci.yml)
[![License: BSL-1.1](https://img.shields.io/badge/License-BSL--1.1-blue.svg)](LICENSE)

Self-hostable, local-first AI agent platform.
Products are configurations, not code.

[Architecture](docs/specs/konf-architecture.md) · [Quickstart](docs/guides/quickstart.md) · [Contributing](CONTRIBUTING.md)

</div>

---

## What is Konf?

Konf is an operating system for AI agents. It provides workflow execution, tool management, memory storage, and security — all configurable through YAML. No application code needed.

The same engine runs on a phone, a laptop, a homelab server, or a cloud cluster.

## Architecture

```
┌─────────────┐     ┌──────────┐
│ konf-backend │     │ konf-mcp │      Transport shells (HTTP / MCP)
│ (HTTP/REST)  │     │ (server) │
└──────┬───────┘     └────┬─────┘
       └────────┬─────────┘
                │
     ┌──────────▼───────────┐
     │      konf-init        │      Init system (config → engine)
     └──────────┬───────────┘
                │
     ┌──────────▼───────────┐
     │    konf-runtime       │      Process management, capabilities
     └──────────┬───────────┘
                │
     ┌──────────▼───────────┐
     │   konflux engine      │      Workflow execution, registries
     └──────────┬───────────┘
                │
     ┌──────────┼──────────┐
     │          │          │
   tools      tools      tools     Pluggable tool crates
   memory     llm        http
```

## Crates

| Crate | Description |
|-------|-------------|
| `konflux-core` | Workflow execution engine with tool/resource/prompt registries |
| `konf-runtime` | Process lifecycle, capability-based security, namespace injection |
| `konf-init` | Config-driven bootstrap — reads YAML, registers tools, wires runtime |
| `konf-mcp` | MCP server — expose tools/resources to Claude Desktop, Cursor, etc. |
| `konf-backend` | HTTP server — REST API with SSE streaming |
| `konf-tool-http` | HTTP GET/POST tools with SSRF protection |
| `konf-tool-llm` | LLM completion via rig-core (OpenAI, Anthropic, Google) |
| `konf-tool-embed` | Local text embeddings via fastembed (ONNX) |
| `konf-tool-mcp` | MCP client — consume external MCP servers |
| `konf-tool-memory` | MemoryBackend trait for pluggable storage |

Memory backends are external:
- [konf-dev/smrti](https://github.com/konf-dev/smrti) — Postgres + pgvector graph memory

## Quick Start

```bash
# Clone
git clone https://github.com/konf-dev/konf.git
cd konf

# Build
cargo build --workspace

# Run tests
cargo test --workspace

# Start with Docker
docker compose up -d
curl http://localhost:8000/v1/health
```

See [docs/guides/quickstart.md](docs/guides/quickstart.md) for detailed setup.

## License

Licensed under the [Business Source License 1.1](LICENSE).

- **Free** for personal, development, testing, and internal production use
- **Not free** for offering as a competing commercial hosted service
- Converts to [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0) on 2030-04-01
