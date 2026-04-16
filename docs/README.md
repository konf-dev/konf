# Konf Documentation

**Start here:** [MENTAL_MODEL.md](MENTAL_MODEL.md) — single source of truth for
architecture, vocabulary, and doctrine.

## Getting started
- [Quickstart](getting-started/quickstart.md) — Run Konf locally

## Product guide
- [Creating a Product](product-guide/creating-a-product.md) — Author an AI product in YAML
- [Workflow Reference](product-guide/workflow-reference.md) — Workflow YAML schema

## Admin guide
- [Deployment](admin-guide/deployment.md) — systemd, Docker, cloud
- [Platform Configuration](admin-guide/platform-config.md) — `konf.toml` reference

## Architecture
- [Overview](architecture/overview.md) — Crate map, composition, entry points
- [Engine](architecture/engine.md) — Workflow execution, three registries, capability validation
- [Runtime](architecture/runtime.md) — Process management, scope, namespace injection, capability lattice
- [Init](architecture/init.md) — Config loading, boot sequence
- [Backend](architecture/backend.md) — HTTP server, REST API, auth
- [MCP](architecture/mcp.md) — MCP server and client, name translation at the wire
- [Tools](architecture/tools.md) — Tool protocol, plugin crate structure, adapters
- [Memory Backends](architecture/memory-backends.md) — `MemoryBackend` trait, SurrealDB default
- [Why Konf](architecture/why-konf.md) — Structural properties that differentiate konf
