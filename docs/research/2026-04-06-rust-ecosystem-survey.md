# Rust Ecosystem Survey for Konf Platform

**Date:** 2026-04-06
**Decision:** All-Rust core is viable and superior to Python for the platform layer.

## Production-Ready Crates

| Crate | Purpose | Version | Downloads | Stars | Verdict |
|---|---|---|---|---|---|
| **rig-core** | Multi-provider LLM (20+ providers, tool calling, streaming) | 0.34 | 472k | 6.2k | **USE** — replaces all Python LLM SDKs |
| **rmcp** | Official MCP SDK (client + server, stdio/SSE/HTTP) | 1.3 | 6.9M | — | **USE** — replaces Python mcp SDK |
| **async-openai** | OpenAI client (streaming, tool calling) | 0.34 | 4.1M | 1.8k | **AVAILABLE** — rig wraps this |
| **fastembed** | Local embeddings via ONNX (BAAI/bge models) | 5.13 | 734k | — | **USE** — eliminates Python for embeddings |
| **jsonwebtoken** | JWT verification (RS256, JWKS) | 10.3 | 113M | — | **USE** — replaces PyJWT |
| **apalis** | Postgres-backed job queue with cron | 1.0-rc.7 | 673k | — | **USE** — replaces custom Python poller |
| **axum** | HTTP server with SSE | 0.8 | 279M | 25k | **USE** — replaces FastAPI |
| **reqwest** | HTTP client | 0.13 | 424M | — | **USE** — replaces httpx |
| **figment** | Config loading (TOML, JSON, env) | 0.10 | — | — | **USE** — replaces pydantic-settings |
| **minijinja** | Jinja2 templates (by Jinja2 creator) | 2.19 | 16.4M | — | **KEEP** — already in konflux |
| **schemars** | JSON Schema from Rust types | 1.2 | 209M | — | **KEEP** — already used |
| **utoipa** | OpenAPI docs for axum | — | — | — | **USE** — replaces FastAPI auto-docs |

## Key Findings

1. **Rig subsumes individual provider SDKs.** One crate handles OpenAI, Anthropic, Google, and 17+ others. Tool calling and streaming are built in.
2. **rmcp is the official MCP SDK** from the modelcontextprotocol org. Feature-complete with all transports.
3. **fastembed eliminates Python for embeddings.** ONNX-based, runs BAAI/bge models locally.
4. **The Langfuse Rust client (0.1.8) is too immature.** Route traces via OTEL Collector → Langfuse v3+ (supports OTEL ingestion natively).
5. **litellm (Python) had a supply chain attack** in March 2026 (v1.82.7-1.82.8). Cautionary tale about Python dependencies.

## What This Means

The entire Konf platform can be Rust. Python is only needed for:
- Custom product tools (developer's iteration layer)
- Some niche integrations without Rust SDKs

The hot path (memory search → LLM call → memory store → SSE stream) is pure Rust with zero GIL involvement.
