# konf/fleet

Phase 0 scaffolding for the konf agent fleet — see
`konf-genesis/scratchpad/STAGE_13_HANDOFF.md` and the in-progress Stage 13
plan.

This directory is intentionally separate from `konf/sandbox/` (older
shell-only container concept) and from the root `docker-compose.yml`
(stale, assumes Postgres + Supabase). Those two are kept as-is for now
until Phase 0 proves the fleet path end-to-end.

## What lives here

- `fleet-compose.yml` — runs one `konf-backend` container per product.
  Starts with a single `_fleet-probe` service. Ramps to N services as
  real products land (auditor, journaler, orchestrator, …).

## Gate check (Phase 0)

1. `docker build -t konf-backend:phase0 /home/bert/Work/orgs/konf-dev-stack/konf`
2. `docker compose -f fleet-compose.yml up -d probe`
3. `curl http://localhost:8000/v1/health` — expect HTTP 200.
4. `curl -N http://localhost:8000/v1/monitor/stream` with `KONF_DEV_MODE=1`
   server-side — expect SSE connection to open.
5. Spawn the `hello` workflow via `/v1/chat` — expect the echo output.

If all four pass, Phase 0 is green and we move to Tier 0 (orchestrator
hand-scaffold + konf-hammer full-product drafter).
