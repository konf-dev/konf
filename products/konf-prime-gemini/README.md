# konf-prime

A sovereign konf agent container. Inside its own namespace the agent is
root — it can edit its own workflows, call `config:reload`, read/write
its own state, and use `shell:exec` without guards. Safety comes from
the container boundary and the mount set, not per-command rules.

The v1 shape is intentionally small: one `chat.yaml` workflow that
routes a handful of smoke probes, plus a default agent turn that the
agent itself will eventually replace with a richer system prompt + tool
loop once we sit down to write the orchestrator.

## Smoke probes (all should return `done` cleanly)

```
POST /v1/chat {"message":"probe:smoke-state", "session_id":"s1"}
  → state:* round-trip within a run

POST /v1/chat {"message":"probe:smoke-passthrough", "session_id":"s1"}
  → pure-ref array passthrough to ai:complete messages (#24 fix)

POST /v1/chat {"message":"probe:smoke-self-intro", "session_id":"s1"}
  → ai:complete respects a 3-message history we hand it

POST /v1/chat {"message":"probe:smoke-author", "session_id":"s1"}
  → agent writes workflows/authored.yaml + config:reload; new
    workflow:authored_by_prime tool appears in the registry
```

## Bring it up

```
docker compose -f ../../fleet/fleet-compose.yml up -d prime
curl http://localhost:8002/v1/health
curl -X POST http://localhost:8002/v1/chat \
  -H 'content-type: application/json' \
  -d '{"message":"probe:smoke-passthrough","session_id":"s1"}'
```

## Why "no guards"

Every other konf product so far scopes the LLM with `tool_guards:` in
`tools.yaml`. konf-prime doesn't. The goal is an OS-like container
where the agent has root inside. That keeps the prompt simple (the LLM
doesn't have to discover what's blocked at dispatch) and lets the agent
evolve itself without fighting rule churn. The cost is that you must
treat the container as untrusted from the host's perspective — only
mount read-only what it needs to see, and writable only where it
should write.
