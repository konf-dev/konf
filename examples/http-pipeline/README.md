# HTTP Pipeline Example

Multi-step workflow: fetch data from an API, extract a field.

## Run

```bash
KONF_CONFIG_DIR=examples/http-pipeline/config KONF_DEV_MODE=true cargo run --bin konf-backend
```

## Test

```bash
curl -N -X POST http://localhost:8000/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "fetch"}'
```

## What it proves

- HTTP tool makes real external requests
- json_get extracts nested fields
- Multi-step sequential DAG (fetch → extract)
- SSRF protection allows public URLs
