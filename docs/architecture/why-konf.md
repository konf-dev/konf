# Why Konf

**Status:** Authoritative
**Scope:** Architectural differentiation from library-based agent frameworks

---

## Five Structural Properties

### 1. Structural security via namespace injection

The LLM never sees the user's namespace. The runtime injects it below the workflow layer via `VirtualizedTool` (in `konf-runtime/src/context.rs`). When the LLM calls `memory:search(query="exercise")`, the tool receives `memory:search(query="exercise", namespace="konf:myproduct:user_123")`. The namespace parameter is invisible to the model and cannot be overridden by prompt injection.

**Status:** Proven, tested.

### 2. Capability attenuation via Fuchsia-style lattice

Child scopes can only narrow permissions, never widen them (in `konf-runtime/src/scope.rs`). An agent granted `memory:search` cannot delegate `memory:delete` to a sub-agent. This is enforced by the runtime at scope creation time, not by the LLM's compliance with instructions.

**Status:** Proven, tested.

### 3. Mechanism/policy separation

Rust code provides mechanisms: dispatch, isolation, resource limits, streaming. YAML workflows define policies: logic, triggers, prompts, tool selection. This mirrors Linux: the kernel does not contain cron logic; shell scripts do not manage memory. Policies can be hot-reloaded without restarting the engine.

**Status:** Proven.

### 4. Backend-agnostic storage

The `MemoryBackend` trait (in `konf-tool-memory/src/lib.rs`) abstracts storage behind a standard interface. Agent logic is identical whether backed by Postgres, SQLite, or SurrealDB. Switching backends is a config change, not a code change.

**Status:** Implemented with one backend (Postgres via smrti). The abstraction exists but is not yet proven with a second backend.

### 5. Deterministic control of non-deterministic models

`ResourceLimits` (max_steps, timeout, max_concurrent_nodes) are enforced at the runtime level. The LLM is treated as a lossy co-processor. The system stays stable even when the model hallucinates, loops, or ignores instructions. Runaway workflows are killed by the kernel, not by asking the LLM to stop.

**Status:** Proven, tested.

---

## Why agent-generated workflows are safe on Konf

An agent can write and register new YAML workflows at runtime. This is safe because of four structural guarantees:

1. **The agent writes YAML, not executable code.** Workflows are configuration. They declare which tools to call and in what order. They cannot execute arbitrary code.

2. **The kernel validates every workflow before accepting it.** Malformed YAML, unknown tools, and invalid node references are rejected at parse time by `yaml:validate_workflow`, before any execution occurs.

3. **The capability lattice prevents privilege escalation.** A workflow cannot require capabilities that the writing agent does not already possess. An agent with `memory:search` cannot create a workflow that uses `memory:delete`.

4. **This is structurally impossible to bypass.** These guarantees are enforced by the Rust runtime, not by prompt instructions. No amount of prompt injection can circumvent compile-time type checking or runtime capability validation.

---

## Comparison

| Concern | Library-based frameworks | Konf |
|---------|------------------------|------|
| Security model | Prompt-based ("don't access other users") | Structural (namespace injection, invisible to LLM) |
| Permissions | Binary (API key grants all access) | Attenuated lattice (recursive narrowing) |
| Agent behavior | Application code (Python/JS) | Configuration (YAML) — hot-reloadable |
| Storage | Hardcoded to specific DBs | Backend-agnostic trait |
| Failure containment | Application-level try/catch | Kernel-level ResourceLimits |
| Agent-generated workflows | Dangerous (agent writes executable code) | Safe (agent writes validated YAML within capability bounds) |
| Deployment | Python environment + dependencies | Single Rust binary |
| Multi-tenancy | Application-level isolation | Kernel-level namespace enforcement |

---

## Honesty Table

| Property | Status |
|----------|--------|
| Namespace injection (VirtualizedTool) | Proven, tested |
| Capability attenuation (lattice) | Proven, tested |
| Mechanism/policy separation | Proven |
| Backend-agnostic storage | Abstraction exists, one backend implemented |
| ResourceLimits enforcement | Proven, tested |
| YAML validation at parse time | Proven |
| Hot-reload without restart | Proven |
| Second storage backend (SQLite) | Not yet implemented |
| WASM tool adapter | Not yet implemented |
