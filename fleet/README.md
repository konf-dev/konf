# konf/fleet

Container scaffolding for the konf agent fleet. Phase 0 proved one
container boots; Phase 1 proved the single container runs the full
substrate end-to-end. Phase 2 (multi-container) is still deferred —
see `konf-genesis/scratchpad/STAGE_13_PROGRESS.md`.

This directory is deliberately separate from `konf/sandbox/` (older
shell-only container concept) and from the root `docker-compose.yml`
(stale, assumes Postgres + Supabase). Both are left as-is pending a
broader sweep.

## What lives here

- `fleet-compose.yml` — runs one `konf-backend` container per product.
  Currently a single `_fleet-probe` service. Ramps to N services as
  real products land.

## Phase 0 — green (2026-04-17)

Minimal container boots end-to-end:

1. `docker build -t konf-backend:phase0 /home/bert/Work/orgs/konf-dev-stack/konf`
2. `docker compose -f fleet-compose.yml up -d probe`
3. `GET /v1/health` → 200
4. `GET /v1/monitor/stream` → SSE opens under `KONF_DEV_MODE=1`
5. `POST /v1/chat` → `hello` echoes

## Phase 1 — green (2026-04-17)

The single container runs the full substrate matrix. Try any of the
chat control words documented in `../products/_fleet-probe/README.md`,
e.g. `{"message":"shell"}`, `{"message":"store"}`, `{"message":"runner"}`.

10/10 matrix items are exercised via `/v1/chat`; 23 frictions
catalogued in `STAGE_13_PROGRESS.md` (0 kernel bugs).

## Phase 2 — not started

Multi-container deferred until one real workload (the planned **code
audit** product) has run through the single probe. Findings #9
(shared surreal journal) and #10 (surreal 30 GB block cache) from the
Phase 0 ledger are the architectural triggers that reopen Phase 2.
