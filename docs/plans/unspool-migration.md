# Unspool Migration Plan

> **Note:** This plan predates the colon naming convention. Tool names below use the current kernel format.

**Goal:** Validate the Konf platform by migrating Unspool to run entirely as config + workflows.
**Depends on:** Phase D (transport shells working) in [master-plan.md](master-plan.md)
**Success criteria:** All existing Unspool functionality works via tools.yaml + workflows/ + prompts/.

---

## What Unspool currently does

From the codebase audit (`../unspool-life/unspool/backend/`):

1. **Hot path (chat):** User message → context assembly (parallel SQL queries) → LLM with tool calling → stream response → persist events
2. **Cold path (extraction):** Debounced (180s) → extract entities/relationships from conversation → semantic dedup → write to graph
3. **Synthesis (nightly):** Archive done items → merge duplicates → edge decay → recompute actionable flags
4. **Maintenance (hourly):** Check deadlines → execute scheduled actions → expire items
5. **Proactive:** On initial load → evaluate triggers → generate proactive message if conditions met

## What becomes config

| Unspool component | Konf equivalent |
|---|---|
| `hot_path/context.py` (context assembly) | `workflows/context.yaml` |
| `hot_path/graph.py` (LangGraph agent) | `workflows/chat.yaml` |
| `cold_path/extractor.py` | `workflows/extraction.yaml` |
| `cold_path/synthesis.py` | `workflows/synthesis.yaml` |
| `proactive/engine.py` | `workflows/proactive.yaml` |
| `jobs/` (hourly/nightly) | `workflows/maintenance.yaml` + `schedules.yaml` |
| `core/config_loader.py` (hyperparams) | `project.yaml` + `models.yaml` |
| `agents/hot_path/tools.py` | `tools.yaml` + custom tool functions |
| `prompts/agent_system.md` | `prompts/system.md` |
| `prompts/extraction_system.md` | `prompts/extraction.md` |

## Config directory structure

```
unspool-config/
├── project.yaml
├── models.yaml
├── tools.yaml
├── schedules.yaml
├── prompts/
│   ├── system.md
│   └── extraction.md
├── workflows/
│   ├── context.yaml
│   ├── chat.yaml
│   ├── extraction.yaml
│   ├── synthesis.yaml
│   ├── proactive.yaml
│   └── scheduled/
│       └── maintenance.yaml
└── tools/
    ├── plate.py          # get_plate_items custom tool
    └── stats.py          # compute_stats custom tool
```

## project.yaml

```yaml
name: unspool
version: 1

workflows:
  chat:
    trigger: message
    pipeline:
      - workflows/context.yaml
      - workflows/chat.yaml
    stream: true
    capabilities:
      - ai:complete
      - ai:stream
      - memory:search
      - memory:store
      - memory:traverse
      - memory:aggregate
      - profile:get
      - history:recent
      - custom:get_plate
      - custom:compute_stats
      - schedule:reminder

  extraction:
    trigger: event
    event: chat_completed
    workflow: workflows/extraction.yaml
    debounce: 180s
    capabilities:
      - ai:complete
      - memory:search
      - memory:store

  synthesis:
    trigger: cron
    schedule: "0 3 * * *"
    workflow: workflows/synthesis.yaml
    capabilities:
      - ai:complete
      - memory:search
      - memory:store
      - memory:retract

  maintenance:
    trigger: cron
    schedule: "0 * * * *"
    workflow: workflows/scheduled/maintenance.yaml
    capabilities:
      - memory:search
      - memory:aggregate
      - schedule:execute

  proactive:
    trigger: session_start
    workflow: workflows/proactive.yaml
    capabilities:
      - memory:search
      - memory:aggregate
      - ai:complete
      - profile:get

scheduler:
  provider: builtin
  poll_interval: 10s

cache:
  provider: builtin
```

## Validation checklist

- [ ] Chat: send message, get streamed response with context from memory
- [ ] Tool calling: agent calls memory:search, memory:store during chat
- [ ] Extraction: after chat, extraction workflow triggers (debounced 180s)
- [ ] Extraction dedup: duplicate nodes not created (semantic similarity check)
- [ ] Synthesis: nightly run archives done items, merges duplicates, decays edges
- [ ] Maintenance: hourly run checks deadlines, executes scheduled actions
- [ ] Proactive: on session start, evaluate triggers, generate message if applicable
- [ ] Reminders: "remind me at 3pm" → scheduled job → fires at correct time
- [ ] Namespace isolation: user A cannot access user B's data
- [ ] Capability scoping: extraction workflow cannot call ai:stream (not in its capability list)
- [ ] Config change: modify prompt, hot-reload, verify new behavior
- [ ] Error recovery: kill backend, restart, sessions rebuild from the memory backend

## What's NOT migrated (stays in Unspool frontend)

- React PWA (Vercel deployment)
- Push notifications (web push)
- Stripe billing
- Email alias handling

These are frontend/infrastructure concerns that don't touch the platform.
