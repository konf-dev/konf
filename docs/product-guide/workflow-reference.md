# Workflow YAML Schema Reference

**Status:** Authoritative
**Scope:** YAML format for defining workflows executed by the konflux engine

---

## Overview

Workflows are YAML files that define a DAG (directed acyclic graph) of tool calls. The engine parses the YAML, validates it, resolves dependencies, and executes nodes concurrently where possible.

---

## Top-Level Fields

```yaml
workflow: my_workflow          # Required. Unique identifier (becomes workflow ID)
version: "1.0"                # Optional. Default: "0.1.0"
description: "What this does" # Optional. Human-readable description
capabilities:                  # Required for register_as_tool. Capability grants.
  - "memory_search"
  - "ai_complete"
register_as_tool: true         # Optional. Default: false. If true, registers as workflow_{id} tool
input_schema:                  # Optional. JSON Schema for workflow input (used by WorkflowTool)
  type: object
  properties:
    message: { type: string }
  required: [message]
output_schema:                 # Optional. JSON Schema for workflow output
  type: object
  properties:
    response: { type: string }
nodes:                         # Required. Map of node_id → node definition
  step1:
    # ...
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `workflow` | string | Yes | — | Unique workflow identifier |
| `version` | string | No | "0.1.0" | Semantic version |
| `description` | string | No | — | Human-readable description |
| `capabilities` | string[] | No | [] | Required capability grants (see note below) |
| `register_as_tool` | bool | No | false | Register as `workflow_{id}` tool |
| `input_schema` | JSON Schema | No | — | Input validation schema. When `register_as_tool: true`, this becomes the tool's parameter schema (advertised to MCP clients and LLMs). |
| `output_schema` | JSON Schema | No | — | Output schema for downstream tools |
| `nodes` | map | Yes | — | Node definitions (at least one required) |

> **Important:** If `register_as_tool: true`, `capabilities` must be non-empty. A workflow with `capabilities: []` will fail at runtime with capability denied errors. Use `capabilities: ["*"]` to grant access to all tools, or list specific tool patterns (e.g., `["http_get", "memory_*"]`).

---

## Node Definition

Each node invokes a single tool with the parameters defined in `with:`.

```yaml
nodes:
  node_id:
    do: tool_name              # Required. Which tool to invoke
    with:                      # Optional. Input parameters (supports {{templates}})
      query: "{{input.message}}"
      limit: 10
    then: next_node            # Optional. Next node(s) on success
    catch: error_node          # Optional. Node to run on failure
    condition: "{{score > 0.5}}" # Optional. Skip if condition is false
    return: true               # Optional. This node's output is the workflow output
    retry:                     # Optional. Retry policy
      max_attempts: 3
      backoff_ms: 500
    join: all                  # Optional. Wait for all/any dependencies
    timeout_ms: 30000          # Optional. Per-node timeout
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `do` | string | Yes | — | Tool name to invoke (e.g. `echo`, `memory_search`, `ai_complete`) |
| `with` | map | No | {} | Input parameters — supports both static values and `{{template}}` expressions |
| `then` | string or string[] | No | — | Next node(s) on success. Use `then: [a, b]` for parallel fan-out. |
| `catch` | string or array | No | — | Error handling. Simple: `catch: fallback_node`. Branch: `catch: [{when: true, then: node}]` |
| `condition` | string | No | — | Expression that must be truthy to execute |
| `return` | bool | No | false | If true, this node's output becomes the workflow result |
| `retry` | object | No | — | Retry policy (see below) |
| `join` | "all" or "any" | No | "all" | Wait for all or any predecessor nodes |
| `timeout_ms` | integer | No | EngineConfig default | Per-node timeout in milliseconds |

> **Note:** There is no separate `input:` field. Use `with:` for all parameters — it handles both static values (`limit: 10`) and template expressions (`query: "{{input.message}}"`).

---

## Entry Node and DAG Structure

The **first node** in the YAML `nodes:` map is the entry node. All other nodes must be reachable from it via `then:` edges. Unreachable nodes are rejected as orphans at parse time.

> **Note:** The entry node must not use `join:` — it has no predecessors and is always the first node executed.

To run nodes in parallel, use `then: [a, b]` fan-out from the entry (or any node):

```yaml
nodes:
  start:
    do: echo
    with:
      message: "Starting parallel work"
    then: [fetch_a, fetch_b]    # Both run concurrently
  fetch_a:
    do: http_get
    with:
      url: "{{input.url_a}}"
    then: combine
  fetch_b:
    do: http_get
    with:
      url: "{{input.url_b}}"
    then: combine
  combine:
    do: template
    with:
      template: "A: {{fetch_a.body}}, B: {{fetch_b.body}}"
    join: all                   # Wait for both fetch_a and fetch_b
    return: true
```

---

## Template Expressions

Values in `with:` can reference workflow input and other nodes' outputs using `{{expression}}` syntax.

### Reference Rules

| Reference | Meaning | Example |
|-----------|---------|---------|
| `{{input.field}}` | Workflow input field | `{{input.message}}`, `{{input.url}}` |
| `{{node_id.field}}` | Output field from a completed node | `{{search.results}}`, `{{fetch.body}}` |
| `{{node_id}}` | Entire output of a completed node | `{{fetch}}` |
| `{{node_id.a.b}}` | Nested field access | `{{response.data.items}}` |

> **Important:** Workflow input is stored under the key `input`. To reference a workflow argument named `message`, use `{{input.message}}`, not `{{message}}`. Using `{{message}}` would look for a node named `message`.

### Expression Types

```yaml
with:
  # Static value (no template)
  limit: 10

  # Reference to workflow input
  query: "{{input.message}}"

  # Reference to another node's output
  context: "{{search.results}}"

  # String interpolation with multiple references
  combined: "{{fetch_a.text}} + {{fetch_b.text}}"

  # Conditional (used in condition: field)
  # condition: "{{score > 0.5}}"
```

---

## Retry Policy

```yaml
retry:
  max_attempts: 3        # Total attempts (including first try)
  backoff_ms: 500        # Delay between retries (doubles each attempt)
```

Only applies to tools marked as `retryable` in their ToolError. Non-retryable errors fail immediately.

---

## DAG Execution

Nodes form a DAG via `then` edges. The engine:

1. Starts with the entry node (first in YAML order)
2. When a node completes, evaluates its `then` targets
3. If a target's `join` policy is satisfied (all/any predecessors done), executes it
4. Nodes without shared dependencies run in parallel, bounded by `EngineConfig.max_concurrent_nodes`
5. Continues until all reachable nodes complete or an unhandled error occurs
6. The node with `return: true` provides the workflow output

---

## Examples

### Simple Sequential

```yaml
workflow: chat
description: "Search memory then respond"
capabilities: ["memory_search", "ai_complete"]
nodes:
  search:
    do: memory_search
    with:
      query: "{{input.message}}"
    then: respond
  respond:
    do: ai_complete
    with:
      prompt: "{{input.message}}"
      context: "{{search.results}}"
    return: true
```

### Parallel Fan-Out

```yaml
workflow: parallel_search
capabilities: ["http_get", "memory_search", "template"]
nodes:
  start:
    do: echo
    with:
      message: "Searching..."
    then: [web, memory]
  web:
    do: http_get
    with:
      url: "https://api.example.com/search?q={{input.query}}"
    then: combine
  memory:
    do: memory_search
    with:
      query: "{{input.query}}"
    then: combine
  combine:
    do: template
    with:
      template: "Web: {{web.body}}\nMemory: {{memory.results}}"
    join: all
    return: true
```

### Error Handling

```yaml
workflow: safe_fetch
capabilities: ["http_get", "echo"]
nodes:
  fetch:
    do: http_get
    with:
      url: "{{input.url}}"
    then: process
    catch: fallback
    retry:
      max_attempts: 3
      backoff_ms: 1000
  process:
    do: echo
    with:
      data: "{{fetch.body}}"
    return: true
  fallback:
    do: echo
    with:
      error: "Fetch failed, using default"
    return: true
```

### Registered as Tool (Workflow Composition)

```yaml
workflow: summarize
description: "Summarize a document into key points"
register_as_tool: true
capabilities: ["ai_complete"]
input_schema:
  type: object
  properties:
    document: { type: string }
    max_points: { type: integer, default: 5 }
  required: [document]
nodes:
  analyze:
    do: ai_complete
    with:
      prompt: "Extract {{input.max_points}} key points from: {{input.document}}"
    return: true
```

Other workflows can call this as `workflow_summarize`:

```yaml
nodes:
  get_summary:
    do: workflow_summarize
    with:
      document: "{{input.text}}"
      max_points: 3
    return: true
```

> **Capability attenuation:** When a workflow calls another workflow-as-tool, the child runs in a child execution scope. Child capabilities can only be equal to or more restrictive than the parent's — never broader. A parent with `capabilities: ["ai_complete"]` cannot call a child that requires `["ai_complete", "shell_exec"]`. See [engine.md](../architecture/engine.md#capability-validation) for details.

---

## Related Specs

- [engine](../architecture/engine.md) — Engine execution model, streaming, capability validation
- [tools](../architecture/tools.md) — Tool names and schemas
- [overview](../architecture/overview.md) — Workflows as composable tools
