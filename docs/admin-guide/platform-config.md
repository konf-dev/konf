# Platform Configuration Reference

> Scope: the `konf.toml` file and environment variable overrides.

## Overview

Platform config is loaded once at startup from three sources (last wins):

1. Serde defaults (built into the binary)
2. `konf.toml` (in the config directory)
3. `KONF_` environment variables (split on `__`)

The file is optional. All fields have sensible defaults.

## Environment Variable Override Pattern

```
KONF_<SECTION>__<FIELD>=value
```

Examples:
- `KONF__DATABASE__URL=redb:///var/lib/konf/konf.redb`
- `KONF__SERVER__PORT=9000`
- `KONF__ENGINE__MAX_STEPS=500`

## Sections

### [database]

Optional. Configures the embedded **redb** file that backs the
journal, scheduler timers, and runner intent store. Edge deployments
omit this section entirely and run without persistence.

```toml
[database]
url = "redb:///var/lib/konf/konf.redb"
retention_days = 7
```

Accepted URL forms:

- `redb:///absolute/path/konf.redb`
- `file:///absolute/path/konf.redb`
- A bare filesystem path (relative or absolute)

`retention_days` (default: 7) controls how long journal entries and
terminal runner intents are kept before the background GC task
deletes them.

The `url` field is redacted in logs to prevent path/credential
leakage. See [`architecture/storage.md`](../architecture/storage.md)
for the full layout of the redb file.

**Breaking change from v1**: konf v1 used Postgres for the journal
(`postgresql://...` URLs and `pool_min` / `pool_max` settings).
Those fields no longer exist in v2. Use redb.

### [server]

```toml
[server]
host = "0.0.0.0"           # default: "0.0.0.0"
port = 8000                 # default: 8000
cors_origins = ["https://app.example.com"]  # default: [] (allow all, dev only)
```

### [auth]

```toml
[auth]
supabase_url = "https://your-project.supabase.co"  # default: "http://localhost:9999"
jwt_audience = "authenticated"                       # default: "authenticated"
```

### [engine]

Controls the workflow execution engine (konflux-core).

```toml
[engine]
max_steps = 100                 # default: 100 — abort after N steps
default_timeout_ms = 30000      # default: 30000 — per-tool timeout
max_workflow_timeout_ms = 300000 # default: 300000 — total workflow timeout
stream_buffer = 64              # default: 64 — SSE channel buffer
finished_channel_size = 128     # default: 128
default_retry_backoff_ms = 1000 # default: 1000
max_yaml_size = 1048576         # default: 1MB — prevents DoS
max_concurrent_nodes = 50       # default: 50 — parallel JoinSet cap
```

### [runtime]

Controls the Konf runtime (namespace isolation, resource limits).

```toml
[runtime]
max_steps = 1000                     # default: 1000
max_workflow_timeout_ms = 300000     # default: 300000 (5 min)
max_concurrent_nodes = 50           # default: 50
max_child_depth = 10                # default: 10 — nested workflow limit
max_active_runs_per_namespace = 20  # default: 20
```

### [observability]

```toml
[observability]
log_level = "info"    # trace, debug, info, warn, error
```

### Top-level fields

```toml
mcp_enabled = false         # default: false — enable MCP server support
config_dir = "./config"     # default: "./config" — product config path
```

## Validation

The binary validates all config at startup. Invalid config causes an immediate exit with descriptive error messages. Zero values for `server.port` and negative limits are rejected.

## Minimal Production Example

```toml
[database]
url = "postgresql://konf:secret@db.internal/konf"
pool_max = 50

[server]
host = "127.0.0.1"
port = 8000
cors_origins = ["https://app.example.com"]

[auth]
supabase_url = "https://your-project.supabase.co"

[engine]
max_steps = 200

[runtime]
max_active_runs_per_namespace = 50
```
