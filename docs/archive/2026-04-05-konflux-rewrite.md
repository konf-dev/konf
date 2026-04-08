# Konflux Rewrite — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite konflux as a minimal, high-quality Rust workflow execution engine with PyO3 bindings — cherry-picking proven logic from the existing codebase, removing everything not needed.

**Architecture:** Konflux is a library (not a service) that parses YAML workflows into a DAG, executes nodes in parallel where possible, dispatches tools via a registry, enforces a capability lattice, and streams results. It has no LLM, memory, networking, or HTTP server built in. The consumer (konf-backend) embeds it via PyO3, registers tools, and calls `engine.run()`.

**Tech Stack:** Rust 2024 edition, tokio, serde, serde_yaml, serde_json, indexmap, async-trait, thiserror, minijinja (templates), PyO3 + maturin (Python bindings)

**Source spec:** `docs/specs/2026-04-04-konf-platform-design.md` (Sections 3.2.*)

**Existing code audit:** 213 tests passing, ~8300 LOC core, all modules extract cleanly. Cherry-pick parser, executor, expr, workflow IR, tool traits, streaming types. Remove P2P, WASM, daemon, HTTP server, discovery, gossip, crypto.

---

## Task 0: Archive and Scaffold

**Goal:** Archive current code, create fresh crate structure.

**Files:**
- Archive: `konflux/` (entire directory → `archive/v1` branch)
- Create: `konflux/Cargo.toml`
- Create: `konflux/src/lib.rs`
- Create: `konflux/konflux-python/Cargo.toml`
- Create: `konflux/konflux-python/src/lib.rs`

- [ ] **Step 1: Archive current code**

```bash
cd konflux
git checkout -b archive/v1
git checkout master
```

- [ ] **Step 2: Clean the working directory**

Remove all source files from master, keeping only `.gitignore` and `LICENSE` if they exist.

```bash
rm -rf src/ tests/ showcase/ Cargo.toml Cargo.lock
```

- [ ] **Step 3: Create workspace Cargo.toml**

```toml
[workspace]
members = ["konflux-core", "konflux-python"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "AGPL-3.0"
repository = "https://github.com/konf-dev/konflux"
```

- [ ] **Step 4: Create konflux-core crate**

```bash
mkdir -p konflux-core/src
```

`konflux-core/Cargo.toml`:
```toml
[package]
name = "konflux"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "YAML workflow execution engine with capability lattice"

[dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
serde = { version = "1.0", features = ["derive"] }
serde_yaml = "0.9"
serde_json = "1.0"
indexmap = { version = "2", features = ["serde"] }
async-trait = "0.1"
thiserror = "2"
minijinja = "2"
uuid = { version = "1", features = ["v4"] }
tracing = "0.1"
futures = "0.3"

[dev-dependencies]
tokio = { version = "1", features = ["full", "test-util"] }
tempfile = "3"
```

`konflux-core/src/lib.rs`:
```rust
//! Konflux — YAML Workflow Execution Engine
//!
//! A library for parsing and executing YAML-defined workflows.
//! Konflux dispatches tools, enforces capability lattices,
//! and streams results. It has no opinions about what tools do.

pub mod error;
pub mod workflow;
pub mod expr;
pub mod parser;
pub mod tool;
pub mod executor;
pub mod stream;
pub mod engine;
pub mod capability;
pub mod template;
```

- [ ] **Step 5: Create stub modules**

Create empty files so the crate compiles:

```bash
touch konflux-core/src/{error,workflow,expr,parser,tool,executor,stream,engine,capability,template}.rs
```

Each file gets a minimal module comment:

```rust
//! [Module name] — TODO: implement
```

- [ ] **Step 6: Create konflux-python crate**

```bash
mkdir -p konflux-python/src
```

`konflux-python/Cargo.toml`:
```toml
[package]
name = "konflux-python"
version.workspace = true
edition.workspace = true
license.workspace = true

[lib]
name = "konflux"
crate-type = ["cdylib"]

[dependencies]
konflux = { path = "../konflux-core" }
pyo3 = { version = "0.24", features = ["extension-module"] }
pyo3-async-runtimes = { version = "0.24", features = ["tokio-runtime"] }
serde_json = "1.0"
tokio = { version = "1", features = ["rt-multi-thread"] }
```

`konflux-python/src/lib.rs`:
```rust
use pyo3::prelude::*;

#[pymodule]
fn konflux(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
```

- [ ] **Step 7: Verify it compiles**

```bash
cargo build
cargo test
```

Expected: compiles with no errors, 0 tests.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "chore: scaffold konflux workspace with core and python crates"
```

---

## Task 1: Error Types

**Goal:** Define the crate's error types. Everything else depends on these.

**Files:**
- Create: `konflux-core/src/error.rs`

**Cherry-pick from:** `src/tool/mod.rs` (ToolError enum), `src/parser/error.rs` (ParseError, ValidationError)

- [ ] **Step 1: Write error types**

`konflux-core/src/error.rs`:
```rust
//! Error types for the konflux engine.

use thiserror::Error;

/// Top-level engine errors.
#[derive(Debug, Error)]
pub enum KonfluxError {
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),

    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),

    #[error("execution error: {0}")]
    Execution(#[from] ExecutionError),

    #[error("tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("capability denied: {0}")]
    CapabilityDenied(String),
}

/// Errors during YAML parsing.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid YAML: {message}")]
    InvalidYaml { message: String },

    #[error("missing required field '{field}' in {context}")]
    MissingField { field: String, context: String },

    #[error("invalid value for '{field}': {message}")]
    InvalidValue { field: String, message: String },
}

/// Errors during workflow validation (post-parse).
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("cycle detected: {}", path.join(" → "))]
    CycleDetected { path: Vec<String> },

    #[error("orphaned node '{node}' — not reachable from entry")]
    OrphanedNode { node: String },

    #[error("unknown node '{node}' referenced in edge from '{from}'")]
    UnknownNode { node: String, from: String },

    #[error("unknown tool '{tool}' in node '{node}'")]
    UnknownTool { tool: String, node: String },

    #[error("capability '{capability}' required but not granted")]
    MissingCapability { capability: String },

    #[error("no entry node found")]
    NoEntryNode,

    #[error("no return node found")]
    NoReturnNode,
}

/// Errors during workflow execution.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("node '{node}' failed: {message}")]
    NodeFailed { node: String, message: String },

    #[error("node '{node}' timed out after {timeout_ms}ms")]
    Timeout { node: String, timeout_ms: u64 },

    #[error("max steps exceeded ({max})")]
    MaxStepsExceeded { max: usize },

    #[error("join failed on node '{node}': {message}")]
    JoinFailed { node: String, message: String },
}

/// Errors from tool invocation.
#[derive(Debug, Clone, Error)]
pub enum ToolError {
    #[error("invalid input: {message}")]
    InvalidInput { message: String, field: Option<String> },

    #[error("execution failed: {message}")]
    ExecutionFailed { message: String, retryable: bool },

    #[error("timeout after {after_ms}ms")]
    Timeout { after_ms: u64 },

    #[error("capability denied: {capability}")]
    CapabilityDenied { capability: String },

    #[error("tool not found: {tool_id}")]
    NotFound { tool_id: String },
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build
```

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(error): define error types for parse, validation, execution, and tool errors"
```

---

## Task 2: Workflow IR + Expression Evaluator

**Goal:** Port the workflow intermediate representation types and expression evaluator. These are the foundational data structures everything else operates on.

**Files:**
- Create: `konflux-core/src/workflow.rs` (cherry-pick from old `src/workflow.rs` — 555 LOC)
- Create: `konflux-core/src/expr.rs` (cherry-pick from old `src/expr.rs` — 499 LOC, zero deps)
- Test: `konflux-core/tests/workflow_tests.rs`
- Test: `konflux-core/tests/expr_tests.rs`

**Cherry-pick from:** `src/workflow.rs`, `src/identity.rs`, `src/expr.rs`

**What to change from old code:**
- Remove `pub use crate::parser::schema::StreamMode;` — define `StreamMode` directly in workflow.rs
- Add `StreamMode::Passthrough` variant (was missing)
- Add `capabilities: Vec<String>` to `Workflow` struct
- Add `grant: Vec<String>` to `Step` (for child workflow capability grants)
- Replace old `ValidationError` (local to workflow.rs) with the one from `error.rs`
- Inline `StepId`, `ToolId`, `WorkflowId` as newtypes directly (was in `identity.rs` — 34 LOC, not worth a separate module)

**What to keep as-is:**
- All IR types: `Workflow`, `Step`, `Edge`, `EdgeTarget`, `Expr`, `JoinPolicy`, `ErrorAction`, `RetryPolicy`, `BackoffStrategy`, `PipeStep`, `RepeatConfig`
- Builder pattern API
- DFS cycle detection
- `validate()` method
- Expression evaluator: `ExprEvaluator`, `ExprValue`, all operators, dot-path resolution

- [ ] **Step 1: Write workflow.rs**

Port old `src/workflow.rs` with the following changes:
- Define `StreamMode` locally: `enum StreamMode { Default, Passthrough }`
- Add `capabilities: Vec<String>` to `Workflow`
- Add `grant: Option<Vec<String>>` to `Step`
- Define ID newtypes inline: `StepId(String)`, `ToolId(String)`, `WorkflowId(String)` with `new()`, `as_str()`, `Display`, `PartialEq`, `Eq`, `Hash`, `Clone`
- Use `crate::error::ValidationError` instead of local one
- Keep ALL builder methods, `validate()`, and cycle detection

- [ ] **Step 2: Write expr.rs**

Copy old `src/expr.rs` exactly. It has zero external dependencies (pure Rust, no crate imports). Only change: update any import paths if `StepId` moved.

Verify these are present:
- `ExprEvaluator` with `evaluate()` and `resolve_ref()`
- `ExprValue` enum (String, Int, Float, Bool, Null, List, Map)
- Operators: `==`, `!=`, `>`, `<`, `>=`, `<=`, `&&`, `||`, `!`
- Dot-path resolution: `"step1.output.text"` → nested lookup
- Safe: no arbitrary code execution, no function calls

- [ ] **Step 3: Write workflow tests**

Port old `tests/workflow_tests.rs` (150 LOC). Adapt to new types. Must cover:
- Build a simple linear workflow (A → B → C), validate succeeds
- Build a diamond workflow (A → [B,C] → D), validate succeeds
- Cycle detection: A → B → A, validate returns `CycleDetected`
- Missing entry node, validate returns error
- Missing edge target, validate returns error
- Builder API ergonomics

- [ ] **Step 4: Write expression tests**

Port old `tests/expr_tests.rs` (124 LOC). Must cover:
- Literal evaluation
- Reference resolution with dot paths
- Boolean operators (&&, ||, !)
- Comparison operators (==, !=, >, <, >=, <=)
- String comparison
- Null handling
- Nested map/list access

- [ ] **Step 5: Run tests**

```bash
cargo test
```

Expected: all workflow and expression tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(core): add workflow IR types with capability support and expression evaluator"
```

---

## Task 3: Streaming Types

**Goal:** Define the typed streaming protocol. This is used by tools, executor, and engine.

**Files:**
- Create: `konflux-core/src/stream.rs`
- Test: `konflux-core/tests/stream_tests.rs`

**Cherry-pick from:** `src/tool/mod.rs` (StreamEvent, stream_channel, collect_stream)

**What to change from old code:**
- Replace generic `StreamEvent::Progress { data: Value }` with typed `ProgressType` enum
- Add `node_id` to Progress events
- Add `workflow_id` to Start event
- Keep collection utilities (`collect_stream`, `collect_text_stream`)

- [ ] **Step 1: Write stream.rs**

```rust
//! Streaming protocol for workflow execution.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

/// Events emitted during workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamEvent {
    /// Workflow execution started.
    Start { workflow_id: String },

    /// Progress within a node.
    Progress {
        node_id: String,
        event_type: ProgressType,
        data: Value,
    },

    /// Workflow completed successfully.
    Done { output: Value },

    /// Workflow failed.
    Error {
        code: String,
        message: String,
        retryable: bool,
    },
}

/// Type of progress event — allows frontends to distinguish
/// text streaming from tool execution from status updates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProgressType {
    /// LLM token chunk.
    TextDelta,
    /// Tool invocation starting — data contains tool name + resolved args.
    ToolStart,
    /// Tool invocation complete — data contains result summary + hash.
    ToolEnd,
    /// Status update ("assembling context", "searching memory").
    Status,
}

/// Sender half of a stream channel.
pub type StreamSender = mpsc::Sender<StreamEvent>;

/// Receiver half of a stream channel.
pub type StreamReceiver = mpsc::Receiver<StreamEvent>;

/// Create a new stream channel with the given buffer size.
pub fn stream_channel(buffer: usize) -> (StreamSender, StreamReceiver) {
    mpsc::channel(buffer)
}

/// Collect all events from a stream into the final output value.
/// Returns the `Done` event's output, or an error if the stream
/// ends with an `Error` event or closes without `Done`.
pub async fn collect_stream(mut rx: StreamReceiver) -> Result<Value, String> {
    let mut last_output = None;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Done { output } => {
                last_output = Some(output);
            }
            StreamEvent::Error { message, .. } => {
                return Err(message);
            }
            _ => {}
        }
    }
    last_output.ok_or_else(|| "stream closed without Done event".to_string())
}

/// Collect text deltas from a stream into a single string.
pub async fn collect_text_stream(mut rx: StreamReceiver) -> Result<String, String> {
    let mut text = String::new();
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Progress {
                event_type: ProgressType::TextDelta,
                data,
                ..
            } => {
                if let Some(s) = data.as_str() {
                    text.push_str(s);
                }
            }
            StreamEvent::Error { message, .. } => {
                return Err(message);
            }
            StreamEvent::Done { .. } => break,
            _ => {}
        }
    }
    Ok(text)
}
```

- [ ] **Step 2: Write stream tests**

Test: channel creation, collect_stream (success + error), collect_text_stream, ProgressType serialization round-trip.

- [ ] **Step 3: Run tests**

```bash
cargo test
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(stream): add typed streaming protocol with ProgressType enum"
```

---

## Task 4: Tool Traits and Registry

**Goal:** Define how tools are registered and dispatched. This is the interface between the engine and the outside world.

**Files:**
- Create: `konflux-core/src/tool.rs`
- Test: `konflux-core/tests/tool_tests.rs`

**Cherry-pick from:** `src/tool/mod.rs` (ToolHandle trait, ToolInfo, ToolRegistry)

**What to change from old code:**
- Rename `ToolHandle` → `Tool` (simpler, it's the primary trait)
- Remove `Capability` type alias (just use `String`)
- Remove `RegistryExecutor`, `NetworkExecutor` (not needed — engine owns execution)
- Remove all backend types (Builtin, HTTP, Process, WASM) — tools are registered externally
- Add `ToolContext` with capabilities vec
- Simplify `ToolRegistry` — just `HashMap<String, Arc<dyn Tool>>`
- Keep `ToolInfo` (name, description, input_schema, capabilities, supports_streaming)

- [ ] **Step 1: Write tool.rs**

```rust
//! Tool abstraction — the interface between the engine and the outside world.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ToolError;
use crate::stream::{StreamSender, StreamEvent, ProgressType};

/// A tool is anything that takes input and returns output.
/// Tools are registered by the consumer, not built into the engine.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool metadata.
    fn info(&self) -> ToolInfo;

    /// Execute the tool.
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError>;

    /// Execute with streaming. Default: non-streaming fallback.
    /// Tools should only push Progress events (e.g., TextDelta) to the sender.
    /// The executor handles ToolStart/ToolEnd wrapping and the final Done event.
    async fn invoke_streaming(
        &self,
        input: Value,
        ctx: &ToolContext,
        _sender: StreamSender,
    ) -> Result<Value, ToolError> {
        // Default: no streaming, just invoke and return.
        // Streaming tools override this to push Progress events.
        self.invoke(input, ctx).await
    }
}

/// Tool metadata for registration and capability matching.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub capabilities: Vec<String>,
    pub supports_streaming: bool,
}

/// Context passed to tools during invocation.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub capabilities: Vec<String>,
    pub workflow_id: String,
    pub node_id: String,
    pub metadata: HashMap<String, Value>,
}

/// Registry of available tools, keyed by name.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.info().name.clone();
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn list(&self) -> Vec<ToolInfo> {
        self.tools.values().map(|t| t.info()).collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
```

- [ ] **Step 2: Write tool tests**

Test: register a mock tool, retrieve by name, list tools, missing tool returns None. Test ToolContext construction. Test default `invoke_streaming` fallback sends Done event.

- [ ] **Step 3: Run tests, commit**

```bash
cargo test
git add -A
git commit -m "feat(tool): add Tool trait, ToolRegistry, and ToolContext"
```

---

## Task 5: Template Rendering

**Goal:** Add minijinja-based `{{ }}` template rendering for workflow input resolution.

**Files:**
- Create: `konflux-core/src/template.rs`
- Test: `konflux-core/tests/template_tests.rs`

**Note:** The old code used a custom template implementation. We use minijinja instead — it's a well-maintained Jinja2 engine for Rust, handles nested access, filters, and escaping properly.

- [ ] **Step 1: Write template.rs**

```rust
//! Template rendering using minijinja for {{ }} expressions.

use minijinja::{Environment, Value};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// Render a template string with the given variables.
pub fn render(template: &str, vars: &HashMap<String, JsonValue>) -> Result<String, String> {
    let env = Environment::new();
    let tmpl = env
        .template_from_str(template)
        .map_err(|e| format!("template parse error: {e}"))?;
    let ctx = minijinja::Value::from_serialize(vars);
    tmpl.render(ctx)
        .map_err(|e| format!("template render error: {e}"))
}

/// Check if a string contains template expressions.
pub fn has_templates(s: &str) -> bool {
    s.contains("{{") && s.contains("}}")
}

/// Resolve all Expr values in a HashMap, rendering templates against state.
pub fn resolve_inputs(
    inputs: &HashMap<String, crate::workflow::Expr>,
    state: &HashMap<String, JsonValue>,
) -> Result<HashMap<String, JsonValue>, String> {
    let mut resolved = HashMap::new();
    for (key, expr) in inputs {
        let value = resolve_expr(expr, state)?;
        resolved.insert(key.clone(), value);
    }
    Ok(resolved)
}

/// Resolve a single Expr against state.
pub fn resolve_expr(
    expr: &crate::workflow::Expr,
    state: &HashMap<String, JsonValue>,
) -> Result<JsonValue, String> {
    match expr {
        crate::workflow::Expr::Literal(s) => Ok(JsonValue::String(s.clone())),
        crate::workflow::Expr::Ref(path) => {
            resolve_ref(path, state).ok_or_else(|| format!("unresolved reference: {path}"))
        }
        crate::workflow::Expr::Template(tmpl) => {
            let rendered = render(tmpl, state)?;
            Ok(JsonValue::String(rendered))
        }
        crate::workflow::Expr::Json(val) => resolve_json_templates(val, state),
    }
}

/// Resolve a dot-path reference against state.
/// "step1.output.text" → state["step1"]["output"]["text"]
fn resolve_ref(path: &str, state: &HashMap<String, JsonValue>) -> Option<JsonValue> {
    let parts: Vec<&str> = path.split('.').collect();
    let root = state.get(parts[0])?;
    let mut current = root;
    for part in &parts[1..] {
        current = current.get(part)?;
    }
    Some(current.clone())
}

/// Recursively resolve templates inside JSON values.
fn resolve_json_templates(
    val: &JsonValue,
    state: &HashMap<String, JsonValue>,
) -> Result<JsonValue, String> {
    match val {
        JsonValue::String(s) if has_templates(s) => {
            let rendered = render(s, state)?;
            Ok(JsonValue::String(rendered))
        }
        JsonValue::Array(arr) => {
            let resolved: Result<Vec<_>, _> = arr
                .iter()
                .map(|v| resolve_json_templates(v, state))
                .collect();
            Ok(JsonValue::Array(resolved?))
        }
        JsonValue::Object(obj) => {
            let mut resolved = serde_json::Map::new();
            for (k, v) in obj {
                resolved.insert(k.clone(), resolve_json_templates(v, state)?);
            }
            Ok(JsonValue::Object(resolved))
        }
        other => Ok(other.clone()),
    }
}
```

- [ ] **Step 2: Write template tests**

Test: simple render, nested access, has_templates, resolve_ref with dot paths, resolve JSON with embedded templates, missing variable error.

- [ ] **Step 3: Run tests, commit**

```bash
cargo test
git add -A
git commit -m "feat(template): add minijinja-based template rendering for workflow inputs"
```

---

## Task 6: YAML Parser

**Goal:** Parse YAML workflow definitions into the Workflow IR.

**Files:**
- Create: `konflux-core/src/parser.rs` (or `parser/` module if splitting)
- Test: `konflux-core/tests/parser_tests.rs`

**Cherry-pick from:** `src/parser/` (2868 LOC — schema.rs, compiler.rs, validator.rs, graph.rs, error.rs, mod.rs)

**This is the largest cherry-pick.** The old parser is high quality (1130 LOC of tests, all passing). The approach:

1. Start with `schema.rs` — the YAML deserialization types (what serde_yaml parses into)
2. Then `compiler.rs` — converts schema types → Workflow IR
3. Then `validator.rs` — validates the compiled workflow
4. Then `graph.rs` — builds dependency graph for parallel execution

**What to change:**
- Update schema types to include `capabilities` field on workflow root
- Update schema types to include `grant` field on nodes
- Update `StreamMode` to include `Passthrough` variant
- Update compiler to map new fields into Workflow IR
- Remove any references to old modules (callable, resolver, etc.)
- Update error types to use `crate::error::*`

**What to keep as-is:**
- YAML node structure (do, with, then, catch, return, timeout, repeat, pipe, credentials)
- Parallel node syntax (`do: [{name: tool}, ...]`)
- Conditional edge syntax (`then: [{when: "...", goto: "..."}, ...]`)
- Topological sort in graph.rs
- Cycle detection
- Orphaned node detection
- Template reference extraction for dependency analysis

- [ ] **Step 1: Write parser schema types**

Port `src/parser/schema.rs` (605 LOC). These are the raw serde_yaml deserialization targets. Add:
- `capabilities: Option<Vec<String>>` to `WorkflowSchema`
- `grant: Option<Vec<String>>` to `NodeSchema`
- `StreamMode::Passthrough` variant

- [ ] **Step 2: Write parser compiler**

Port `src/parser/compiler.rs` (742 LOC). This converts schema types → Workflow IR. Update to map new fields.

- [ ] **Step 3: Write parser validator**

Port `src/parser/validator.rs` (696 LOC). Add capability validation: check that all tools referenced in nodes are present in the workflow's capabilities list (if capabilities are declared).

- [ ] **Step 4: Write parser graph**

Port `src/parser/graph.rs` (495 LOC). Dependency graph construction + topological sort. This determines parallel execution order.

- [ ] **Step 5: Write parser mod.rs**

The public API: `Workflow::from_yaml(yaml_str) -> Result<Workflow, ParseError>`

- [ ] **Step 6: Port parser tests**

Port `tests/parser_tests.rs` (1130 LOC). Adapt to new types. Add tests for:
- `capabilities` field parsing
- `grant` field parsing
- `stream: passthrough` parsing
- Capability validation (tool not in capabilities list → error)

**IMPORTANT:** Do not blindly copy tests. Read each test, understand what it validates, verify the assertions are correct for the new types.

- [ ] **Step 7: Run tests, commit**

```bash
cargo test
git add -A
git commit -m "feat(parser): add YAML workflow parser with capability and streaming support"
```

---

## Task 7: Capability Lattice

**Goal:** Implement capability enforcement — the security boundary that prevents privilege escalation.

**Files:**
- Create: `konflux-core/src/capability.rs`
- Test: `konflux-core/tests/capability_tests.rs`

**This is new code** — the old codebase had capability types but no enforcement wiring.

- [ ] **Step 1: Write capability.rs**

```rust
//! Capability lattice — capabilities only attenuate, never amplify.

use crate::error::ToolError;

/// Check if a tool invocation is allowed given the current capabilities.
pub fn check_tool_access(tool_name: &str, capabilities: &[String]) -> Result<(), ToolError> {
    if capabilities.is_empty() {
        // No capabilities declared = unrestricted (backwards compat)
        return Ok(());
    }
    if capabilities.iter().any(|c| matches_capability(c, tool_name)) {
        Ok(())
    } else {
        Err(ToolError::CapabilityDenied {
            capability: tool_name.to_string(),
        })
    }
}

/// Check if a grant list is a subset of the parent's capabilities.
pub fn validate_grant(grant: &[String], parent_capabilities: &[String]) -> Result<(), String> {
    if parent_capabilities.is_empty() {
        // Parent is unrestricted, any grant is valid
        return Ok(());
    }
    for cap in grant {
        if !parent_capabilities.iter().any(|p| matches_capability(p, cap)) {
            return Err(format!(
                "capability '{cap}' cannot be granted — parent does not have it"
            ));
        }
    }
    Ok(())
}

/// Match a capability pattern against a tool name.
/// Supports glob-style wildcards:
///   "memory:*" matches "memory:search", "memory:store"
///   "ai:*" matches "ai:complete", "ai:stream"
///   "*" matches everything
///   "memory:search" matches only "memory:search" (exact)
fn matches_capability(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.ends_with(":*") {
        let prefix = &pattern[..pattern.len() - 1]; // "memory:" from "memory:*"
        return tool_name.starts_with(prefix);
    }
    pattern == tool_name
}
```

- [ ] **Step 2: Write capability tests**

Test:
- Exact match: `["memory:search"]` allows `"memory:search"`
- Glob match: `["memory:*"]` allows `"memory:search"` and `"memory:store"`
- Wildcard: `["*"]` allows everything
- Denial: `["memory:search"]` denies `"ai:complete"`
- Empty capabilities: allows everything (unrestricted)
- Grant validation: `["memory:search"]` ⊆ `["memory:*"]` → ok
- Grant escalation: `["memory:delete"]` ⊄ `["memory:search"]` → error
- Grant from unrestricted parent: always ok

- [ ] **Step 3: Run tests, commit**

```bash
cargo test
git add -A
git commit -m "feat(capability): add capability lattice with glob matching and grant validation"
```

---

## Task 8: Executor

**Goal:** Port the parallel workflow executor — the core execution engine that runs nodes respecting the dependency graph.

**Files:**
- Create: `konflux-core/src/executor.rs` (or `executor/` module)
- Test: `konflux-core/tests/executor_tests.rs`

**Cherry-pick from:** `src/tool/executor/parallel.rs` (1532 LOC), `tests/executor_tests.rs` (875 LOC)

**What to change from old code:**
- Wire capability checks: before dispatching a tool, call `capability::check_tool_access()`
- Use new `StreamEvent` types with `ProgressType` — emit `ToolStart` before invocation, `ToolEnd` after
- Use new `ToolContext` instead of old executor context
- Use `template::resolve_inputs()` for input resolution instead of inline logic
- Remove any references to old modules (RegistryExecutor, NetworkExecutor)

**What to keep as-is:**
- `ParallelState` (thread-safe state with `Arc<RwLock<>>`)
- Graph-based parallel execution (run independent nodes concurrently via tokio::spawn)
- Fan-in with join policies (All, Any, Quorum, Lenient)
- Conditional edge evaluation
- Retry with exponential/linear backoff
- Per-step timeouts via `tokio::time::timeout`
- Bounded loops via RepeatConfig
- Max steps protection (prevent infinite execution)
- Execution tracing

- [ ] **Step 1: Write executor**

Port `src/tool/executor/parallel.rs`. This is the largest single file. Key changes:
- Add `capabilities: Vec<String>` to executor state
- Before each tool dispatch: `capability::check_tool_access(tool_name, &capabilities)?`
- Emit `StreamEvent::Progress { event_type: ToolStart, ... }` before tool invocation
- Emit `StreamEvent::Progress { event_type: ToolEnd, ... }` after tool invocation
- Use `template::resolve_inputs()` for resolving `{{ }}` expressions in node inputs
- Accept `&ToolRegistry` instead of the old executor/resolver abstractions

- [ ] **Step 2: Port executor tests**

Port `tests/executor_tests.rs` (875 LOC). Add tests for:
- Capability denial: node calls tool not in capabilities → error
- ToolStart/ToolEnd events emitted during execution
- Capability grant for child workflows

**IMPORTANT:** Read each test, verify it tests what it claims, verify assertions are correct. Don't blindly copy.

- [ ] **Step 3: Run tests, commit**

```bash
cargo test
git add -A
git commit -m "feat(executor): add parallel workflow executor with capability enforcement and typed streaming"
```

---

## Task 9: Engine (Public API)

**Goal:** Build the public-facing `Engine` struct — the single entry point for consumers.

**Files:**
- Create: `konflux-core/src/engine.rs`
- Test: `konflux-core/tests/engine_tests.rs`

**This is new code.** The old codebase didn't have a unified Engine API.

- [ ] **Step 1: Write engine.rs**

```rust
//! Engine — the public API for running workflows.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::capability;
use crate::error::KonfluxError;
use crate::executor::Executor;
use crate::stream::{stream_channel, StreamReceiver, StreamEvent};
use crate::tool::{Tool, ToolRegistry};
use crate::workflow::Workflow;

/// Configuration for the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Maximum number of steps before aborting (prevents infinite loops).
    pub max_steps: usize,
    /// Default timeout per node in milliseconds (0 = no timeout).
    pub default_timeout_ms: u64,
    /// Stream channel buffer size.
    pub stream_buffer: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_steps: 1000,
            default_timeout_ms: 30_000,
            stream_buffer: 256,
        }
    }
}

/// The workflow execution engine.
pub struct Engine {
    registry: ToolRegistry,
    config: EngineConfig,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            registry: ToolRegistry::new(),
            config: EngineConfig::default(),
        }
    }

    pub fn with_config(config: EngineConfig) -> Self {
        Self {
            registry: ToolRegistry::new(),
            config,
        }
    }

    /// Register a tool.
    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        self.registry.register(tool);
    }

    /// Run a workflow to completion, returning the final output.
    ///
    /// `granted_capabilities` comes from the trigger config (project.yaml).
    /// The engine verifies `workflow.capabilities ⊆ granted_capabilities`
    /// before execution begins.
    ///
    /// `execution_metadata` is injected into every ToolContext.metadata —
    /// carries config_version, session_id, user_id, trace_id, etc.
    pub async fn run(
        &self,
        workflow: &Workflow,
        input: Value,
        granted_capabilities: &[String],
        execution_metadata: HashMap<String, Value>,
    ) -> Result<Value, KonfluxError> {
        // Verify workflow capabilities are a subset of granted capabilities
        capability::validate_grant(&workflow.capabilities, granted_capabilities)
            .map_err(KonfluxError::CapabilityDenied)?;

        let (tx, rx) = stream_channel(self.config.stream_buffer);
        let executor = Executor::new(
            &self.registry,
            granted_capabilities,
            &self.config,
            execution_metadata,
        );
        executor.execute(workflow, input, tx).await?;
        crate::stream::collect_stream(rx)
            .await
            .map_err(|msg| KonfluxError::Execution(
                crate::error::ExecutionError::NodeFailed {
                    node: "stream".into(),
                    message: msg,
                }
            ))
    }

    /// Run a workflow with streaming, returning the stream receiver.
    /// Same capability and metadata semantics as `run()`.
    pub async fn run_streaming(
        &self,
        workflow: &Workflow,
        input: Value,
        granted_capabilities: &[String],
        execution_metadata: HashMap<String, Value>,
    ) -> Result<StreamReceiver, KonfluxError> {
        capability::validate_grant(&workflow.capabilities, granted_capabilities)
            .map_err(KonfluxError::CapabilityDenied)?;

        let (tx, rx) = stream_channel(self.config.stream_buffer);
        let executor = Executor::new(
            &self.registry,
            granted_capabilities,
            &self.config,
            execution_metadata,
        );
        // Spawn execution in background, stream events as they happen
        tokio::spawn(async move {
            if let Err(e) = executor.execute(workflow, input, tx.clone()).await {
                let _ = tx.send(StreamEvent::Error {
                    code: "execution_error".into(),
                    message: e.to_string(),
                    retryable: false,
                }).await;
            }
        });
        Ok(rx)
    }

    /// Parse a YAML string into a Workflow.
    pub fn parse_yaml(&self, yaml: &str) -> Result<Workflow, KonfluxError> {
        crate::parser::parse(yaml).map_err(KonfluxError::from)
    }

    /// Get the tool registry (for inspection).
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }
}
```

- [ ] **Step 2: Update lib.rs re-exports**

```rust
pub use engine::{Engine, EngineConfig};
pub use workflow::Workflow;
pub use tool::{Tool, ToolInfo, ToolContext, ToolRegistry};
pub use stream::{StreamEvent, ProgressType, StreamReceiver};
pub use error::KonfluxError;
```

- [ ] **Step 3: Write engine integration tests**

Test with mock tools:
- Parse YAML → run with granted_capabilities → get result
- Parse YAML → run_streaming → collect events → verify ToolStart/ToolEnd/Done
- Missing tool → error
- Capability denial: workflow declares capability not in granted_capabilities → error
- Capability denial: tool not in workflow capabilities → error at dispatch
- execution_metadata flows through to ToolContext.metadata (verify config_version accessible)
- Timeout → error

- [ ] **Step 4: Run all tests, commit**

```bash
cargo test
git add -A
git commit -m "feat(engine): add Engine public API with run, run_streaming, and parse_yaml"
```

---

## Task 10: Builtin Tools

**Goal:** Minimal builtin tools that ship with the engine.

**Files:**
- Create: `konflux-core/src/builtin.rs`
- Test: `konflux-core/tests/builtin_tests.rs`

**Cherry-pick from:** `src/tool/stdlib/` — only the minimal set

**Builtins:** `echo`, `json_get`, `json_set`, `concat`, `template`, `log`

These are intentionally minimal. No HTTP, no research, no state, no AI. Those are registered by the consumer.

- [ ] **Step 1: Write builtin tools**

Each builtin implements `Tool` trait. ~20-40 lines each.

- `echo` — returns input as-is
- `json_get` — extract value by JSON path
- `json_set` — set value at JSON path
- `concat` — concatenate string items
- `template` — render a minijinja template
- `log` — emit a log event via tracing, return input unchanged

- [ ] **Step 2: Write builtin tests**

Test each builtin with basic input/output.

- [ ] **Step 3: Add `register_builtins()` helper**

```rust
pub fn register_builtins(engine: &mut Engine) {
    engine.register_tool(Arc::new(EchoTool));
    engine.register_tool(Arc::new(JsonGetTool));
    // ...
}
```

- [ ] **Step 4: Run tests, commit**

```bash
cargo test
git add -A
git commit -m "feat(builtin): add minimal builtin tools (echo, json_get, json_set, concat, template, log)"
```

---

## Task 11: End-to-End Integration Tests

**Goal:** Verify the full pipeline: YAML → parse → execute → stream → result.

**Files:**
- Create: `konflux-core/tests/integration_tests.rs`

- [ ] **Step 1: Write integration tests**

Test full workflows using mock tools:
- Linear workflow: A → B → C, verify each step's output feeds into the next
- Parallel workflow: A → [B, C] → D, verify B and C run concurrently
- Conditional routing: classify → branch based on condition
- Error handling: step fails → catch → error_handler
- Retry: step fails 2x, succeeds on 3rd attempt
- Bounded loop: repeat until condition met
- Nested workflow: workflow:execute calls a child workflow with reduced capabilities
- Capability denial: child workflow tries to call a tool not in its grant list → error
- Streaming passthrough: verify TextDelta events propagate through the pipeline
- Large workflow: 50+ nodes, verify max_steps protection

- [ ] **Step 2: Run all tests**

```bash
cargo test
```

Expected: ALL tests pass. Zero failures.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test: add end-to-end integration tests for full workflow pipeline"
```

---

## Task 12: PyO3 Bindings

**Goal:** Python bindings so konf-backend can embed konflux.

**Files:**
- Modify: `konflux-python/Cargo.toml`
- Modify: `konflux-python/src/lib.rs`
- Create: `konflux-python/tests/test_konflux.py`

- [ ] **Step 1: Write Python bindings**

Expose: `Engine`, `Workflow`, `EngineConfig`, `StreamEvent`, `ProgressType`

```rust
use pyo3::prelude::*;
use pyo3::types::PyDict;
use konflux::{Engine as RustEngine, EngineConfig, Workflow};

#[pyclass]
struct Engine {
    inner: RustEngine,
}

#[pymethods]
impl Engine {
    #[new]
    fn new() -> Self {
        Self { inner: RustEngine::new() }
    }

    fn register_tool(&mut self, name: String, tool: PyObject) -> PyResult<()> {
        // Wrap Python callable as a Rust Tool impl
        // ...
    }

    fn parse_yaml(&self, yaml: &str) -> PyResult<Workflow> {
        // Parse and validate once, return cached Workflow object
        // ...
    }

    fn run<'py>(
        &self, py: Python<'py>,
        workflow: &Workflow,
        input: &Bound<'py, PyDict>,
        granted_capabilities: Vec<String>,
        execution_metadata: &Bound<'py, PyDict>,
    ) -> PyResult<PyObject> {
        // Convert input, run with capabilities + metadata, convert output
        // ...
    }

    fn run_streaming(
        &self,
        workflow: &Workflow,
        input: &Bound<'_, PyDict>,
        granted_capabilities: Vec<String>,
        execution_metadata: &Bound<'_, PyDict>,
    ) -> PyResult<PyObject> {
        // Return an async iterator of StreamEvents
        // ...
    }
}

#[pyclass]
struct Workflow {
    inner: konflux::Workflow,
}

```

**Key challenge:** The async boundary. Python tools are `async def` functions. The PyO3 bridge needs `pyo3-async-runtimes` to call them from Tokio. See spec Section 3.2.8 for the architecture.

- [ ] **Step 2: Write Python tests**

```python
import pytest
from konflux import Engine

def test_engine_creates():
    engine = Engine()
    assert engine is not None

def test_register_and_run():
    engine = Engine()
    engine.register_tool("echo", lambda input, ctx: input)
    workflow = engine.parse_yaml("workflow: test\nnodes:\n  step:\n    do: echo\n    return: '{{ step }}'")
    result = engine.run(workflow, {"message": "hello"}, ["echo"], {})
    assert result["message"] == "hello"
```

- [ ] **Step 3: Build and test**

```bash
cd konflux-python
maturin develop
pytest tests/
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(python): add PyO3 bindings for Engine, Workflow, and streaming"
```

---

## Task 13: Documentation and Cleanup

**Goal:** Clean up, document public API, write README.

**Files:**
- Create: `konflux-core/README.md`
- Update: all `pub` items with doc comments

- [ ] **Step 1: Add doc comments to all public types and functions**

Every `pub fn`, `pub struct`, `pub enum`, `pub trait` gets a `///` doc comment.

- [ ] **Step 2: Write README.md**

Brief: what it is, how to use it (Rust and Python examples), link to spec.

- [ ] **Step 3: Run clippy and fix warnings**

```bash
cargo clippy -- -D warnings
```

- [ ] **Step 4: Run full test suite one final time**

```bash
cargo test
cd konflux-python && maturin develop && pytest
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: add documentation and cleanup"
```

---

## Verification

After all tasks complete:

1. **`cargo test`** — all Rust tests pass
2. **`cargo clippy -- -D warnings`** — zero warnings
3. **`maturin develop && pytest`** — Python bindings work
4. **Run a showcase workflow** — parse and execute one of the old showcase YAMLs (adapted) to verify end-to-end
5. **Review dependency count** — `Cargo.toml` should have ~12 dependencies (tokio, serde, serde_yaml, serde_json, indexmap, async-trait, thiserror, minijinja, uuid, tracing, futures, pyo3). No axum, no iroh, no reqwest, no rusqlite, no ring.

---

## Dependency Graph

```
Task 0 (scaffold)
  └→ Task 1 (errors)
      └→ Task 2 (workflow IR + expr)
          ├→ Task 3 (streaming)
          ├→ Task 5 (templates)
          └→ Task 7 (capabilities)
              └→ Task 4 (tool traits)
                  └→ Task 6 (parser)
                      └→ Task 8 (executor)
                          └→ Task 9 (engine)
                              └→ Task 10 (builtins)
                                  └→ Task 11 (integration tests)
                                      └→ Task 12 (PyO3)
                                          └→ Task 13 (docs)
```

