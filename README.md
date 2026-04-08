<div align="center">

# Konf

**The Agent OS — an operating system for AI agents**

[![CI](https://github.com/konf-dev/konf/actions/workflows/ci.yml/badge.svg)](https://github.com/konf-dev/konf/actions/workflows/ci.yml)
[![License: BSL-1.1](https://img.shields.io/badge/License-BSL--1.1-blue.svg)](LICENSE)

Self-hostable, local-first AI agent platform.
Products are configurations, not code.

[Quickstart](docs/getting-started/quickstart.md) · [Product Guide](docs/product-guide/creating-a-product.md) · [Architecture](docs/architecture/overview.md) · [Contributing](CONTRIBUTING.md)

</div>

---

## What is Konf?

Konf is an operating system for AI agents. It provides workflow execution, tool management, memory storage, and security — all configurable through YAML. No application code needed.

The same engine runs on a phone, a laptop, a homelab server, or a cloud cluster. An agent's behavior, tools, memory, and security are defined entirely through configuration. Switching LLM providers, memory backends, or adding new tools is a config change, not a code change.

## Who Is This For?

| You are a... | You want to... | Start here |
|--------------|---------------|------------|
| **Product builder** | Build an AI product using YAML config | [Product Guide](docs/product-guide/creating-a-product.md) · [Products](products/) |
| **Operator** | Deploy and manage a Konf instance | [Admin Guide](docs/admin-guide/deployment.md) |
| **Infrastructure contributor** | Contribute to the Rust codebase | [Architecture](docs/architecture/overview.md) · [Contributing](CONTRIBUTING.md) |
| **Curious** | Understand how Konf works | [Core Concepts](docs/getting-started/concepts.md) |

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

## Products

A **product** is a complete AI agent defined entirely through config files — no code. See [products/](products/) for reference products and a starter template.

```
products/assistant/
├── config/
│   ├── tools.yaml          # Which tools to use
│   ├── models.yaml         # LLM provider and model
│   ├── project.yaml        # Product metadata
│   └── workflows/
│       └── chat.yaml       # Workflow: search memory → respond
└── prompts/
    └── system.md           # Assistant personality
```

## Extensibility

Tools are added, not coded. Three tiers:

| Tier | Mechanism | Who Can Add | Examples |
|------|-----------|-------------|---------|
| Compiled Rust | In-process | Infra | memory, LLM, HTTP |
| WASM Plugins | Sandboxed runtime | Admin | Custom transforms (planned) |
| MCP Servers | Out-of-process | Admin / User | Gmail, Calendar, Notion |

The agent can't tell the difference. See [sdk/](sdk/) for details.

## Quick Start

```bash
# Clone and build
git clone https://github.com/konf-dev/konf.git
cd konf
cargo build --workspace

# Run tests
cargo test --workspace

# Start with Docker (includes Postgres)
docker compose up -d
curl http://localhost:8000/v1/health
```

See [docs/getting-started/quickstart.md](docs/getting-started/quickstart.md) for detailed setup.

## License

Licensed under the [Business Source License 1.1](LICENSE).

- **Free** for personal, development, testing, and internal production use
- **Not free** for offering as a competing commercial hosted service
- Converts to [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0) on 2030-04-01
