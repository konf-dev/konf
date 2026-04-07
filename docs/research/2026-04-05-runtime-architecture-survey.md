# Runtime Architecture Survey for Konf Platform

**Date:** 2026-04-05
**Purpose:** Evaluate OS/runtime philosophies for managing workflows, tools, and multi-tenant isolation in the Konf agentic AI platform.

---

## 1. Models Evaluated

### 1.1 Actor Model (Erlang/OTP)

**Philosophy:** Independent actors with private state communicate via async messages. Supervision trees handle fault recovery ("let it crash"). Millions of lightweight processes, no shared state.

**Strengths for Konf:**
- Supervision trees map naturally to: Platform → Tenant → Session → Agent → Tool call
- "Let it crash" suits unpredictable LLM failures — supervisor restarts the agent, context rebuilt from smrti
- Message passing naturally supports streaming (tokens are messages from agent to gateway)
- Process isolation gives per-agent capability scoping
- Dynamic supervisors handle "start agent on demand" perfectly

**Weaknesses:**
- No built-in durability — a crash loses in-flight state unless journaling is added
- No built-in capability lattice (must be layered on)
- Debugging message flows between many actors can be complex

**Rust ecosystem:** `ractor` (Erlang-inspired, supervision, used at Meta, ~1.5k stars), `kameo` (backpressure, lifecycle hooks, ~1k stars), `actix` (mature but no supervision trees, ~8.7k stars)

### 1.2 Durable Execution (Temporal, Restate)

**Philosophy:** Workflow code runs as ordinary code, but the runtime guarantees completion despite failures via event history replay (Temporal) or journaling (Restate).

**Temporal strengths:**
- Long-running workflows survive infrastructure failures
- Child workflow parent-close policies (terminate, cancel, abandon)
- Namespace-based multi-tenancy
- Rich monitoring (visibility API, search attributes, web UI)

**Temporal weaknesses for Konf:**
- **Determinism constraint kills AI use cases.** Workflow code must be deterministic for replay. LLM calls are inherently non-deterministic. Every LLM call must be wrapped as an "activity," adding boilerplate.
- **No streaming in the durable path.** Temporal's model is request-response, not streaming. Can't stream LLM tokens through workflow execution.
- **Heavy infrastructure.** Requires Cassandra/MySQL/Postgres + multiple services (Frontend, History, Matching, Worker). Too complex for self-hosted deployment.
- **No Rust SDK for writing workflows.** Rust core exists but only as a foundation for other language SDKs.

**Restate strengths:**
- Single Rust binary — operationally simple
- Virtual Objects (actor-like, keyed, serialized per key) — natural for per-user agents
- Built-in state per object (no external store)
- Journaling without strict determinism constraints

**Restate weaknesses:**
- No streaming in durable path
- Less mature ecosystem
- HTTP invocation model adds latency for in-process tools

### 1.3 Unix/Plan 9 Process Model

**Philosophy:** Processes with kernel-enforced isolation. Plan 9 extends with per-process namespaces — each process sees a different filesystem view. Resources are mounted into a process's namespace, providing capability-based access.

**Plan 9's key insight for Konf:** Instead of passing `namespace_id` to every memory call, the runtime "mounts" a smrti namespace into the workflow's context. The workflow calls `memory:search` — the runtime translates it to `smrti.search(namespace="user_123")`. The LLM never sees the namespace parameter, preventing prompt injection.

**Strengths:** Hard isolation, resource limits (cgroups), well-understood security
**Weaknesses:** Too heavyweight for per-step isolation (process creation is ~1000x actor spawn)

### 1.4 CSP (Communicating Sequential Processes)

**Philosophy:** Anonymous processes communicate through typed channels with synchronous rendezvous. Identity is in the channel topology, not the processes. Go's goroutines + channels.

**Strengths:** Simple, natural backpressure with bounded channels, excellent for streaming pipelines
**Weaknesses:** No supervision, no fault tolerance primitives, no process identity (can't address specific workers), no hierarchy

**Verdict:** Good as a communication mechanism within a broader model, not as the primary architecture.

### 1.5 Kubernetes Controller Pattern

**Philosophy:** Declarative desired state + reconciliation loops. Controllers watch resources and converge actual state toward desired state. Level-triggered (idempotent), not edge-triggered (event-driven).

**Key insight for Konf:** Agent sessions as "resources" with spec (from YAML config) and status. A controller reconciles: if spec says "agent should be running," but no process exists, start one. If agent crashes, controller notices and restarts.

**Strengths:** Extremely robust (no lost events), naturally config-driven, well-understood
**Weaknesses:** Reconciliation latency (seconds, not milliseconds), etcd not designed for high-throughput status updates

### 1.6 Fuchsia Component Framework

**Philosophy:** Zero ambient authority. Components start with nothing — capabilities (tools, services, memory access) must be explicitly routed from parent to child via a declarative manifest. No process can access anything it wasn't explicitly granted.

**Key insight for Konf:** This is exactly the capability lattice we already have, but formalized as the core security model. A parent workflow grants `memory:search[namespace=user_123]` to a child — the child literally cannot access any other namespace.

---

## 2. Analysis Against Konf Requirements

| Requirement | Actor (OTP) | Temporal | Restate | K8s Pattern | Fuchsia | Plan 9 |
|---|---|---|---|---|---|---|
| Multi-tenant isolation | Tree per tenant | Namespaces | Virtual Object keys | Namespaces + RBAC | Capability routing | Per-process namespaces |
| AI agent workloads (slow IO) | Async messages | Activities + heartbeats | ctx.run() | Pods | N/A | N/A |
| **Streaming responses** | **Native (messages)** | Not built-in | Not in durable path | N/A | N/A | N/A |
| **Config-driven products** | App config | Code-only | Code-only | **CRDs are YAML** | **Declarative manifests** | N/A |
| **Capability scoping** | Process isolation | Application-level only | Resource injection | RBAC | **Zero ambient authority** | **Namespace mounting** |
| Process tree monitoring | Observer tool | Visibility API | Not built-in | kubectl API | Component tree | /proc |
| Fault tolerance | Supervision trees | Durable replay | Journaling | Reconciliation | Restart policies | Signals |
| Self-hostable simplicity | Single binary | Multi-service + DB | Single binary | Heavy | N/A | N/A |

**The clear winner for the core execution model is OTP-style supervision.**

No other model handles streaming, fault tolerance, parent-child hierarchies, and dynamic process management as naturally. The weaknesses (no durability, no built-in capabilities) are addressed by borrowing from other models.

---

## 3. Recommended Architecture: Hybrid

**The Konf runtime should be a synthesis of four models:**

### Layer 1: OTP Supervision Tree (core execution)

Each workflow execution is a supervised actor. The tree structure:

```
PlatformSupervisor (one_for_one)
├── TenantSupervisor:acme (one_for_one)
│   ├── SessionSupervisor:sess_001 (one_for_all)
│   │   ├── GatewayAgent ←── SSE to frontend
│   │   ├── ContextWorkflow (transient, completes and dies)
│   │   └── ChatWorkflow (transient)
│   │       ├── ai:complete tool call (transient child)
│   │       └── memory:search tool call (transient child)
│   └── SessionSupervisor:sess_002 ...
└── TenantSupervisor:personal (one_for_one)
    └── ...
```

Why: Each agent is a lightweight actor with a message channel. Streaming is just messages from child to parent. Crash recovery is automatic — supervisor restarts the agent, context is rebuilt from smrti.

### Layer 2: Fuchsia Capability Routing (permissions)

Zero ambient authority. Every workflow/actor starts with nothing. Capabilities are explicitly granted by the parent:

```yaml
# The system root grants to tenant supervisor:
grant:
  - memory:*[namespace=acme:*]
  - ai:complete
  - ai:stream
  - tools:web_search

# Tenant supervisor grants to user session:
grant:
  - memory:search[namespace=acme:user_123]
  - memory:store[namespace=acme:user_123]
  - ai:complete
  # web_search NOT granted — this user's plan doesn't include it
```

**Parameterized capabilities** (not just `memory:search` but `memory:search[namespace=X]`) prevent namespace escaping. The runtime enforces this at dispatch time — the LLM never sees the namespace parameter.

### Layer 3: Plan 9 Context Virtualization (resource access)

Instead of passing namespace/user_id as tool parameters, the runtime "mounts" a virtualized context:

```
The workflow sees:        What actually happens:
memory:search(query)  →   smrti.search(query, namespace="acme:user_123")
memory:store(content) →   smrti.add_nodes(content, namespace="acme:user_123")
profile:get()         →   db.get_profile(user_id="user_123")
```

The namespace is injected by the runtime, not by the workflow or the LLM. This prevents:
- Prompt injection changing the namespace
- Accidental cross-tenant data access
- Tools needing to know about multi-tenancy

### Layer 4: K8s Controller Pattern (lifecycle management)

The config-driven product model uses desired-state reconciliation:

1. Product config (YAML) declares: "this product needs these workflows, these tools, these capabilities"
2. The runtime controller reads the config and ensures the right supervisors/actors exist
3. When a user connects, the controller creates a session with the right scope
4. If anything crashes, the controller reconciles back to desired state

This is level-triggered — if the platform restarts, it reads all active sessions from the database and recreates the supervision tree. No events lost.

### NOT using: Temporal's durable replay

AI agents are non-deterministic. Temporal's deterministic replay doesn't fit. Instead:
- **Journaling for audit** (inspired by Restate): record every significant event in an append-only log (which is already what smrti's event sourcing does)
- **Recovery from state, not replay**: on crash, rebuild agent context from smrti graph + session state, not by replaying workflow steps

---

## 4. Rust Ecosystem Choices

| Component | Crate | Why |
|---|---|---|
| Actor framework | `ractor` | Most Erlang-like, has supervision trees, used at Meta, active development |
| Alternative | `kameo` | Better backpressure + lifecycle hooks, younger but promising |
| Channels | `tokio::sync::mpsc/broadcast` | Standard, fast, already used in konflux |
| Concurrent state | `dashmap` | Lock-free concurrent hashmap for process table |
| Cancellation | `tokio-util::CancellationToken` | Standard, propagates through tree |
| MCP (remote tools) | `rmcp` | Rust MCP implementation, already planned |
| Serialization | `serde` + `serde_json` | Already used everywhere |

**Key decision: `ractor` vs `kameo` vs raw tokio tasks?**

| | ractor | kameo | Raw tokio tasks |
|---|---|---|---|
| Supervision | Full OTP-style | Links + lifecycle hooks | Manual |
| Message typing | Strongly typed | Strongly typed | Channels (typed) |
| Backpressure | Bounded mailbox | Built-in | Bounded channels |
| Learning curve | Medium (actor concepts) | Medium | Low |
| Overhead | Low | Low | Minimal |
| Maturity | Higher (Meta usage) | Lower | Production (tokio itself) |

**Recommendation:** Start with `ractor` for the supervision tree and actor lifecycle. It provides the OTP patterns we need without reinventing them. If it proves too heavy or constraining, dropping down to raw tokio tasks + manual supervision is straightforward since the conceptual model is the same.

---

## 5. What This Means for Existing Code

### konflux-core (workflow engine)

**Stays mostly as-is.** The workflow engine executes individual workflows. The runtime wraps it — each workflow execution happens inside a supervised actor. Changes needed:
- CancellationToken support (already planned)
- Execution hooks (so the actor can observe node-level events)
- The engine doesn't need to know about actors — it's called by the actor

### smrti (memory)

**No changes.** The runtime provides a virtualized context that injects namespace into smrti calls. smrti continues to be a dumb storage layer.

### konf-runtime (new crate)

**The new piece.** Implements:
- Supervision tree (via ractor or kameo)
- Capability routing with parameterized grants
- Context virtualization (namespace injection)
- Process table and monitoring
- Session lifecycle management
- Event journal for audit

### konf-backend (Python/FastAPI)

**Thin API layer.** Calls konf-runtime via PyO3 for:
- Starting sessions (creates supervised actor tree)
- Sending messages (routed to gateway actor)
- Streaming responses (reads from actor's output channel)
- Monitoring (queries process table)
- Admin (cancel, kill, inspect)

---

## 6. Open Questions

1. **ractor vs kameo vs raw tasks** — needs prototyping to evaluate ergonomics and performance
2. **Parameterized capabilities** (`memory:search[namespace=X]`) — how to parse and enforce efficiently? Glob matching with parameters?
3. **Session recovery** — when the platform restarts, how exactly do we rebuild the actor tree from database state?
4. **MCP for remote tools** — do remote tools participate in the supervision tree, or are they just HTTP calls?
5. **Journaling** — should the runtime journal to its own store (RocksDB), or reuse smrti's event log, or Postgres directly?

---

## Sources

- Erlang/OTP Supervision Principles (erlang.org/doc)
- Temporal.io Architecture Docs (docs.temporal.io)
- Restate.dev Documentation (docs.restate.dev)
- Fuchsia Component Framework (fuchsia.dev)
- Plan 9 Namespace Paper (doc.cat-v.org)
- Inngest Concurrency & Rate Limiting Docs
- Kubernetes Controller Pattern (kubernetes.io/docs)
- Ractor GitHub (github.com/slawlor/ractor)
- Kameo GitHub (github.com/tqwewe/kameo)
- "Orchestrating AI Agents with Elixir's Actor Model" (freshcodeit.com)
- "Actor Model for Antifragile Serverless Architectures" (arXiv:2306.14738)
- LangGraph Persistence & Checkpointing Docs
- Letta Core Architecture (docs.letta.com)
