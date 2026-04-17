# code-audit

The Phase 1 anchor workload. Audits a konf crate using the substrate
primitives proven on `_fleet-probe`: `shell:exec` to read files,
`ai:complete` to review them, `memory:store` to persist findings, and
`template` to assemble a human-readable summary.

## v0 scope

- Hardwired target: `crates/konf-tool-llm` (3 files, 1237 lines)
- Audits `introspect.rs` and `validate.rs` (skipping the 904-line
  `lib.rs` until we know how qwen3-coder:30b handles ~200-line files)
- Default model: `qwen3-coder:30b` (override via `KONF_MODEL`)
- Memory namespace: `konf:audit:konf-tool-llm:v0`

## Bring it up

```
docker compose -f ../../fleet/fleet-compose.yml up -d audit
curl http://localhost:8001/v1/health
curl -X POST http://localhost:8001/v1/chat \
  -H 'content-type: application/json' \
  -d '{"message":"audit"}'
```

The audit always runs the same pipeline regardless of the message
content (v0). Output is a markdown report on the SSE `done` event.

## Why a separate container

One product per container per Stage 13 doctrine. The probe and the
audit share the host Ollama and the host filesystem (audit's source is
bind-mounted read-only at `/audit-target`), but they have separate
state volumes — no shared surreal journal yet (friction #9 in
`STAGE_13_PROGRESS.md`).
