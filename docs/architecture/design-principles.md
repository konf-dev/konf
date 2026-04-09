# Konf Design Principles

**Status:** Living document — updated as we learn
**Audience:** Contributors, product developers, future-us

These are the rules that govern how Konf is built and extended. They're not aspirational — they're derived from building the system and seeing what works.

---

## 1. Rust = mechanisms. Workflows = policies.

The Rust kernel provides mechanisms: "run this tool, enforce this limit, inject this namespace, stream these events."

Workflows define policies: "when user says X, do Y. If Y fails, try Z. Every night, do W."

This is the OS analogy: Linux doesn't put cron logic in the kernel. You don't put memory management in a shell script.

**What must be Rust:**
- Workflow parser and executor (it's what runs workflows)
- Tool dispatch and capability checking (security-critical, every call)
- Namespace injection via VirtualizedTool (security-critical)
- ResourceLimits enforcement (must be below the workflow layer)
- Protocol handling (SSE, HTTP, MCP wire format)
- Tool implementations needing raw performance (DB queries, LLM streaming)
- Process table and metrics (shared mutable state, atomics)

**What must be workflows:**
- Chat conversation loop
- Entity extraction from conversations
- Persona evolution
- Proactive messages and reminders
- Intent routing (LLM decides which action to take)
- System health monitoring
- Workflow generation (self-modification)
- Nightly maintenance (decay, cleanup, reflection)
- User onboarding
- Ops escalation (explaining failures in human terms)

**The test:** If it's configurable per product or might change per user, it's a workflow. If it's a security boundary or performance-critical path that never changes, it's Rust.

---

## 2. Three abstractions — everything composes from these

### ExecutionScope
Who you are, what you can do, how much you can use.

Contains: namespace, capabilities (with bindings), resource limits, actor identity, nesting depth. Set by the product config. The workflow never sees or modifies its own scope — the runtime enforces it.

### Tools
What's available. Atomic operations.

Every tool — Rust function, MCP server, Python function, or registered workflow — implements the same `Tool` trait, publishes the same `ToolInfo`, and is dispatched identically. The agent can't tell where a tool lives.

### Workflows
What to do. Compositions of tool calls.

YAML-defined DAGs. Resolved on demand from a WorkflowStore. A workflow registered as a tool IS a tool — same interface, same capability checking, same health tracking.

**If you can't express something with these three, the abstraction is wrong — don't add a fourth.**

---

## 3. Resource management follows the cgroup model

Workflows don't know their limits. The runtime enforces them invisibly.

Like Linux cgroups: a process doesn't check "do I have enough memory?" before allocating. It just allocates. The kernel says yes or no. If the cgroup is exhausted, the process gets killed or throttled — not by its own code, but by the layer below.

**Implications:**
- ResourceLimits are set by product config (tiers), not by the user or AI
- When a limit is exceeded, the tool returns an error
- The workflow's `catch` block handles it (the persona says "I'm tired")
- No energy counters in workflow code. No budget tracking in tools. The runtime is the single enforcer.
- Cumulative quotas (tokens per day, memory nodes) are tracked per namespace, reset by scheduled job

---

## 4. Workflows are the universal composition unit

A registered workflow IS a tool. The same things apply to both:
- Capability checking before dispatch
- Resource limit enforcement
- Health tracking (duration, success rate, cost)
- Annotations (read_only, destructive, idempotent, estimated_tokens)

**Performance reality:** Workflow call overhead is ~2ms. LLM calls cost 1,000-10,000ms. Tool optimization matters. Workflow optimization is premature until profiling proves otherwise.

**Rule:** Build everything as workflows first. Move to Rust only when profiling proves a bottleneck in a specific workflow.

---

## 5. Lazy resolution over eager loading

Workflows are stored, not loaded at boot. Resolved on demand when triggered.

**Triggers:**
- User sends a message → router picks a workflow
- Schedule fires → scheduler looks up the workflow
- Another workflow calls it → `workflow_name` tool resolves at call time
- External event (webhook, MCP call) → handler resolves workflow

**WorkflowStore backends:**
- Filesystem: product-level workflows from `config/workflows/`
- Database/state: user-generated workflows
- Memory: runtime-created ephemeral workflows

Like a filesystem: you don't load every file into RAM at boot. You read them when needed.

---

## 6. Operational honesty over silent failure

When infrastructure breaks, the AI communicates it in emotional terms, not error codes.

**How it works:**
- A background workflow writes system health to `konf://system/status` (a Resource)
- The persona's chat workflow reads this resource before every response
- If memory is slow: "I'm still thinking about what you said earlier"
- If LLM is overloaded: "This is a tough one, let me really think"
- If database is down: "My memory is foggy right now, but I can still hear you"

**Observability:**
- Every tool call is tracked: duration, success/failure, namespace, cost
- Event journal records all operations (append-only, queryable)
- `konf-top` dashboard: token usage, memory usage, active workflows, recent events
- Runtime metrics: active runs, completed, failed, cancelled, uptime

---

## 7. Self-modification is validation-first

LLMs can generate workflow YAML, but the pipeline is: generate → validate → register. Never generate → execute directly.

**`workflow_validate`** takes YAML and returns structured errors:
```json
{
  "valid": false,
  "errors": [
    {"type": "unknown_tool", "node": "step1", "tool": "nonexistent",
     "suggestion": "Did you mean 'echo'?"}
  ]
}
```

The LLM iterates on errors until validation passes. Only then does `workflow_create` register it.

**Safety guardrails:**
- Generated workflows inherit the user's capabilities (can't escalate)
- Resource quotas cap how many workflows/schedules a user can create
- Persona evolution is slow (nightly reflect), not reactive (per-message rewrite)
- No workflow can run faster than the minimum schedule interval (e.g., 1 hour)
- External tool access (http_get) requires explicit capability grant

---

## 8. The capability lattice only attenuates, never amplifies

A child scope can be equal to or more restricted than its parent. Never more permissive.

This is Fuchsia's capability routing model:
- Product grants user: `["memory_*", "ai_complete", "state_*"]`
- User's workflow creates sub-workflow: can grant at most `["memory_search"]` — a subset
- Sub-workflow cannot gain `http_get` if the parent doesn't have it

Namespace injection is structural, not prompt-based:
- VirtualizedTool injects `namespace` parameter before the tool sees input
- The LLM cannot override injected parameters
- The tool receives the namespace as if it was always there

**The LLM never controls security-relevant parameters.** The runtime does.

---

## 9. Products are configuration, not code

A complete product is a directory of YAML + text files:

```
my-product/
├── tools.yaml          # What tools/backends to use, tier limits
├── prompts/            # How the AI talks, what it extracts
├── workflows/          # What it does (chat, extract, reflect, proactive)
├── schedules.yaml      # When it does background work
└── docker-compose.yml  # How to deploy (konf + postgres + ollama)
```

No Rust. No Python. No JavaScript. The Konf binary runs it.

Changing behavior = editing a YAML file. Adding a feature = adding a workflow. Tuning intelligence = editing a prompt. Deploying = `docker compose up`.

---

## 10. Everything is observable

If it happens in the system, it's in the journal. If it's in the journal, it's queryable.

**What's tracked:**
- Every workflow start/complete/fail with duration
- Every tool invocation with input hash, output hash, duration
- Every memory write with node IDs and namespace
- Every schedule creation/execution
- Every capability check (granted or denied)
- Resource quota usage per namespace

**Who sees what:**
- User: sees their companion's health state (derived from system status)
- Product developer: sees aggregate metrics, extraction quality, failure patterns
- Platform operator: sees everything (admin API, event journal, runtime metrics)

No silent failures. No invisible state. If you can't observe it, you can't trust it.

---

## 11. The kernel does nothing a workflow can do

Principle 1 says Rust = mechanisms, workflows = policies. This principle is stronger: **prove you need Rust before writing it.** The default answer is "no, it's a workflow."

**The test:** "Is this impossible to express as a workflow using existing tools?" If `shell_exec` + `ai_complete` + filesystem can do it, it's a workflow. No exceptions.

**What is NOT Rust:**
- Scheduling (system cron via `shell_exec`, file-based state, LLM-managed)
- Config management (LLM reads/writes YAML files)
- Monitoring dashboards (workflow reads metrics, formats output)
- Error escalation (workflow catches errors, decides response)
- Boot-time setup (a workflow that runs at startup)

**What IS Rust (the only valid reasons):**
- Security boundaries (capability enforcement, namespace injection — cannot be in userspace)
- Performance-critical hot paths (tool dispatch, DAG execution — every call, microseconds matter)
- Shared mutable state primitives (process table, metrics — need atomics/locks)
- Protocol handling (MCP wire format, SSE streaming — must be correct, not "usually correct")

**The timer exception:** A non-blocking "run this workflow after a delay" is the one primitive that cannot be expressed as a workflow (a sleeping workflow blocks a node). This is the `timer_create` syscall equivalent — minimal kernel support for userspace scheduling.

---

## 12. Prompts are runtime-tweakable code

Where traditional systems use schemas, rules engines, or configuration DSLs, prefer prompts + LLM + filesystem.

An LLM maintaining a file structure is a "database" that:
- Needs no schema migrations
- Is tweakable at runtime via prompt overrides
- Handles edge cases through reasoning, not more rules
- Self-documents (the LLM can explain its own decisions)

The trade-off: marginally less precision than rigid schemas. The gain: radical flexibility, composability, and zero compiled code.

**Use rigid schemas only for:** security boundaries, performance-critical paths, and data that must be bit-exact (cryptographic operations, financial ledgers).

**Use prompts for:** state management, decision making, classification, formatting, routing, scheduling logic, error diagnosis. These are all judgment calls — and judgment is what LLMs do.

---

## 13. Thin onion layers, each independently useful

The system is an onion:

```
Layer 0: Kernel     — konflux engine + konf-runtime (DAG execution, capabilities, limits)
Layer 1: Shell      — builtin tools (echo, template, shell_exec, ai_complete, http_get, schedule)
Layer 2: Userspace  — workflows that compose tools into behaviors
Layer 3: Products   — directories of YAML that wire workflows into applications
Layer 4: Users      — runtime prompt overrides, user-generated workflows
```

Each layer is independently forkable, composable, and replaceable:
- A scheduling workflow doesn't know it's inside the devkit product
- The devkit product doesn't know its scheduling uses cron vs file-based vs manual
- Others can borrow any layer, fork it, compose differently

Like Linux distributions: they share a kernel but differ in userspace. Konf products share an engine but differ in workflows and prompts.
