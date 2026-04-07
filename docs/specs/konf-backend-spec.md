# Konf Backend Specification

**Status:** Authoritative
**Crate:** `konf-backend`
**Role:** Shell (bash) — HTTP transport over the engine

---

## Overview

konf-backend is a thin HTTP server. It serves the REST API, handles authentication, and optionally mounts konf-mcp for MCP clients. It contains zero tool implementations.

---

## What konf-backend IS

- HTTP server (axum)
- Uses konf-init for bootstrap (config loading, engine creation, tool registration)
- Auth middleware (JWT/JWKS, pluggable: Supabase, Auth0, local signer)
- Optionally mounts konf-mcp as additional transport at `/mcp`
- Scheduling (server-only: Postgres job queue for cron/delayed workflows)

## What konf-backend is NOT

- Contains zero tool implementations — all tools live in konf-tools crates
- Does not import smrti or any memory backend directly
- Does not contain MCP protocol logic — delegates to konf-mcp crate
- Does not contain config loading or tool registration — delegates to konf-init

---

## Startup Flow

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    // 1. Boot via konf-init (loads config, creates engine, registers tools, wires runtime)
    let instance = konf_init::boot(Path::new("./config")).await?;

    // 2. Auth
    let verifier = JwtVerifier::new(&instance.config.auth);

    // 3. Scheduling (only if DB configured)
    if let Some(db_url) = &instance.config.database {
        let scheduler = Scheduler::new(db_url, instance.runtime.clone());
        scheduler.migrate().await?;
        scheduler.start_polling(10);
    }

    // 4. Mount konf-mcp (optional)
    let mcp_router = if instance.config.mcp.enabled {
        let server = KonfMcpServer::new(instance.engine.clone(), instance.runtime.clone());
        Some(server.sse_handler())
    } else {
        None
    };

    // 5. Build router
    let app = build_router(instance, verifier, mcp_router);

    // 6. Serve with graceful shutdown
    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
```

---

## REST API Endpoints

All endpoints except `/v1/health` require JWT authentication.

### Chat

| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/chat` | Streaming chat — SSE response |

Accepts `{ message, session_id }`. Builds ExecutionScope from JWT claims + product config capabilities. Starts workflow via `runtime.start_streaming()`. Pipes `StreamReceiver` → SSE events.

SSE event types:
- `start` — `{ run_id }`
- `text_delta` — `{ content }` (LLM content token)
- `thought_delta` — `{ content }` (LLM reasoning token)
- `tool_start` — `{ tool, node_id }`
- `tool_end` — `{ tool, node_id, duration_ms }`
- `done` — `{ run_id, status }`
- `error` — `{ message }`

### Messages

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/messages` | Conversation history for session |

### Monitoring

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/monitor/runs` | List active workflow runs |
| GET | `/v1/monitor/runs/{id}` | Get run detail |
| GET | `/v1/monitor/runs/{id}/tree` | Get process tree |
| DELETE | `/v1/monitor/runs/{id}` | Cancel a run |
| GET | `/v1/monitor/metrics` | Runtime metrics (active, completed, failed, cancelled, uptime) |

All monitoring endpoints require authentication. Audit logging on list and cancel operations.

### Admin

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/admin/config` | Read current product config |
| PUT | `/v1/admin/config` | Update product config (triggers hot-reload via konf-init) |
| GET | `/v1/admin/audit` | Query event journal |

### Health

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/health` | Health check (no auth required) |

Returns `{ status: "ok", version }`. If DB is configured, includes connectivity check.

### MCP (optional)

| Path | Description |
|------|-------------|
| `/mcp` | SSE endpoint for MCP clients (mounted from konf-mcp) |

---

## Authentication

JWT verification via JWKS endpoint:
- Fetches and caches JWKS from configured auth provider (Supabase, Auth0, etc.)
- Extracts `sub` (user ID), `role`, `aud` (audience) from claims
- Builds `Actor { id, role }` for ExecutionScope
- Maps role claim to `ActorRole` enum (InfraAdmin, ProductAdmin, User, agents)

Auth is pluggable — the provider URL and audience are in `konf.toml`. No code changes to switch auth providers.

---

## Scheduling

Server-only concern. Disabled if no database URL is configured.

- Postgres-backed job queue with `FOR UPDATE SKIP LOCKED` for multi-worker safety
- Cron jobs registered from `schedules.yaml`
- One-off delayed jobs via API (reminders, debounced extraction)
- Jobs execute workflows via `runtime.run()`

---

## Graceful Shutdown

On SIGTERM or SIGINT:
1. Stop accepting new HTTP connections
2. Drain in-flight requests (configurable timeout)
3. Stop scheduler polling
4. Cancel running workflows (propagate to children)
5. Cleanup MCP server processes (SIGTERM, wait, SIGKILL)

---

## Cargo.toml

```toml
[dependencies]
konf-init = { ... }
konf-mcp = { ..., optional = true }
konf-runtime = { ... }
konflux = { ... }

# HTTP
axum = { version = "0.8", features = ["macros"] }
tower-http = { version = "0.6", features = ["cors", "trace"] }
tokio = { version = "1", features = ["full"] }

# Streaming
async-stream = "0.3"
tokio-stream = "0.1"

# Auth
jsonwebtoken = "10"
reqwest = { version = "0.13", features = ["json"] }

# Scheduling (optional)
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres"], optional = true }

# Config
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Observability
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[features]
default = ["mcp", "scheduling"]
mcp = ["konf-mcp"]
scheduling = ["sqlx"]
```

Note: konf-backend does NOT depend on any konf-tool-* crates or smrti. All tool dependencies are in konf-init.

---

## Related Specs

- [konf-architecture](konf-architecture.md) — crate map, backend as shell
- [konf-init-spec](konf-init-spec.md) — boot sequence, KonfInstance
- [konf-mcp-spec](konf-mcp-spec.md) — MCP server mounted at `/mcp`
- [konf-runtime-spec](konf-runtime-spec.md) — Runtime API, streaming
- [multi-tenancy](multi-tenancy.md) — auth → ExecutionScope mapping
- [configuration-strategy](configuration-strategy.md) — konf.toml, tools.yaml
