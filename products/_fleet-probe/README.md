# _fleet-probe

Minimal konf product for the fleet Phase 0 gate check. Boots `konf-backend`
in a Docker container with:

- redb-backed journal at `/var/lib/konf/konf.redb` (volume-mounted)
- surreal memory (embedded) at `/var/lib/konf/memory.db`
- one trivial workflow (`hello`) using the `echo` primitive only
- no LLM, no shell, no secrets, no HTTP

Purpose: prove that `konf-backend` in a container serves `/v1/health`,
`/v1/chat`, and `/v1/monitor/stream` against a minimal config. If this fails,
that failure is the first real konf issue the fleet discovers.

Not a user-facing product — delete or keep archived once Phase 0 passes.
