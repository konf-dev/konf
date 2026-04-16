# Konf Engine Specification (konflux)

**Status:** Authoritative
**Crate:** `konflux` (konflux-substrate)
**Role:** Kernel — all tool dispatch, workflow execution, and registry management routes through the engine

---

## Overview

The konflux engine is the kernel of the Konf platform. It:

1. Manages three registries: **Tools**, **Resources**, **Prompts**
2. Parses and executes YAML workflows as DAGs of tool calls
3. Validates capabilities before dispatching tools
4. Streams execution events via channels
5. Supports cancellation, timeouts, and concurrency limits

The engine contains zero I/O. It doesn't know about databases, HTTP, or MCP. It only knows about traits and registries.

---

## Three Registries

The engine manages three MCP-aligned primitive types. Each has a registry for registration and lookup.

### Tools (model-controlled)

Tools are actions the LLM decides to call. They are the primary primitive.

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn info(&self) -> ToolInfo;
    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError>;
    async fn invoke_streaming(&self, env: Envelope<Value>, sender: StreamSender) -> Result<Envelope<Value>, ToolError> {
        self.invoke(env).await
    }
    fn projection(&self) -> Option<&dyn StateProjection> { None }
}
```

#### ToolInfo (MCP-parity)

Every tool publishes identical metadata regardless of source (Rust, MCP, Python):

```rust
pub struct ToolInfo {
    /// Unique tool name, e.g. "memory:search", "ai:complete", "workflow:summarize"
    pub name: String,

    /// Human-readable description for the LLM
    pub description: String,

    /// JSON Schema defining expected input parameters
    pub input_schema: Value,

    /// JSON Schema defining expected output shape (optional, aids LLM reasoning)
    pub output_schema: Option<Value>,

    /// Required capability grants to invoke this tool
    pub capabilities: Vec<String>,

    /// Whether this tool supports streaming via invoke_streaming()
    pub supports_streaming: bool,

    /// Behavioral hints (from MCP tool annotations)
    pub annotations: ToolAnnotations,
}

/// Behavioral hints that enable smart engine decisions.
/// Derived from MCP's tool annotation vocabulary.
pub struct ToolAnnotations {
    /// Tool has no side effects (safe to call speculatively)
    pub read_only: bool,

    /// Tool deletes or irreversibly modifies data (warn before calling)
    pub destructive: bool,

    /// Calling with the same input produces the same result (safe to retry)
    pub idempotent: bool,

    /// Tool interacts with external services beyond the Konf platform
    pub open_world: bool,
}

impl Default for ToolAnnotations {
    fn default() -> Self {
        Self {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        }
    }
}
```

#### Envelope

All execution context is carried via the `Envelope<T>` wrapper that holds the tool input alongside typed fields: `namespace`, `actor_id`, `capabilities` (as `CapSet`), `trace_id`, `deadline`, `idempotency_key`, `qos_class`, and extensible metadata. Tools receive `Envelope<Value>` and return `Envelope<Value>`. See `konflux-substrate/src/envelope.rs` for the full definition.

#### ToolError

```rust
pub enum ToolError {
    /// Input validation failed (missing field, wrong type)
    InvalidInput { message: String, field: Option<String> },

    /// Execution failed (network error, API error, etc.)
    ExecutionFailed { message: String, retryable: bool },

    /// Tool invocation exceeded timeout
    Timeout { after_ms: u64 },

    /// Caller lacks required capability
    CapabilityDenied { capability: String },

    /// Tool not found in registry
    NotFound { tool_id: String },
}
```

### Resources (app-controlled)

Resources are read-only context that the application exposes. Agents and MCP clients can browse them.

```rust
#[async_trait]
pub trait Resource: Send + Sync {
    fn info(&self) -> ResourceInfo;
    async fn read(&self) -> Result<Value, ResourceError>;

    /// Optional: subscribe to change notifications (e.g. config file changes).
    /// Returns None if the resource does not support subscriptions.
    fn subscribe(&self) -> Option<tokio::sync::broadcast::Receiver<ResourceChanged>> {
        None
    }
}

pub struct ResourceChanged {
    pub uri: String,
}

pub struct ResourceInfo {
    /// URI identifying this resource, e.g. "konf://config/tools.yaml"
    pub uri: String,

    /// Human-readable name
    pub name: String,

    /// Description of what this resource contains
    pub description: String,

    /// MIME type (e.g. "application/yaml", "application/json")
    pub mime_type: String,
}
```

**What registers as Resources:**
- Product config files (tools.yaml, konf.toml)
- Workflow definitions (each YAML file in workflows/)
- Memory schema (if the backend exposes it)
- Audit journal summary (recent events)

### Prompts (user-controlled)

Prompts are parameterized templates that expand into messages. Users select which prompt to use.

```rust
#[async_trait]
pub trait Prompt: Send + Sync {
    fn info(&self) -> PromptInfo;
    async fn expand(&self, args: Value) -> Result<Vec<Message>, PromptError>;
}

pub struct PromptInfo {
    /// Prompt name, e.g. "code_review", "summarize"
    pub name: String,

    /// Description of what this prompt does
    pub description: String,

    /// Parameters the prompt accepts
    pub arguments: Vec<PromptArgument>,
}

pub struct PromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

pub struct Message {
    pub role: String,       // "user", "assistant", "system"
    pub content: Value,     // String or rich content (text, image, resource)
}
```

**What registers as Prompts:**
- Workflow templates from the prompts/ config directory
- System prompts per product mode

---

## Engine Struct

```rust
pub struct Engine {
    tools: Arc<RwLock<ToolRegistry>>,
    resources: Arc<RwLock<ResourceRegistry>>,
    prompts: Arc<RwLock<PromptRegistry>>,
    config: EngineConfig,
}

impl Engine {
    pub fn new() -> Self;
    pub fn with_config(config: EngineConfig) -> Self;

    // Tool operations
    pub fn register_tool(&self, tool: Arc<dyn Tool>);
    pub fn registry(&self) -> ToolRegistry;    // snapshot

    // Resource operations
    pub fn register_resource(&self, resource: Arc<dyn Resource>);
    pub fn resources(&self) -> ResourceRegistry;

    // Prompt operations
    pub fn register_prompt(&self, prompt: Arc<dyn Prompt>);
    pub fn prompts(&self) -> PromptRegistry;

    // Execution
    pub async fn run(&self, workflow, input, capabilities, metadata, cancel_token, hooks) -> Result<Value>;
    pub async fn run_streaming(&self, workflow, input, capabilities, metadata, cancel_token, hooks) -> Result<StreamReceiver>;

    // Parsing
    pub fn parse_yaml(&self, yaml: &str) -> Result<Workflow>;

    // Config
    pub fn config(&self) -> &EngineConfig;
}
```

---

## Workflow Execution

Workflows are YAML-defined DAGs of tool calls. The engine:

1. Parses YAML into a `Workflow` (validated against max_yaml_size)
2. Validates required capabilities against granted capabilities
3. Resolves the DAG: identifies start nodes (no dependencies), dependency edges, join policies
4. Executes nodes concurrently where dependencies allow (bounded by max_concurrent_nodes)
5. For each node: evaluates conditions, resolves input templates, invokes the tool, handles retries
6. Streams events (Progress, ToolStart, ToolEnd, Done, Error) via channels
7. Respects CancellationToken for graceful abort
8. Enforces global workflow timeout (max_workflow_timeout_ms)

### Streaming

Execution produces a stream of events:

```rust
pub enum StreamEvent {
    Progress {
        node_id: String,
        event_type: ProgressType,
        data: Value,
    },
    Done { output: Value },
    Error { code: String, message: String, retryable: bool },
}

pub enum ProgressType {
    TextDelta,      // LLM content token
    ThoughtDelta,   // LLM reasoning token (thinking models)
    ToolStart,      // Tool invocation began
    ToolEnd,        // Tool invocation completed
    Status,         // Informational status update
}
```

Stream channel uses bounded `mpsc` with backpressure. Progress events are dropped (via `try_send`) if the buffer is full — this is intentional for non-critical updates.

### Workflow-as-Tool

Any workflow with `register_as_tool: true` in its YAML header registers as a tool named `workflow_{id}`. This enables composition:

```yaml
# workflows/summarize.yaml
workflow: summarize
description: "Summarize a document into key points"
register_as_tool: true
input_schema:
  type: object
  properties:
    document: { type: string }
  required: [document]
capabilities: ["ai:complete"]
nodes:
  analyze:
    do: ai:complete
    with:
      prompt: "Summarize this: {{input.document}}"
    return: true
```

This workflow is callable as `workflow:summarize` from other workflows (or `workflow_summarize` from MCP clients). The engine creates a `WorkflowTool` wrapper that:
- Publishes ToolInfo from the workflow's YAML header
- Creates a child execution scope (attenuated capabilities)
- Runs the workflow via the runtime
- Returns the workflow output as the tool result

---

## Capability Validation

Before executing a workflow, the engine validates capabilities:

```rust
capability::validate_grant(&workflow.capabilities, granted_capabilities)?;
```

This ensures the caller has all capabilities the workflow requires. Tool-level capability checks happen at dispatch time via `ExecutionScope::check_tool()`.

At execution time, `VirtualizedTool` wraps each tool with parameter bindings from the capability grant. This injects values like `namespace` into tool input before the tool sees it — the LLM cannot override injected parameters. Child workflows created via `child_scope()` can only attenuate capabilities (make them more specific), never amplify.

See [runtime.md](runtime.md) for the full capability lattice specification and VirtualizedTool implementation details.

### Per-Execution Tool Filtering

The main engine holds ALL registered tools (the global registry). But each workflow execution gets a **scoped copy** containing only the tools its capabilities grant access to.

When `runtime.start()` is called:
1. It iterates the global registry
2. For each tool, calls `scope.check_tool(tool_name)`
3. **Denied tools are NOT registered** in the per-execution engine — the LLM never sees them
4. Granted tools are wrapped with `VirtualizedTool` (if bindings exist) and registered

This means: if a scope has capabilities `["memory:search", "ai:complete"]`, the LLM only sees two tools, even if the global registry has 100+. This prevents LLM context overflow and enforces least-privilege — the LLM cannot even attempt to call a tool it wasn't granted.

**Why this matters:** MCP clients connecting via konf-mcp will also be scoped. An MCP `tools/list` response only includes tools the client's auth token grants. Different users see different tool sets from the same Konf instance.

---

## Engine Configuration

```rust
pub struct EngineConfig {
    /// Maximum steps before aborting (prevents infinite loops)
    pub max_steps: usize,                    // default: 1000

    /// Default timeout per tool invocation in milliseconds
    pub default_timeout_ms: u64,             // default: 30_000

    /// Maximum total workflow execution time in milliseconds (0 = no limit)
    pub max_workflow_timeout_ms: u64,        // default: 300_000

    /// Stream channel buffer size (Progress events dropped if full)
    pub stream_buffer: usize,               // default: 256

    /// Internal channel size for node completion signals
    pub finished_channel_size: usize,        // default: 100

    /// Default retry backoff delay in milliseconds
    pub default_retry_backoff_ms: u64,       // default: 250

    /// Maximum YAML size in bytes (prevents DoS)
    pub max_yaml_size: usize,               // default: 10 MB

    /// Maximum concurrent nodes executing in parallel
    pub max_concurrent_nodes: usize,         // default: 50
}
```

All values have sane defaults. Zero-config works out of the box. Validation fails on zero values (except max_workflow_timeout_ms where 0 means no limit).

---

## Builtin Tools

The engine includes lightweight builtin tools for workflow composition:

| Tool | Purpose | Annotations |
|------|---------|-------------|
| `echo` | Return input as output | read_only, idempotent |
| `json_get` | Extract a field from JSON | read_only, idempotent |
| `concat` | Concatenate strings | read_only, idempotent |
| `log` | Log a message (tracing) | read_only, idempotent |
| `template` | Render a minijinja template | read_only, idempotent |

All builtins are stateless, side-effect-free, and safe to retry.

---

## Related Specs

- [overview](overview.md) — platform-wide architecture
- [tools](tools.md) — tool protocol and plugin crates
- [runtime](runtime.md) — process management, ExecutionScope, WorkflowTool
- [runtime](runtime.md) — ExecutionScope, capability lattice, namespace injection
