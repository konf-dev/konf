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
capabilities:                  # Optional. Required capability grants for execution
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
| `capabilities` | string[] | No | [] | Required capability grants |
| `register_as_tool` | bool | No | false | Register as `workflow_{id}` tool |
| `input_schema` | JSON Schema | No | — | Input validation schema |
| `output_schema` | JSON Schema | No | — | Output schema for downstream tools |
| `nodes` | map | Yes | — | Node definitions (at least one required) |

---

## Node Definition

```yaml
nodes:
  node_id:
    do: tool_name              # Required. Which tool to invoke
    with:                      # Optional. Static input parameters
      key: value
    input:                     # Optional. Dynamic input with template expressions
      query: "{{message}}"
      context: "{{search.results}}"
    then: next_node            # Optional. Next node(s) on success
    catch: error_node          # Optional. Node to run on failure
    condition: "{{score > 0.5}}" # Optional. Skip if condition is false
    return: true               # Optional. This node's output is the workflow output
    retry:                     # Optional. Retry policy
      max_attempts: 3
      backoff_ms: 500
    join: all                  # Optional. Wait for all/any/none dependencies
    timeout_ms: 30000          # Optional. Per-node timeout
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `do` | string | Yes | — | Tool name to invoke (e.g. `echo`, `memory_search`, `ai_complete`) |
| `with` | map | No | {} | Static input parameters (not templated) |
| `input` | map | No | {} | Dynamic input with `{{expression}}` templates |
| `then` | string or string[] | No | — | Next node(s) on success |
| `catch` | string | No | — | Error handler node |
| `condition` | string | No | — | Expression that must be truthy to execute |
| `return` | bool | No | false | If true, this node's output becomes the workflow result |
| `retry` | object | No | — | Retry policy (see below) |
| `join` | "all" or "any" | No | "all" | Wait for all or any predecessor nodes |
| `timeout_ms` | integer | No | EngineConfig default | Per-node timeout in milliseconds |

---

## Template Expressions

Input values can reference other nodes' outputs and workflow input using `{{expression}}` syntax:

```yaml
input:
  query: "{{message}}"              # From workflow input
  context: "{{search.results}}"     # From node named "search", field "results"
  combined: "{{a.text}} + {{b.text}}" # Multiple references
```

Expressions support:
- **Dot paths:** `{{node_id.field.nested}}` — access nested JSON fields
- **Workflow input:** `{{field_name}}` — top-level workflow input fields
- **Conditionals:** `{{score > 0.5}}` — boolean expressions (in `condition` field)
- **String interpolation:** `"Hello {{name}}"` — embedded in strings

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

1. Identifies start nodes (no incoming `then` edges from other nodes)
2. Executes start nodes concurrently
3. When a node completes, evaluates its `then` targets
4. If a target's `join` policy is satisfied (all/any predecessors done), executes it
5. Continues until all reachable nodes complete or an unhandled error occurs
6. The node with `return: true` provides the workflow output

**Concurrency:** Nodes without dependencies run in parallel, bounded by `EngineConfig.max_concurrent_nodes`.

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
    input:
      query: "{{message}}"
    then: respond
  respond:
    do: ai_complete
    input:
      prompt: "{{message}}"
      context: "{{search.results}}"
    return: true
```

### Parallel Fan-Out

```yaml
workflow: parallel_search
nodes:
  web:
    do: http_get
    input:
      url: "https://api.example.com/search?q={{query}}"
  memory:
    do: memory_search
    input:
      query: "{{query}}"
  combine:
    do: template
    with:
      template: "Web: {{web.body}}\nMemory: {{memory.results}}"
      vars: {}
    join: all
    return: true
```

### Error Handling

```yaml
workflow: safe_fetch
nodes:
  fetch:
    do: http_get
    input:
      url: "{{url}}"
    then: process
    catch: fallback
    retry:
      max_attempts: 3
      backoff_ms: 1000
  process:
    do: echo
    input:
      data: "{{fetch.body}}"
    return: true
  fallback:
    do: echo
    with:
      error: "Fetch failed, using default"
    return: true
```

### Registered as Tool

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
    input:
      prompt: "Extract {{max_points}} key points from: {{document}}"
    return: true
```

Other workflows can call this as `workflow_summarize`.

---

## Related Specs

- [engine](../architecture/engine.md) — Engine execution model, streaming, capability validation
- [tools](../architecture/tools.md) — Tool names and schemas
- [overview](../architecture/overview.md) — Workflows as composable tools
