# Konf Runtime Specification

**Status:** Authoritative
**Crate:** `konf-runtime`
**Role:** Process manager — lifecycle, scoping, capabilities, monitoring

---

## What It Is

konf-runtime is a Rust crate that provides OS-like workflow management. It wraps the konflux engine with process lifecycle, capability routing, monitoring, and optional event journaling.

It is a library, not a service. Both konf-backend and konf-mcp embed it via konf-init. It has zero knowledge of memory backends, databases (except optional journal), or specific tools.

---

## Public API

### Runtime

```rust
pub struct Runtime {
    // Internal: engine, process table, journal, default limits
}

impl Runtime {
    /// Create a new runtime with a konflux engine and an optional journal
    /// backend. If a journal is provided, `reconcile_zombies` is invoked
    /// once to surface workflows interrupted by a prior crash. If `None`
    /// (edge/phone deployment), events exist only in the in-memory
    /// process table and are lost on restart.
    pub async fn new(
        engine: Engine,
        journal: Option<Arc<dyn JournalStore>>,
    ) -> Result<Self, RuntimeError>;

    /// Create with custom default resource limits.
    pub async fn with_limits(
        engine: Engine,
        journal: Option<Arc<dyn JournalStore>>,
        limits: ResourceLimits,
    ) -> Result<Self, RuntimeError>;

    /// Install the durable scheduler (`RedbScheduler`). Called once by
    /// `konf-init::boot` after storage and runtime are both constructed;
    /// the scheduler itself holds a `Weak<Runtime>` to break the cycle.
    pub fn install_scheduler(&self, scheduler: Arc<RedbScheduler>);

    /// Access the installed scheduler (`None` if no storage is configured).
    pub fn scheduler(&self) -> Option<&Arc<RedbScheduler>>;

    /// Invoke a single tool under a scope, applying `VirtualizedTool`
    /// namespace injection and `GuardedTool` deny/allow rules, without
    /// creating a workflow-run lifecycle entry. Used by the HTTP MCP
    /// transport so direct tool calls still pick up guards in dev mode.
    pub async fn invoke_tool(
        &self,
        tool_name: &str,
        input: Value,
        scope: &ExecutionScope,
    ) -> Result<Value, RuntimeError>;

    /// Access the real-time event bus (`RunEventBus`) for subscribers
    /// like the TUI's `/v1/monitor/stream` SSE endpoint.
    pub fn event_bus(&self) -> Arc<RunEventBus>;

    // --- Execution ---

    /// Start a workflow. Returns RunId immediately, execution happens in background.
    /// session_id tracks the conversation context (used for journal entries and state scoping).
    pub async fn start(
        &self,
        workflow: &Workflow,
        input: Value,
        scope: ExecutionScope,
        session_id: String,
    ) -> Result<RunId, RuntimeError>;

    /// Wait for a workflow to complete. Returns its output.
    pub async fn wait(&self, run_id: RunId) -> Result<Value, RuntimeError>;

    /// Start + wait (convenience).
    pub async fn run(
        &self,
        workflow: &Workflow,
        input: Value,
        scope: ExecutionScope,
        session_id: String,
    ) -> Result<Value, RuntimeError>;

    /// Start with streaming. Returns RunId + stream receiver for real-time events.
    /// The receiver emits TextDelta, ToolStart, ToolEnd, Done, Error events.
    pub async fn start_streaming(
        &self,
        workflow: &Workflow,
        input: Value,
        scope: ExecutionScope,
        session_id: String,
    ) -> Result<(RunId, StreamReceiver), RuntimeError>;

    // --- Lifecycle ---

    /// Graceful cancel (SIGTERM). Propagates to children.
    pub async fn cancel(&self, run_id: RunId, reason: &str) -> Result<(), RuntimeError>;

    // --- Monitoring ---

    /// List runs, optionally filtered by namespace prefix.
    pub fn list_runs(&self, namespace_prefix: Option<&str>) -> Vec<RunSummary>;

    /// Get detailed info about a specific run.
    pub fn get_run(&self, run_id: RunId) -> Option<RunDetail>;

    /// Get the process tree rooted at a run.
    pub fn get_tree(&self, run_id: RunId) -> Option<ProcessTree>;

    /// Get aggregate metrics.
    pub fn metrics(&self) -> RuntimeMetrics;

    /// Access the event journal (None on edge deployments without DB).
    pub fn journal(&self) -> Option<&EventJournal>;

    // --- Maintenance ---

    /// Remove completed runs older than max_age from the process table.
    pub fn gc(&self, max_age: Duration);

    /// Access the underlying engine (for tool registration).
    pub fn engine(&self) -> &Engine;
}
```

### ExecutionScope

```rust
/// Defines what a workflow execution is allowed to do.
pub struct ExecutionScope {
    /// Hierarchical namespace (e.g., "konf:unspool:user_123").
    pub namespace: String,

    /// Granted capabilities with parameter bindings.
    pub capabilities: Vec<CapabilityGrant>,

    /// Resource limits for this execution.
    pub limits: ResourceLimits,

    /// Identity of the agent or human initiating this execution.
    pub actor: Actor,
}

/// A capability grant with optional parameter bindings.
pub struct CapabilityGrant {
    /// Tool name pattern. Supports glob: "memory:*", "ai:complete", "*".
    pub pattern: String,

    /// Parameters injected into tool input, overriding any LLM-set values.
    /// Key use: {"namespace": "konf:unspool:user_123"} for namespace injection.
    pub bindings: HashMap<String, Value>,
}

/// Resource limits for a workflow execution.
pub struct ResourceLimits {
    pub max_steps: usize,                    // default: 1000
    pub max_workflow_timeout_ms: u64,        // default: 300_000 (5 min)
    pub max_concurrent_nodes: usize,         // default: 50
    pub max_child_depth: usize,              // default: 10
    pub max_active_runs_per_namespace: usize, // default: 20
}

/// Who is executing this workflow.
pub struct Actor {
    pub id: String,        // user_id, agent_id, or "system"
    pub role: ActorRole,
}

/// Serialized as snake_case in JSON/SQL: "infra_admin", "product_admin", etc.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    InfraAdmin,
    ProductAdmin,
    User,
    InfraAgent,
    ProductAgent,
    UserAgent,
    System,
}
```

### Process types

```rust
pub type RunId = uuid::Uuid;

pub enum RunStatus {
    Pending,
    Running,
    Completed { output: Value, duration_ms: u64 },
    Failed { error: String, duration_ms: u64 },
    Cancelled { reason: String, duration_ms: u64 },
}

pub struct RunSummary {
    pub id: RunId,
    pub parent_id: Option<RunId>,
    pub workflow_id: String,
    pub namespace: String,
    pub status: RunStatus,
    pub actor: Actor,
    pub started_at: DateTime<Utc>,
    pub active_node_count: usize,
    pub steps_executed: usize,
}

pub struct RunDetail {
    pub summary: RunSummary,
    pub active_nodes: Vec<ActiveNode>,
    pub capabilities: Vec<String>,  // granted capability patterns
    pub metadata: HashMap<String, Value>,
    pub children: Vec<RunSummary>,
}

pub struct ActiveNode {
    pub node_id: String,
    pub tool_name: String,
    pub started_at: DateTime<Utc>,
    pub status: NodeStatus,
}

pub enum NodeStatus {
    Running,
    Retrying { attempt: u32, max: u32 },
}

pub struct ProcessTree {
    pub run: RunSummary,
    pub children: Vec<ProcessTree>,
    pub active_nodes: Vec<ActiveNode>,
}

pub struct RuntimeMetrics {
    pub active_runs: usize,
    pub total_completed: u64,
    pub total_failed: u64,
    pub total_cancelled: u64,
    pub uptime_seconds: u64,
}
```

### Event journal

The EventJournal is optional. On server deployments with a database, it writes to Postgres. On edge/phone deployments without a database, the journal is disabled — events exist only in the in-memory process table.

When enabled, the runtime writes to TWO Postgres tables:

1. **`runtime_events`** — operational journal. Every workflow start/complete/fail, node execution, tool invocation. For monitoring, debugging, and billing.
2. **`audit_log`** — security audit. Every cross-scope data access (admin reading user data, config changes, GDPR deletions). For compliance.

```rust
pub struct JournalEntry {
    pub run_id: RunId,
    pub session_id: String,
    pub namespace: String,
    pub actor: Actor,
    pub event_type: String,   // "workflow_started", "node_completed", "tool_invoked", etc.
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}
```

**Ownership:** konf-runtime owns writes to all persistent stores. The
backend reads from them for admin APIs. See
[`storage.md`](storage.md) for the full layout of the single redb
file that backs the journal, scheduler, and runner intents.

### Process table persistence

The ProcessTable is **ephemeral** (in-memory `papaya::HashMap`). On
server restart:

- All running workflows are lost (their `CancellationToken`s are
  dropped, tokio tasks are aborted).
- Clients must handle reconnection — the SSE stream closes and the
  client retries.
- The in-memory table starts fresh.

But the runtime **is** durable at the intent layer:

- **Journal** (`runtime_events` in redb) records every lifecycle event
  for audit and for zombie reconciliation on boot. `Runtime::new`
  calls `journal.reconcile_zombies()` which finds runs that started
  but never reached a terminal event and inserts a synthetic
  `workflow_failed { reconciled: true }` so the admin dashboard never
  shows eternally "running" zombies from a previous process lifetime.
- **Scheduler** (`scheduler_timers` in redb) keeps durable timers for
  cron and fixed-delay workflows. On restart the polling loop picks
  up where it left off — overdue timers fire immediately (catch-up).
- **Runner intents** (`runner_intents` in redb) persist the input and
  scope of every `runner:spawn` call. On restart, unterminated
  intents are replayed from the top with the same run id,
  preserving external references (TUI bookmarks, journal entries).

### Why not mid-workflow checkpointing?

Durable mid-workflow execution (Temporal-style, saving
`(step_name, step_output)` pairs) was explicitly rejected. AI agent
workflows are non-deterministic — replaying from a mid-workflow
checkpoint produces different results because LLM responses aren't
reproducible. We do not pretend otherwise.

Konf's durability model is: **persist the intent, retry the whole
workflow from the top, let the author make it idempotent.** The
idiomatic tools are memory-backed cursors and dedup keys. See
[`durability.md`](durability.md) for the doctrine and worked examples.

### Long-running tasks

For hours-or-days tasks, the pattern is **workflow-as-tool
composition**: an outer orchestrator workflow chains short-lived
sub-workflows, each storing intermediate results in memory. On
restart, the orchestrator replays from the top and reads memory to
see which sub-workflows already produced their output. No
checkpointing machinery required.

### Errors

```rust
pub enum RuntimeError {
    NotFound(RunId),
    NotRunning(RunId),
    ResourceLimit { limit: String, value: usize },
    CapabilityDenied(String),
    Engine(KonfluxError),
    JoinFailed(String),
    Database(sqlx::Error),
}
```

---

## Behaviors

### Capability matching

`CapabilityGrant::matches(tool_name)` uses the same logic as konflux's `capability.rs`:
- `"*"` matches everything
- `"memory:*"` matches `"memory:search"`, `"memory:store"` (requires colon separator)
- `"memory:search"` matches exactly

When a match is found, the grant's `bindings` are returned and injected into the tool input by `VirtualizedTool`.

### Namespace injection (context virtualization)

When the runtime starts a workflow, it wraps every tool in the engine's registry with a `VirtualizedTool` that:
1. Checks if the tool name matches a granted capability
2. If it does, injects the grant's bindings into the tool input
3. Bindings override any existing keys (prevents LLM from setting namespace)

The LLM calls `memory:search(query="exercise routine")`. The runtime intercepts and calls `memory:search(query="exercise routine", namespace="konf:unspool:user_123")`.

### Process lifecycle

1. `runtime.start()` → validates scope, creates WorkflowRun in ProcessTable, spawns tokio task
2. Task runs konflux engine with CancellationToken and RuntimeHooks
3. RuntimeHooks update ProcessTable active_nodes in real-time
4. On completion/failure/cancellation → update ProcessTable status, write journal entry
5. `runtime.wait()` → awaits the tokio JoinHandle

### Cancellation propagation

`runtime.cancel(run_id)` cancels the CancellationToken for that run AND recursively cancels all child runs (found via `parent_id` in ProcessTable).

### Garbage collection

`runtime.gc(max_age)` removes entries from ProcessTable where status is terminal (Completed/Failed/Cancelled) and `completed_at` is older than `max_age`. Running/Pending entries are never removed.

---

## Workflow-as-Tool

Any workflow with `register_as_tool: true` in its YAML header can be registered as a tool named `workflow_{id}`. This is handled by `WorkflowTool` in konf-runtime:

```rust
pub struct WorkflowTool {
    workflow: Workflow,
    runtime: Arc<Runtime>,
    default_scope: ExecutionScope,
}

impl Tool for WorkflowTool {
    fn info(&self) -> ToolInfo {
        // name, description, input_schema from workflow YAML header
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        // Create child scope (attenuated from parent)
        // Run workflow via self.runtime.run()
        // Return workflow output
    }
}
```

konf-init creates `WorkflowTool` instances for each eligible workflow and registers them in the engine.

See [engine.md](engine.md) for workflow-as-tool composition details.

---

> **Note:** Python bindings are not yet implemented in this monorepo. The API below is aspirational.

## Python API (opt-in via PyO3)

```python
from konf_runtime import Runtime, ExecutionScope, CapabilityGrant, ResourceLimits, Actor

# Create
runtime = await Runtime.connect("postgresql://localhost/konf")

# Register tools
runtime.register_tool("echo", echo_func, {"description": "Echo input"})

# Parse workflow
workflow = runtime.parse_yaml(yaml_str)

# Run
scope = ExecutionScope(
    namespace="konf:unspool:user_123",
    capabilities=[
        CapabilityGrant(pattern="memory:*", bindings={"namespace": "konf:unspool:user_123"}),
        CapabilityGrant(pattern="ai:complete"),
    ],
    limits=ResourceLimits(max_steps=500),
    actor=Actor(id="user_123", role="user"),
)

result = await runtime.run(workflow, {"message": "hello"}, scope)

# Or streaming
run_id, stream = await runtime.start_streaming(workflow, input, scope)
async for event in stream:
    print(event)

# Monitor
runs = runtime.list_runs(namespace_prefix="konf:unspool")
tree = runtime.get_tree(run_id)
metrics = runtime.metrics()

# Control
await runtime.cancel(run_id, "user requested")
```

---

## Tool Guards

Tool guards are configurable deny/allow rules evaluated before tool invocation. They follow the same decorator pattern as `VirtualizedTool` — wrapping tools transparently at registry construction time.

### Wrapping Order

```text
GuardedTool(              ← rules checked on raw LLM input
  VirtualizedTool(        ← namespace/bindings injected
    inner_tool            ← actual execution
  )
)
```

Rules evaluate **before** namespace injection. This means guards operate on what the LLM actually sent, not the post-injection input.

### Configuration

Guards are defined in `tools.yaml` under `tool_guards:`:

```yaml
tool_guards:
  shell:exec:
    rules:
      - action: deny
        predicate:
          type: contains
          path: "command"
          value: "sudo"
        message: "sudo is not allowed"
      - action: deny
        predicate:
          type: matches
          path: "command"
          pattern: "rm -rf*"
        message: "destructive rm blocked"
    default: allow

  # Aliasing: redirect calls to a wrapper workflow
  dangerous:tool:
    alias: workflow:safe_dangerous_tool
```

### Rule Evaluation

Rules are evaluated in order. First match wins:
- **deny** → returns `ToolError::CapabilityDenied` with the message
- **allow** → delegates to the inner tool immediately (skips remaining rules)
- **no match** → `default` action applies (defaults to `deny` — fail-closed)

### Predicate Types

| Type | Fields | True when |
|------|--------|-----------|
| `contains` | `path`, `value` | String at `path` contains `value` as substring |
| `matches` | `path`, `pattern` | String at `path` matches glob pattern (`*`, `?`) |
| `equals` | `path`, `value` | Value at `path` equals `value` exactly |
| `exists` | `path` | Field at `path` exists and is not null |
| `not` | `predicate` | Inner predicate is false |
| `all` | `predicates` | All inner predicates are true |
| `any` | `predicates` | Any inner predicate is true |

Paths are dot-separated (e.g., `config.level`) and support array indexing (e.g., `items.0.name`).

### Tool Aliasing

When `alias` is set, the runtime registers the alias workflow under the original tool's name. The agent calls `shell:exec` but actually gets `workflow:safe_shell`. Combined with capability attenuation, the original tool is only accessible inside the wrapper workflow's scope.

### No Hidden Behaviors

Guards are applied at registry construction time — the tool in the registry IS the guarded version. `system:introspect` shows the tools as they appear. The executor is unchanged; it dispatches tools identically regardless of wrapping.

---

## Auth Scoping

The runtime provides `scope_from_role()` to build an `ExecutionScope` from a role name and product context. This is the shared auth resolution path used by both HTTP (axum middleware) and MCP session setup.

```rust
let scope = scope_from_role(
    "alice",                          // actor ID
    "agent",                          // role name
    "konf:myproduct",                 // product namespace
    &["memory:*", "ai:complete"],     // capabilities for this role
    Some("agents"),                   // namespace suffix
    ResourceLimits::default(),
);
```

Role definitions live in `tools.yaml`:

```yaml
roles:
  admin:
    capabilities: ["*"]
  agent:
    capabilities: ["memory:*", "ai:complete", "workflow:*"]
    namespace_suffix: "agents"
  guest:
    capabilities: ["echo", "template"]
    namespace_suffix: "guest"
```

MCP sessions use `KonfMcpServer::with_capabilities()` for scoped access. The default (`KonfMcpServer::new()`) gives full access for local development.
