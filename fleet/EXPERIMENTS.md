# konf-prime fleet — v1 experiments

Three sovereign-agent containers running in parallel, each with the
same genesis prompt, tool access, and self-modification rights. They
differ only in which LLM sits behind `ai:complete`.

The point of running them side by side: **learn which model/provider
shape yields a useful autonomous agent with the same seed**. Same
prompt + same tools + different brain = different behavior to study.

## The three variants

| Variant | Port | Brain | Cost posture | What we're testing |
|---|---|---|---|---|
| `prime` (ollama) | 8002 | Ollama qwen3-coder:30b (big GPU, RTX 3090 Ti) | Free local | Can a fully-local agent bootstrap itself using a 30B coder model? Ceiling of what runs on this box without paid API. |
| `prime-gemini` | 8003 | Gemini 2.5 Pro (paid) | Paid cloud | Ceiling of reasoning. What does an agent with the strongest reasoning model available do with the same seed? |
| `prime-hybrid` | 8004 | Default Ollama gemma4:e4b (small GPU, RTX 2070 S); overrides to Gemini 2.5 Pro for orchestration decisions | Mostly free, occasional paid | Cost-efficient shape. Can an agent learn when to "spend" a Gemini call vs use local Gemma for bulk work? |

All three share:

- Same genesis prompt (`products/konf-prime*/config/prompts/genesis.md`).
- Same 6-node orchestrator (`products/konf-prime*/config/workflows/chat.yaml`).
- Same tool whitelist: shell, state, memory, schedule, runner, config:reload, workflow:\*, MCP servers (filesystem/git/github/fetch), `ai:complete`.
- Same self-modification rights: own config dir writable, own `/src` git clone writable, docker socket mounted, sudo NOPASSWD for apt/npm/pip installs.
- Same recovery rails: genesis tells each agent to stage edits to `chat.yaml.new` + parse-check + atomic swap + keep `.bak`.

## The hypotheses worth watching

- **H1: Coding models reason well enough for orchestration.**
  `prime` uses qwen3-coder:30b, tuned for code/tool-calls. We expect
  tight tool loops but shallow self-reflection. Watch for: does it
  ever pause to plan, or does it just hack?
- **H2: Gemini 2.5 Pro's default thinking budget is enough.**
  The rig crate supports `thinking_budget` but konf doesn't pass it
  through yet. With default behavior, does `prime-gemini` already
  out-reason the local models on bootstrap + self-modification tasks?
- **H3: A cheap local model can be driven by a smart remote one.**
  `prime-hybrid` defaults to gemma4:e4b (fast) and has to *choose* to
  invoke Gemini for harder steps. Does it learn the cost/quality
  tradeoff, or does it over- or under-use Gemini?
- **H4: The same seed + different brains produces divergent agents.**
  After a day of running, do the three agents have meaningfully
  different config trees, memory namespaces, schedule graphs? If not,
  the genesis is overconstraining; if yes, it's a useful dial.

## How we track

### Live view — what is happening right now

```bash
bash fleet/watch.sh             # all three, both docker logs and SSE monitor
bash fleet/watch.sh prime       # just one variant
bash fleet/watch.sh --no-docker # only the agent-level SSE events
```

Lines are prefixed `[prime] ...` / `[gemini:mon] ...` so you can see
who is doing what. The SSE stream carries every tool call, LLM
iteration, and workflow outcome. Docker logs carry the process-level
tracing (MCP child starts, errors, scheduler).

### Ground truth — what the agent remembers

Each variant has its own state + memory volume. Probe with:

```bash
# Ask the agent directly (it has memory:search)
curl -sN -H 'content-type: application/json' \
  -d '{"message":"Summarize everything you have stored under namespace konf-prime:self","session_id":"review"}' \
  http://localhost:8002/v1/chat
```

Or inspect the volume directly from a sidecar container:

```bash
docker run --rm -v fleet_prime_state:/data alpine sh -c 'ls -la /data'
```

### Behavior log — what the agent is doing over time

Chat sessions are durable: reusing `session_id=main` across days
gives you a running transcript. The agent can summarize old turns
into memory to keep `chat_history` tractable.

Heartbeats (if the agent sets one up — it's encouraged, not required):
a cron-scheduled workflow that writes `{ts, activity, plan}` to a
memory namespace like `konf-prime:heartbeats` means you can reconstruct
what it did between chats.

### Rust changes — what the agent wants in the substrate

Each variant has `/src` as its own git clone. Branches the agent
creates there are separate from the host working tree; nothing merges
upstream without the user running `git fetch` from the host.

```bash
# See what branches each agent created
for v in prime prime-gemini prime-hybrid; do
  echo "=== $v ==="
  docker exec konf-$v git -C /src branch -a 2>/dev/null | head
done
```

### Cost — when the paid-tier ones spend

Only `prime-gemini` and `prime-hybrid` cost money. Gemini API calls
are logged in the SSE `ai:complete` events; the response carries a
`usage` block in `_meta`. Sum it over a session to get token spend.
`prime-hybrid` should stay cheap if the agent learns when to reach
for Gemini — that's the experiment's core question.

## v1 state-of-play (2026-04-17)

- `prime` (ollama): chat round-trip green, tool use confirmed via SSE.
  Orchestrator runs 6 nodes per turn; `shell:exec` called from inside
  the ReAct loop. **Ready.**
- `prime-hybrid`: chat round-trip green on default local model
  (gemma4:e4b). Escalation to Gemini blocked by the tool-schema 400
  bug (see `genesis.md`, gap #1). **Ready for local-only work, agent
  must patch gap #1 to enable its Gemini-escalation path.**
- `prime-gemini`: container healthy, but the first tool-equipped chat
  turn 400s against Gemini due to the same tool-schema bug.
  **Blocked on gap #1 until the agent (or operator) patches it.**

The gap list lives in `genesis.md` so each agent has it in its
permanent context window. Patching any gap is a perfect first
self-modification exercise: the change is small, the effect is large,
and success exercises the whole edit-build-restart loop.

## What a "successful" experiment looks like at v1

We're not scoring capabilities yet — v1 is about proving the shape
works. Success at v1 =

- All three containers stay up over a multi-hour session without the
  operator re-running smoke.sh.
- Each agent can hold a multi-turn conversation that references
  earlier turns (memory round-trip).
- Each agent uses at least three distinct tools in a chat loop
  (not just `ai:complete`).
- At least one variant edits its own `chat.yaml` safely (stage +
  parse-check + swap) without bricking itself.
- At least one variant schedules a durable tick and uses it.

Anything beyond that — successful Rust self-patches, new MCP servers
the agent installs, cross-agent coordination, actual work delivered —
is v2 territory and tracked per-variant in a followup doc.

## When to retire a variant

- It burns through its context window every chat turn. (Need to teach
  it compaction, or drop it.)
- It hallucinates tool calls that never happen in SSE. (Model is too
  weak for this seed; try a stronger one before retiring.)
- Its Rust edits brick the binary repeatedly. (Tighten genesis
  constraints before giving up.)
- It costs more than it delivers. (Gemini variants — watch this.)
