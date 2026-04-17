# konf-prime

A sovereign konf agent container. Inside its own namespace the agent
is root — it can edit its own workflows, call `config:reload`, read
and write its own state and memory, spawn background workflows,
schedule durable ticks, and `shell:exec` anything it wants. Safety
comes from the container boundary and the mount set, not per-command
rules.

`konf-prime` is the **ollama** variant of a three-container fleet.
Sibling variants `konf-prime-gemini` and `konf-prime-hybrid` run the
same genesis + orchestrator with different LLM wiring. See
`../../fleet/EXPERIMENTS.md` for the parallel experiment shape.

## Files the agent owns

```
config/
├── konf.toml              # server + storage config
├── tools.yaml             # llm, memory, shell, mcp_servers
├── models.yaml            # available model catalog (what ai:complete can call)
├── prompts/
│   └── genesis.md         # the agent's system prompt — the whole personality
└── workflows/
    ├── chat.yaml          # /v1/chat entry point; 6-node orchestrator + smoke routes
    ├── chat.yaml.bak      # last known-good snapshot (host-side recovery)
    └── ...                # anything the agent authors at runtime
```

All of `config/` is bind-mounted writable into the container. The
agent edits → `config:reload` → new behavior live.

## The orchestrator

`workflows/chat.yaml` has a single entry point named `route`. It
dispatches on `input.message`:

- `probe:smoke-*` → diagnostic smoke routes that prove substrate
  primitives work.
- anything else → `agent_turn`, a 6-node pipeline:

  | Node | Tool | What it does |
  |---|---|---|
  | `agent_turn` | `state:get` | Read `chat_history` for this session_id |
  | `agent_load_prompt` | `shell:exec` | `cat` the genesis.md from disk |
  | `agent_llm` | `ai:complete` | Run the LLM with system + messages + prompt + full tool whitelist, ReAct loop up to 20 iterations |
  | `agent_append` | `list:append` | Extend history with [user turn, assistant turn] |
  | `agent_save` | `state:set` | Persist new history for this session_id |
  | `agent_return` | `echo` | Return assistant text |

History lives in durable state (redb), keyed by session_id. It
survives container restarts if the same session_id is re-used.

## Mounts

| Host path | Container path | Mode | Purpose |
|---|---|---|---|
| `../products/konf-prime/config` | `/etc/konf/products/konf-prime/config` | rw | Agent edits itself |
| `fleet_prime_src` (docker volume) | `/src` | rw | Agent's own git clone of konf; edits Rust here on branches |
| `/var/run/docker.sock` | `/var/run/docker.sock` | rw | Agent can `docker build` + `docker restart` itself |
| `fleet_prime_state` (docker volume) | `/var/lib/konf` | rw | Durable state (redb journal + surreal memory) |

## Bring it up

One-time setup (once per machine):

```bash
# 1. Build the image (once, or whenever Dockerfile changes)
cd konf && docker build -t konf-backend:phase0 .

# 2. Populate each prime_*_src volume with a fresh copy of the konf source
for v in fleet_prime_src fleet_prime_gemini_src fleet_prime_hybrid_src; do
  docker volume create "$v"
  docker run --rm \
    -v "$(pwd):/host:ro" -v "$v:/dest" \
    alpine sh -c 'cp -a /host/. /dest/ && rm -rf /dest/target /dest/node_modules && chown -R 999:999 /dest'
done

# 3. (Optional, for multi-GPU) Start a second ollama on port 11435 pinned to GPU 0
setsid env CUDA_VISIBLE_DEVICES=0 OLLAMA_HOST=0.0.0.0:11435 /usr/bin/ollama serve \
  </dev/null >/tmp/ollama-small.log 2>&1 &
```

Run one or all variants:

```bash
# Just the ollama variant (no API keys required)
docker compose -f fleet/fleet-compose.yml up -d prime

# Gemini + hybrid variants (require GEMINI_API_KEY in shell env)
export GEMINI_API_KEY=...
docker compose -f fleet/fleet-compose.yml up -d prime-gemini prime-hybrid

# Everything
docker compose -f fleet/fleet-compose.yml up -d
```

## Talk to it

```bash
curl -sN -H 'content-type: application/json' \
  -d '{"message":"Hello","session_id":"s1"}' \
  http://localhost:8002/v1/chat
```

Ports: `prime` = 8002, `prime-gemini` = 8003, `prime-hybrid` = 8004.

## Watch it

```bash
bash fleet/watch.sh                  # every stream, all variants
bash fleet/watch.sh prime            # one variant
bash fleet/watch.sh --no-monitor     # just docker logs
```

## Smoke probes — all should return `done` cleanly

```
POST /v1/chat {"message":"probe:smoke-state", "session_id":"s1"}
  → state:* round-trip within a run

POST /v1/chat {"message":"probe:smoke-passthrough", "session_id":"s1"}
  → pure-ref array passthrough to ai:complete messages

POST /v1/chat {"message":"probe:smoke-self-intro", "session_id":"s1"}
  → ai:complete respects a 3-message history we hand it

POST /v1/chat {"message":"probe:smoke-author", "session_id":"s1"}
  → agent writes workflows/authored.yaml + config:reload; new
    workflow:authored_by_prime tool appears in the registry
```

```
bash products/konf-prime/smoke.sh
```

## Why "no guards"

Every other konf product so far scopes the LLM with `tool_guards:` in
`tools.yaml`. konf-prime doesn't. The goal is an OS-like container
where the agent has root inside. The prompt is simpler (the LLM
doesn't have to discover what's blocked at dispatch) and the agent
can evolve itself without fighting rule churn. The cost is that you
must treat the container as untrusted from the host's perspective —
only mount read-only what it needs to see, and writable only where
it should write.
