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
    /// Create a new runtime with a konflux engine and optional Postgres pool.
    /// If pool is provided, creates EventJournal and reconciles zombie workflows.
    /// If pool is None (edge/phone deployment), journal is disabled — events exist
    /// only in the in-memory process table.
    pub async fn new(engine: Engine, pool: Option<sqlx::PgPool>) -> Result<Self, RuntimeError>;

    /// Create with custom default resource limits.
    pub async fn with_limits(engine: Engine, pool: Option<sqlx::PgPool>, limits: ResourceLimits) -> Result<Self, RuntimeError>;

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

**Ownership:** konf-runtime owns writes to both tables. The backend reads from them for admin APIs.

### Process table persistence

The ProcessTable is **ephemeral** (in-memory `papaya::HashMap`). On server restart:
- All running workflows are lost (their CancellationTokens are dropped, tokio tasks are aborted)
- Clients must handle reconnection (SSE stream closes, client retries)
- Completed run history survives in `runtime_events` table (persistent)
- Agent context survives in the memory backend (memory graph + session state)

Recovery after restart: the backend does NOT reconstruct the process table from the journal. It starts fresh. Active sessions reconnect and start new workflow runs. This is acceptable because workflows are short-lived (seconds to minutes) and context is in the memory backend (graph + session state).

**Why not checkpointing?** Durable execution (Temporal-style) was explicitly rejected (see `docs/research/2026-04-05-runtime-recommendation.md`). AI agent workflows are non-deterministic — replaying from a checkpoint produces different results because LLM responses aren't reproducible. Instead, Konf follows the Kubernetes model: processes are ephemeral, state is external. Side effects from completed steps (memory writes, API calls) already happened. The client reconnects and starts fresh, reading context from the memory backend.

**For long-running tasks** (hours/days), the pattern is workflow-as-tool composition: an outer workflow chains short-lived sub-workflows, each storing intermediate results in session state. If the server restarts, the outer workflow resumes from the last completed sub-workflow by reading state. No checkpointing machinery needed — composability solves it.

### Zombie workflow reconciliation

On startup, `Runtime::new()` runs a reconciliation query:

```sql
-- Find runs that started but never completed (zombies from previous crash)
INSERT INTO runtime_events (run_id, session_id, namespace, event_type, payload, created_at)
SELECT 
    e.run_id,
    e.session_id,
    e.namespace,
    'workflow_failed',
    jsonb_build_object('error', 'System restart — workflow was interrupted', 'reconciled', true),
    NOW()
FROM runtime_events e
WHERE e.event_type = 'workflow_started'
  AND NOT EXISTS (
    SELECT 1 FROM runtime_events t 
    WHERE t.run_id = e.run_id 
      AND t.event_type IN ('workflow_completed', 'workflow_failed', 'workflow_cancelled')
  );
```

This ensures the admin dashboard and metrics never show eternally "running" workflows from a previous process lifetime.

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

Any workflow with `register_as_tool: true` in its YAML header can be registered as a tool named `workflow:{id}`. This is handled by `WorkflowTool` in konf-runtime:

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

See [konf-engine-spec.md](konf-engine-spec.md) for workflow-as-tool composition details.

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
