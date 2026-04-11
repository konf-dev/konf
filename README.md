<div align="center">

# Konf

**An operating system for AI agents**

[![CI](https://github.com/konf-dev/konf/actions/workflows/ci.yml/badge.svg)](https://github.com/konf-dev/konf/actions/workflows/ci.yml)
[![License: BSL-1.1](https://img.shields.io/badge/License-BSL--1.1-blue.svg)](LICENSE)

Self-hostable, local-first. Products are configurations (YAML + markdown), not code.

[Mental Model](docs/MENTAL_MODEL.md) · [Quickstart](docs/getting-started/quickstart.md) · [Product Guide](docs/product-guide/creating-a-product.md) · [Architecture](docs/architecture/overview.md) · [Contributing](CONTRIBUTING.md)

</div>

---

## What is Konf?

Konf is an operating system for AI agents. It provides workflow execution, tool management, capability enforcement, process management, and memory — all configurable through YAML. A product is a directory of YAML + markdown that defines a complete agent. No application code needed.

The same Rust binary runs every product. Switching LLM providers, memory backends, or adding tools is a config change, not a code change.

**For the full architecture, vocabulary, and doctrine — read [`docs/MENTAL_MODEL.md`](docs/MENTAL_MODEL.md) first.** That file is the single source of truth.

## Architecture

```
┌──────────────┐     ┌──────────┐
│ konf-backend │     │ konf-mcp │      Transport shells (HTTP / stdio-MCP)
└──────┬───────┘     └────┬─────┘
       └────────┬─────────┘
                │
     ┌──────────▼───────────┐
     │      konf-init       │         Bootstrap (config → engine)
     └──────────┬───────────┘
                │
     ┌──────────▼───────────┐
     │    konf-runtime      │         Process table, capabilities, namespaces
     └──────────┬───────────┘
                │
     ┌──────────▼───────────┐
     │   konflux-core       │         Engine: tools, resources, prompts
     └──────────┬───────────┘
                │
     ┌──────────┴──────────┐
     │   konf-tool-*       │         Plugin crates (http, llm, embed,
     │                     │          memory, mcp-client, shell, secret)
     └─────────────────────┘
```

## Crates

13 crates in this workspace:

| Crate | Role |
|-------|------|
| `konflux-core` | Workflow execution engine with tool/resource/prompt registries. Zero I/O. |
| `konf-runtime` | Process lifecycle, `ExecutionScope`, capability-based security, namespace injection |
| `konf-init` | Config-driven bootstrap — reads YAML, registers tools, wires runtime |
| `konf-init-kell` | CLI scaffolder for new product directories |
| `konf-mcp` | MCP server — exposes products to MCP clients (Claude Desktop, Cursor, etc.) |
| `konf-backend` | HTTP server — REST API with SSE streaming |
| `konf-tool-http` | HTTP GET/POST tools with SSRF protection |
| `konf-tool-llm` | LLM completion via rig-core (OpenAI, Anthropic, Google) |
| `konf-tool-embed` | Local text embeddings via fastembed (ONNX) |
| `konf-tool-memory` | `MemoryBackend` trait for pluggable storage |
| `konf-tool-mcp` | MCP client — consume external MCP servers |
| `konf-tool-shell` | Sandboxed shell execution |
| `konf-tool-secret` | Secret retrieval with allowlist |

Memory backends are external:
- [konf-dev/smrti](https://github.com/konf-dev/smrti) — Postgres + pgvector graph memory

## Products

A **product** is a complete AI agent defined through config files. See [`products/`](products/) for reference products — `_template/` for a minimal starter, `devkit/` for the experiment-003-validated reference with VCS workflows, `init/` for an example infrastructure-provisioning product.

```
products/_template/
├── config/
│   ├── tools.yaml           # Which tools to use
│   └── workflows/
│       └── hello.yaml       # Workflow DAG
└── README.md
```

## Extensibility

Tools are added, not coded. Every tool implements the same `Tool` trait — the engine dispatches all tools identically regardless of how they're implemented.

Konf ships several adapters that wrap different execution environments behind this interface:

| Adapter | How it works | Who can add tools | Status |
|---------|-------------|-------------------|--------|
| Compiled Rust | In-process | Infra | Available |
| MCP client | Out-of-process (stdio/SSE) | Admin / User | Available |
| HTTP | Network call | Admin | Available |
| WASM | Sandboxed runtime | Admin | Planned |

See [`sdk/`](sdk/) for details.

## Quick Start

```bash
git clone https://github.com/konf-dev/konf.git
cd konf
cargo build --workspace
cargo test --workspace

# Start with Docker (includes Postgres)
docker compose up -d
curl http://localhost:8000/v1/health
```

See [`docs/getting-started/quickstart.md`](docs/getting-started/quickstart.md) for detailed setup.

## License

Licensed under the [Business Source License 1.1](LICENSE).

- **Free** for personal, development, testing, and internal production use
- **Not free** for offering as a competing commercial hosted service
- Converts to [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0) on 2030-04-01
