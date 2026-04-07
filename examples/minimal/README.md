# Minimal Example

Boots Konf with zero configuration. Proves the system starts and responds to health checks.

## Run

```bash
KONF_CONFIG_DIR=examples/minimal/config cargo run --bin konf-backend
```

## Test

```bash
curl http://localhost:8000/v1/health
# → {"status":"ok","version":"0.1.0"}
```

## What's available

- 5 builtin tools: echo, json_get, concat, log, template
- 2 HTTP tools: http:get, http:post
- Health endpoint (no auth required)
- No database, no LLM, no auth service needed
