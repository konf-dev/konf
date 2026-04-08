# Echo Workflow Example

A workflow that uses only builtin tools. No LLM, no database, no external services.

## Run

```bash
KONF_CONFIG_DIR=examples/echo-workflow/config KONF_DEV_MODE=true cargo run --bin konf-backend
```

## Test

```bash
# SSE streaming chat
curl -N -X POST http://localhost:8000/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Alice"}'
```

## What it proves

- Workflow YAML parsing from config/workflows/
- Workflow registers as a tool (workflow_greet)
- SSE streaming: start → events → done
- Builtin echo tool executes
