# Phase C: konf-backend (Rust/axum) — SUPERSEDES Python version

**Date:** 2026-04-06 (rewritten for all-Rust architecture)
**Status:** Replaces the Python/FastAPI plan
**Spec:** `docs/specs/2026-04-06-konf-backend-spec-v2.md`
**Implementation plan:** See `/home/bert/.claude/plans/bright-conjuring-unicorn.md` for detailed step-by-step

---

## Summary

konf-backend is now a single Rust binary (axum) that embeds konf-runtime, konflux, and smrti. All core tools are Rust. Python is opt-in for custom product tools only.

### What changed from the Python plan:
- FastAPI → axum (SSE built-in, 5-10x faster)
- Python LLM SDKs → rig-core (20+ providers, tool calling, streaming)
- Python mcp SDK → rmcp (official Rust SDK)
- asyncpg → sqlx (shared pool, one process)
- httpx → reqwest
- PyJWT → jsonwebtoken
- Custom Python poller → apalis (Postgres job queue)
- pydantic-settings → figment
- konf-tools Python package → eliminated (tools are Rust in konf-backend)

### Implementation phases:
1. Scaffold + config + health
2. Auth (JWT/JWKS)
3. Core Rust tools (memory, HTTP)
4. LLM tool (rig integration)
5. MCP client manager (rmcp)
6. Chat endpoint + SSE streaming
7. Scheduling (apalis)
8. Admin + monitoring API
9. Embeddings + starter templates
10. Python custom tools (opt-in)
11. Documentation + CI + Docker

See the detailed plan file for exact code per step.
