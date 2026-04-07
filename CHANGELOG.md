# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [0.1.0] - 2026-04-07

### Added

- Initial release: 10-crate monorepo
- `konflux-core`: workflow execution engine with MCP-native registries (Tools, Resources, Prompts)
- `konf-runtime`: process management with capability-based security and namespace injection
- `konf-init`: config-driven bootstrap system
- `konf-mcp`: MCP server (stdio transport) for Claude Desktop and other MCP clients
- `konf-backend`: HTTP server with SSE streaming (axum)
- `konf-tool-http`: HTTP GET/POST tools with SSRF protection
- `konf-tool-llm`: LLM completion via rig-core (OpenAI, Anthropic, Google)
- `konf-tool-embed`: local text embeddings via fastembed (ONNX)
- `konf-tool-mcp`: MCP client for consuming external MCP servers
- `konf-tool-memory`: MemoryBackend trait for pluggable storage backends
- Workflow-as-tool: workflows register as callable tools for composition
- ToolAnnotations: MCP-parity behavioral hints (read_only, destructive, idempotent, open_world)
- Configurable CORS, optional database, edge-mode operation
- GitHub Actions CI (fmt, clippy, test, cargo-deny)
- Docker multi-stage build with cargo-chef caching
