# Konf Runtime — Architectural Recommendation

**Date:** 2026-04-05
**Companion:** `2026-04-05-runtime-architecture-survey.md`

---

## My Recommendation

**Build an OTP-inspired supervision tree in Rust, with Fuchsia-style capability routing and Plan 9-style context virtualization. Don't use Temporal. Don't over-abstract.**

Here's why, and what specifically to build.

---

## Why OTP, Not Temporal

Three facts about AI agent workloads that eliminate Temporal-style durable execution:

1. **LLM calls are non-deterministic.** Temporal requires deterministic workflow code for replay. AI agents can't provide this. You'd end up wrapping every LLM call as an "activity" and fighting the framework.

2. **Streaming is the primary output mode.** Temporal is request-response. There's no way to stream LLM tokens through Temporal's execution model. You'd need a side-channel, defeating the purpose.

3. **Recovery from state, not replay.** When an agent crashes mid-conversation, you don't replay its reasoning — you rebuild context from smrti (memory graph + session state + conversation history) and continue. smrti IS your durability layer. You don't need another one.

The OTP model works because:
- Each agent is a lightweight process (actor) that receives messages and sends messages
- Streaming is just messages from child to parent
- Crash recovery is: supervisor restarts actor → actor loads context from smrti → continues
- Supervision trees give you the process hierarchy you need for monitoring

---

## What To Actually Build

### The Runtime Crate (`konf-runtime`)

One Rust crate that provides five things:

#### 1. Supervision Tree

Using `ractor` (or raw tokio tasks if ractor proves too heavy). The tree:

```
RuntimeSupervisor
├── Tenant:acme
│   ├── Session:user_123:sess_abc
│   │   └── WorkflowActor (runs konflux engine)
│   │       ├── ToolCall:memory_search (transient)
│   │       └── ToolCall:ai_complete (transient, streaming)
│   └── Session:user_456:sess_def
│       └── ...
├── Tenant:personal
│   └── ...
└── SchedulerActor (manages cron jobs)
```

**Key design choice: actors or tasks?**

I lean toward **raw tokio tasks + channels**, not a full actor framework. Here's why:

- konflux already uses `JoinSet` + channels. Adding ractor would mean two concurrency models in the same codebase.
- Supervision is ~100 lines of code when you have tokio. A supervisor is a task that spawns children and restarts them on failure.
- Actor frameworks add message type boilerplate that doesn't pay for itself when most communication is "here's a JSON blob."
- If we later need ractor's features (distributed actors, process registry), we can adopt it. Starting simpler is better.

The supervision tree is a data structure (`ProcessTable`) + a set of conventions, not a framework.

#### 2. Parameterized Capability Routing

Extend the existing capability lattice from `capability.rs` with parameter binding:

```rust
// Current: "memory:search" (matches tool name)
// New: "memory:search[namespace=acme:user_123]" (matches tool name + injects params)

pub struct CapabilityGrant {
    pub pattern: String,           // "memory:*" or "ai:complete"
    pub bindings: HashMap<String, Value>,  // {"namespace": "acme:user_123"}
}
```

When a tool is invoked, the runtime:
1. Checks if the tool name matches any granted capability
2. Injects the bound parameters into the tool input (overriding anything the LLM tried to set)
3. The LLM never sees `namespace` as a parameter — it's invisible

This is Plan 9-style context virtualization + Fuchsia zero-ambient-authority in one mechanism.

#### 3. Process Table + Monitoring

Same as the plan — `DashMap<RunId, WorkflowRun>` with process tree, status tracking, and metrics. This is the `/proc` filesystem equivalent.

#### 4. Session Lifecycle

A session represents a connected user. It owns:
- A supervision context (restart policy for child workflows)
- A capability scope (what this user/product can do)
- A smrti namespace binding (injected into all tools)
- A stream channel (for SSE to the frontend)

Sessions are created on first message and cleaned up on disconnect/timeout.

#### 5. Event Journal

Append-only log of runtime events. NOT for durable replay — for audit, debugging, and billing.

**Where to store it:** Postgres, via smrti's existing connection. A simple `runtime_events` table:

```sql
CREATE TABLE runtime_events (
    id BIGSERIAL PRIMARY KEY,
    run_id UUID NOT NULL,
    session_id TEXT NOT NULL,
    namespace TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload JSONB,
    created_at TIMESTAMPTZ DEFAULT NOW()
);
CREATE INDEX idx_runtime_events_run ON runtime_events (run_id);
CREATE INDEX idx_runtime_events_session ON runtime_events (session_id, created_at);
```

Don't use RocksDB (adds a dependency). Don't use smrti's event log (it's for graph mutations, not runtime events). Keep it simple — Postgres is already there.

---

## What NOT To Build

1. **Don't build a distributed actor system.** All workflows run in-process. Remote tools are just HTTP/MCP calls from the tool implementation. Distribution is a future problem.

2. **Don't build durable replay.** smrti is your durability. Recovery = reload context from graph + session state. No event replay.

3. **Don't build a custom actor framework.** Use tokio tasks + channels + a ProcessTable data structure. The supervision logic is ~200 lines. If ractor is needed later, adopt it then.

4. **Don't build a scheduler into the runtime.** The backend's Postgres job queue handles scheduling. The runtime just executes what it's told.

5. **Don't build WASM sandboxing yet.** Tools are Python functions called via PyO3. WASM sandboxing for untrusted tools is a future feature.

---

## How It Fits Together

```
User sends message
    ↓
konf-backend (Python/FastAPI)
    │ verify JWT, load project config
    │ find or create session in runtime
    ↓
konf-runtime (Rust, via PyO3)
    │ session.handle_message(message)
    │ creates CapabilityScope from config
    │ creates WorkflowRun in ProcessTable
    │ spawns supervised task
    ↓
konflux engine.run(workflow, input, capabilities)
    │ executes YAML workflow DAG
    │ calls tools (memory:search → runtime injects namespace)
    │ streams tokens via channel
    ↓
konf-runtime
    │ forwards stream events to session channel
    │ logs runtime events to Postgres
    │ updates ProcessTable status
    ↓
konf-backend
    │ pipes stream events as SSE to frontend
    │ persists message/response to Postgres
    ↓
Frontend receives streamed response
```

---

## Implementation Order

1. **konflux-core: CancellationToken + hooks + global timeout** (prerequisite, ~2 hrs)
2. **konf-runtime: ProcessTable + CapabilityScope + supervision** (core runtime, ~1 week)
3. **konf-runtime: parameterized capabilities + context virtualization** (~2 days)
4. **konf-runtime: monitoring API + event journal** (~2 days)
5. **konflux-python: update bindings to expose Runtime** (~1 day)
6. **konf-tools: Python tool implementations** (parallel with above)
7. **konf-backend: FastAPI server using runtime** (after runtime is solid)
8. **Unspool migration** (validation)

---

## Key Insight

The runtime is simpler than it sounds. Strip away the OS analogies and it's:

- A hashmap of running workflows (`ProcessTable`)
- A tree of parent-child relationships (`parent_id` field)
- A permission check before each tool call (parameterized capabilities)
- A namespace injection before each smrti call (context virtualization)
- A channel for streaming events out (already exists in konflux)
- A way to cancel a workflow (CancellationToken)
- A log of what happened (Postgres table)

That's it. No actor framework, no durable replay, no distributed consensus. The complexity is in getting the details right (race conditions, cleanup, error propagation), not in the conceptual model.

The ambitious vision (OS-like management with process trees and capability routing) is achieved through simple, composable primitives — not through adopting a heavy framework.
