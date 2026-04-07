# Real-World Experiences & Ecosystem Deep Dive

**Date:** 2026-04-06
**Companion:** `2026-04-05-runtime-architecture-survey.md`, `2026-04-05-runtime-recommendation.md`

---

## 1. Temporal for AI Agents — What Actually Happened

### It works at scale (with significant effort)

**Replit** runs every Agent as a Temporal Workflow. Hundreds of thousands of production runs. Their verdict: *"Temporal has never been the bottleneck."* Multi-region replication saved them during a cloud provider degradation.

**OpenAI Codex** is built on Temporal. They describe it as *"responsible for executing the core control flows, allowing reasoning about concurrency, correctness, and fault tolerance."*

**Dust.tt** migrated to Temporal because with reasoning-capable agents running 15+ minutes, their synchronous stateless architecture couldn't survive failures.

### It hurts in specific ways

**Grid Dynamics** had the most detailed public failure. They built an agentic AI prototype with LangGraph + Redis:
- *"An endless stream of issues, including race conditions, stale state, and agents getting stuck without clear reporting."*
- After migrating to Temporal: *"One of the most satisfying aspects was the opportunity to delete thousands of lines of custom retry and error handling code."*

**xgrid's production pitfalls analysis** identified concrete issues:
1. **Conversation history bloat** — long conversations fill event history, breach payload limits. Need claim-check pattern (store externally).
2. **Temporal is not a database** — storing conversation history in workflow state creates latency. Querying requires an active worker to replay and respond.
3. **Orphaned side effects** — tool calls that modify databases don't get reversed on cancellation. Compensation must be designed as first-class.
4. **Non-determinism tension** — workflows must be deterministic, LLMs aren't. Architectural separation works but adds boilerplate.

### Verdict on Temporal

Temporal works for AI agents if you accept the constraints. The teams that succeed treat it as an orchestration layer for coarse-grained activities, not a fine-grained execution engine. But: no streaming, heavy infrastructure, and determinism constraints are real friction for our use case.

---

## 2. The Erlang Argument

### "Your Agent Framework Is Just a Bad Clone of Elixir"

George Guimaraes's thesis: *"Every major AI agent framework is independently reinventing the actor model: isolated state, message passing, supervision, fault recovery. You're rebuilding telecom infra in a language that wasn't designed for it."*

Specific callout: Microsoft AutoGen v0.4 rebuilt as an *"event-driven actor framework"* but under the hood it's *"single-threaded Python asyncio with no preemptive scheduling, no per-agent garbage collection, no supervision trees, no 'let it crash' recovery."*

### Jido proves the model works

Jido 2.0 is an Elixir agent framework built directly on OTP GenServer. Runs 10k agents at 25KB each on the BEAM. Multi-agent distributed support via BEAM processes with supervision. It uses *"pure functional agent design inspired by Elm/Redux with cmd/2 as the core operation."*

### But actors have real problems

Jaksa's production analysis: *"Actors are often marketed as 'high availability, no-deadlock solutions,' but the truth is far from this. The Actor Model is actually a very powerful and experts-only model."*

The deadlock issue: despite claims otherwise, Erlang's `receive` waits for specific messages — functioning as a lock. *"In real-world large-scale applications, deadlocks may not be obvious where they lurk."*

HN commenter: *"the urge to use the actor model in non-distributed systems solely to achieve concurrency has been a massive boondoggle"* — structured concurrency may be more appropriate for single-process scenarios.

### The supervision tree truth

Fred Hebert (Zen of Erlang): "Let it crash" is about *"figuring out how components interact, what is critical and what is not, and what state can be saved, kept, recomputed, or lost."* It is NOT about carelessly crashing.

Common mistake: *"burning all your CPU time endlessly restarting dead services"* when the wrong restart strategy is chosen.

Tobias Pfeiffer: *"You can build significant Elixir applications without writing your own GenServers and supervision trees."* Overuse is the real pattern — *"grossly overcomplicate code, deciding deep supervision trees and worker pools are needed when a simple function will do."*

**The critical insight from Flawless:** "Let it crash" loses work. Durable execution complements it by storing minimal data needed to reconstruct state, allowing computation to run until completion.

---

## 3. Capability-Based Security — Real Deployments

### Fuchsia in production

Fuchsia's capability framework is *"deployed across all Nest Hub models, replacing the legacy Cast OS."* Each system component runs with only explicitly granted capabilities. This is the largest consumer deployment of capability-based security.

### The ergonomics problem is real

Fuchsia developers report: *"It's easy to miss routing LogSink to some components, leading to missing diagnostics in the field."* Capability routing is *"non-ergonomic and error prone"* — developers must manually update all CMLs. The team acknowledges *"a possible gap in the framework for a way to route capabilities both securely and ergonomically."*

### cap-std is directly usable

`cap-std` (Bytecode Alliance) works without WASM on Linux/macOS/Windows. Provides `Dir` (filesystem capability) and `Pool` (network capability). **Not a security sandbox for untrusted code** — it's for cooperative capability-oriented programming where you control the code.

---

## 4. Rust Ecosystem — What Actually Exists

### Actor frameworks (ranked by production readiness)

| Crate | Production use | Key differentiator |
|---|---|---|
| **ractor** (~1.5k stars) | Used at Meta for distributed overload protection | Full OTP supervision, 4-priority message levels |
| **kameo** (~1.2k stars) | Newer, actively developed | Multi-message-type (traits, not enums), built-in backpressure (bounded 64), lifecycle hooks (on_start/on_panic/on_stop) |
| **actix** (~8.7k stars) | Widely used (web framework fame) | Fastest messaging, own runtime, but NO supervision trees |

**Performance:** Messaging differences are negligible between ractor/kameo/coerce. Pick on API design and supervision model, not throughput.

**Key difference:** ractor = one enum message type per actor. kameo = multiple message types via traits. For an AI platform where agents handle `UserMessage`, `ToolResult`, `Cancel`, `Query` as different types, kameo is more natural.

### Lifecycle management (not full actor frameworks)

| Crate | What it does | Relevance |
|---|---|---|
| **tokio-graceful-shutdown** (~15k/month downloads) | Nested subsystem trees, automatic SIGINT/SIGTERM, timeout-based shutdown, partial subtree shutdown | **Highly relevant.** Solves 80% of OTP supervision for lifecycle without actor overhead |
| **CancellationToken** (tokio-util) | Hierarchical cancellation via child_token() | Standard primitive, already used |
| **JoinSet** (tokio) | Structured task groups | Already used in konflux |

### Concurrent data structures

| Crate | Best for | Notes |
|---|---|---|
| **papaya** | Read-heavy with predictable latency | Lock-free, async support, orders of magnitude better tail latency than dashmap |
| **scc** | Write-heavy | Lock count scales with entries, not CPU cores |
| **dashmap** | General purpose | Most widely used, but tail latency spikes during resize |

**For process table (mostly reads, occasional writes):** papaya.

### Interesting orchestration crates

| Crate | Model | Relevance |
|---|---|---|
| **mahler** | HTN planning — define target state, planner finds the path | K8s-controller-like. Interesting for "desired state → actual state" reconciliation |
| **orka** | Pipeline with before/on/after hooks per step | Good hook pattern to study |

---

## 5. What This Changes About Our Recommendation

### Refinement 1: Don't build a full actor framework — but DO use supervision patterns

The real-world evidence is clear: supervision trees are valuable but easy to over-apply. The right approach for Konf:

- **Use `tokio-graceful-shutdown`** for structured lifecycle management (subsystem trees, shutdown propagation)
- **Use `CancellationToken` hierarchies** for cancellation propagation
- **Use `JoinSet`** for task groups (already in konflux)
- **DON'T adopt ractor or kameo yet** — they add actor message boilerplate without clear benefit when most communication is "here's a JSON blob over a channel"

If we later need typed actor semantics (e.g., for distributed agents), kameo is the better choice (multi-message types, backpressure defaults).

### Refinement 2: Journaling matters more than we initially said

The Flawless essay's insight is important: "let it crash" alone loses work. Grid Dynamics' experience confirms this — stateless crash recovery causes race conditions and stale state.

For Konf, the synthesis is:
- **smrti is the durable state** (graph + session state survive crashes)
- **The runtime should checkpoint to smrti at meaningful boundaries** — after tool execution completes, after LLM response complete, after user message processed
- **On crash recovery:** rebuild from smrti, not event replay. Load the session state, recent conversation, and relevant memory. The agent picks up naturally.
- **The event journal is for audit/billing**, not replay. Postgres table, simple append.

### Refinement 3: Capability ergonomics need design attention

Fuchsia's production experience shows that manual capability routing is error-prone. For Konf:
- The project config (YAML) should declare capabilities per-trigger (already in the spec)
- The runtime validates at startup that all referenced tools exist and capabilities are satisfiable
- Error messages should be specific: *"workflow 'chat' requires 'memory:store' but trigger only grants 'memory:search'"*
- Consider a `--validate` CLI command that checks config without running

### Refinement 4: Use papaya for the process table

papaya gives us:
- Lock-free concurrent hashmap (no sharded RwLock like dashmap)
- Async support (rare for concurrent maps)
- Predictable tail latency (no resize spikes)
- Better for our read-heavy access pattern (monitoring queries process table frequently)

---

## 6. Updated Implementation Direction

```
konf-runtime/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── runtime.rs       # Runtime struct — wraps engine, owns process table
    ├── process.rs       # WorkflowRun, ProcessTable (papaya), RunStatus
    ├── tree.rs          # Process tree building from parent_id links
    ├── scope.rs         # ExecutionScope with parameterized CapabilityGrant
    ├── context.rs       # Context virtualization (namespace injection)
    ├── lifecycle.rs     # Supervision via tokio-graceful-shutdown subsystems
    ├── journal.rs       # Append-only event journal (Postgres)
    ├── monitor.rs       # RuntimeMetrics, RunSummary, RunDetail
    └── error.rs
```

**Key dependencies:**
- `konflux` (workflow engine)
- `tokio` + `tokio-util` (CancellationToken)
- `tokio-graceful-shutdown` (subsystem lifecycle)
- `papaya` (concurrent process table)
- `sqlx` (journal to Postgres)
- `serde` + `serde_json`
- `tracing`
- `chrono`
- `uuid`

---

## Sources

### Production accounts
- Replit Agent on Temporal (temporal.io/resources/case-studies/replit)
- OpenAI Codex on Temporal (infoq.com)
- Grid Dynamics migration (temporal.io/blog)
- Temporal AI pitfalls (xgrid.co)
- Dust.tt on Temporal (temporal.io/blog)

### Architecture analysis
- "Your Agent Framework Is Just a Bad Clone of Elixir" (georgeguimaraes.com)
- The Zen of Erlang (ferd.ca)
- When Letting It Crash Is Not Enough (flawless.dev)
- What's Wrong with the Actor Model (jaksa.wordpress.com)

### Rust ecosystem
- ractor (github.com/slawlor/ractor)
- kameo (github.com/tqwewe/kameo)
- tokio-graceful-shutdown (github.com/Finomnis/tokio-graceful-shutdown)
- papaya (github.com/ibraheemdev/papaya)
- cap-std (github.com/bytecodealliance/cap-std)
- Actors with Tokio (ryhl.io/blog/actors-with-tokio)
- Restate architecture (github.com/restatedev/restate)

### Capability security
- Fuchsia Security Principles (fuchsia.dev)
- Fuchsia RFC-0171 routing ergonomics (fuchsia.dev)
- WASI Design Principles (github.com/WebAssembly/WASI)
- cap-std (github.com/bytecodealliance/cap-std)

### Observability
- Anthropic's ClickHouse observability (clickhouse.com/blog)
- OpenTelemetry AI Agent Observability (opentelemetry.io/blog)
