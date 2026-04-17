# _fleet-probe

The fleet's substrate-exercise product. Boots `konf-backend` in a Docker
container and moves real bytes through every primitive on the Stage 13
substrate matrix via `/v1/chat`. Not a user-facing product — retained
because it's the cheapest way to prove and regression-test the kernel
end-to-end.

## What's wired

- **redb journal** at `/var/lib/konf/konf.redb` (volume `probe_state`)
- **surreal memory** (embedded RocksDB) at `/var/lib/konf/memory.db`
- **Ollama LLM** via `host.docker.internal:11434` (no API key)
- **shell:exec** with deny-rule `tool_guards` (sudo, rm -rf, rm -fr)
- **workflows**:
  - `chat.yaml` — `chat_router` (19 nodes, 12 control-word routes)
  - `attenuation_target.yaml` — child workflow for Phase 1 step 9
  - `probe_added.yaml` — sample workflow (originally added as the
    config:reload test artifact; now a permanent echo smoke workflow)
  - `hello.yaml` — boot-gate echo kept from Phase 0

## /v1/chat routes (control words)

Send `{"message": "<word>"}` to `POST /v1/chat`:

| Word | Primitive proven |
|---|---|
| `store` / `search` | `memory:store` / `memory:search` cross-run + restart |
| `shell` | `shell:exec` + output-based `when:` branching |
| `shell-sudo` / `shell-rm` | guard deny + graceful `catch:` recovery |
| `template` | `template` builtin with `{% raw %}` workaround |
| `runner` | `runner:spawn` + `runner:wait` async pattern |
| `schedule` | `schedule:create` durable timer (2 s delay) |
| `attenuate-ok` / `attenuate-deny` | per-node `grant:` capability attenuation |
| `reload` | `config:reload` pull-mode hot reload |
| anything else | `ai:complete` passthrough to Ollama |

## Bring it up

```
docker compose -f ../../fleet/fleet-compose.yml up -d probe
curl http://localhost:8000/v1/health
curl -X POST http://localhost:8000/v1/chat -H 'content-type: application/json' \
  -d '{"message":"shell"}'
```

Phase 1 is documented step by step in
`konf-genesis/scratchpad/STAGE_13_PROGRESS.md`, including the 23-entry
friction ledger (0 kernel bugs).
