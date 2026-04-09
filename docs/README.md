# Konf Documentation

## By Audience

### Getting Started (Everyone)
- [Quickstart](getting-started/quickstart.md) — Run Konf in 5 minutes
- [Core Concepts](getting-started/concepts.md) — Products, workflows, tools, namespaces

### Product Guide (Product Builders)
- [Creating a Product](product-guide/creating-a-product.md) — Build an AI product with YAML
- [Workflow Reference](product-guide/workflow-reference.md) — YAML schema for workflows
- [Tools Reference](product-guide/tools-reference.md) — Available tools and configuration
- [Product Configuration](product-guide/configuration.md) — tools.yaml, models.yaml, project.yaml

### Admin Guide (Operators)
- [Deployment](admin-guide/deployment.md) — systemd, Docker, cloud
- [Platform Configuration](admin-guide/platform-config.md) — konf.toml reference
- [Security](admin-guide/security.md) — Namespaces, capabilities, audit logging
- [Integration](admin-guide/integration.md) — Crate dependencies, boot sequence, request flows

### Architecture (Infrastructure Contributors)
- [Overview](architecture/overview.md) — The OS analogy, crate map, design philosophy
- [Design Principles](architecture/design-principles.md) — 10 core principles
- [Engine](architecture/engine.md) — Workflow execution, registries, capability validation
- [Runtime](architecture/runtime.md) — Process management, scoping, streaming
- [Backend](architecture/backend.md) — HTTP server, REST API, auth
- [Init](architecture/init.md) — Config loading, boot sequence
- [MCP](architecture/mcp.md) — MCP server and client
- [Tools](architecture/tools.md) — Tool protocol, plugin crate structure
- [Tool Extensibility](architecture/tool-extensibility.md) — One interface, many adapters
- [Memory Backends](architecture/memory-backends.md) — MemoryBackend trait, implementations
- [Multi-Tenancy](architecture/multi-tenancy.md) — Namespace hierarchy, capability lattice
- [Configuration Strategy](architecture/configuration-strategy.md) — Platform vs product config
- [Session State](architecture/session-state.md) — Ephemeral KV store
- [Service Model](architecture/service-model.md) — How Konf runs as a service

### Internal
- [Plans](plans/) — Implementation roadmaps
