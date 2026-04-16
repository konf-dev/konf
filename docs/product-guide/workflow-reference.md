# Workflow YAML Schema Reference

**Status:** Authoritative
**Source of truth:** `crates/konflux-substrate/src/parser/schema.rs`

---

## Overview

Workflows are YAML files that define a DAG (directed acyclic graph) of tool calls. The engine parses the YAML, validates it, resolves dependencies, and executes nodes concurrently where possible.

---

## Top-Level Fields

```yaml
workflow: my_workflow          # Required. Unique identifier (becomes workflow ID)
version: "0.1.0"              # Optional. Default: "0.1.0"
description: "What this does" # Optional. Human-readable description
capabilities:                  # Required for register_as_tool. Capability grants.
  - "memory:search"
  - "ai:complete"
register_as_tool: true         # Optional. Default: false. Registers as workflow:<id> tool.
input_schema:                  # Optional. JSON Schema for workflow input (used by WorkflowTool)
  type: object
  properties:
    message: { type: string }
  required: [message]
output_schema:                 # Optional. JSON Schema for workflow output
  type: object
  properties:
    response: { type: string }
nodes:                         # Required. Ordered map of node_id -> node definition
  step1:
    # ...
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `workflow` | string | Yes | — | Unique workflow identifier |
| `version` | string | No | `"0.1.0"` | Semantic version |
| `description` | string | No | — | Human-readable description |
| `capabilities` | string[] | No | `[]` | Required capability grants |
| `register_as_tool` | bool | No | `false` | Register as `workflow:<id>` tool |
| `input_schema` | JSON Schema | No | — | Input validation schema. When `register_as_tool: true`, this becomes the tool's parameter schema. |
| `output_schema` | JSON Schema | No | — | Output schema for downstream tools |
| `nodes` | ordered map | Yes | — | Node definitions (at least one required). Order matters: first node is entry by default. |

> **Important:** If `register_as_tool: true`, `capabilities` must be non-empty. A workflow with `capabilities: []` will fail at runtime with capability denied errors. Use `capabilities: ["*"]` to grant access to all tools, or list specific tool patterns (e.g., `["http:get", "memory:*"]`).

---

## Node Definition

Every field documented here corresponds to a field on `NodeSchema` in `schema.rs`. Nothing else exists.

```yaml
nodes:
  node_id:
    do: tool_name              # Tool to invoke (single) or parallel array
    with:                      # Optional. Input parameters (supports {{templates}})
      query: "{{input.message}}"
      limit: 10
    pipe:                      # Optional. Chain of transforms on output
      - json_get
      - { do: template, with: { template: "Result: {{vars.value}}" } }
    then: next_node            # Optional. Next node(s) on success
    catch: error_node          # Optional. Error handling
    retry:                     # Optional. Retry policy
      times: 3
      delay: "1s"
      backoff: exponential
      max_delay: "30s"
      on: ["timeout"]
    timeout: "30s"             # Optional. Duration string (e.g. "500ms", "5m")
    entry: true                # Optional. Marks explicit entry node
    repeat:                    # Optional. Loop construct
      until: "{{done}}"
      max: 10
      as: "iteration"
    stream: true               # Optional. Streaming mode
    return: true               # Optional. This node's output is the workflow output
    join:                      # Optional. Wait for parallel branches
      wait_for: [node_a, node_b]
      policy: all
    credentials:               # Optional. Key-value credential map
      api_key: "{{secret.key}}"
    grant:                     # Optional. Capability patterns for this node
      - "http:get"
```

### Field Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `do` | string or array | No | — | Tool to invoke. Single string for one tool, array for parallel tasks. |
| `with` | JSON object | No | — | Input parameters. Supports `{{input.field}}` and `{{node_id.field}}` templates. |
| `pipe` | array | No | `[]` | Chain of transforms applied to output. Each element: simple string (tool name) or `{do: tool, with: {...}}`. |
| `then` | string, string[], or conditional array | No | — | Next node(s). See [Then Block](#then-block). |
| `catch` | string or array | No | — | Error handling. See [Catch Block](#catch-block). |
| `retry` | object | No | — | Retry policy. See [Retry](#retry). |
| `timeout` | string | No | — | Per-node timeout as duration string: `"30s"`, `"500ms"`, `"5m"`. |
| `entry` | bool | No | — | If `true`, marks this node as the explicit entry point. |
| `repeat` | object | No | — | Loop construct. See [Repeat](#repeat). |
| `stream` | bool or string | No | `false` | Streaming mode: `true`, `"passthrough"`, `"pass"`, or `"stream"`. |
| `return` | JSON value | No | — | Marks this node's output as the workflow output. |
| `join` | object | No | — | Wait for parallel branches. See [Join](#join). |
| `credentials` | map | No | `{}` | Key-value credential strings. |
| `grant` | string[] | No | — | Capability patterns granted to this specific node. |

---

## Do Block

The `do` field accepts two forms:

### Single tool

```yaml
do: echo
```

### Parallel tasks

```yaml
do:
  - fetch_web:
      tool: http:get
      with:
        url: "https://example.com/a"
  - fetch_api:
      tool: http:get
      with:
        url: "https://example.com/b"
```

Each parallel task has a name (key), a `tool` field, and an optional `with` field.

---

## Pipe

A chain of transforms applied sequentially to the node's output. Each step is either a tool name string, a map with tool args, or a `{do, with}` block:

```yaml
nodes:
  fetch:
    do: http:get
    with:
      url: "{{input.url}}"
    pipe:
      - json_get                           # Simple: tool name only
      - { do: template, with: { template: "Result: {{vars.data}}" } }  # Full form
    return: true
```

---

## Then Block

Three forms:

### Unconditional (single next node)

```yaml
then: next_node
```

### Multiple next nodes (parallel fan-out)

```yaml
then: [node_a, node_b]
```

### Conditional branching

```yaml
then:
  - when: "{{score > 0.8}}"
    then: high_quality
  - when: "{{score > 0.5}}"
    goto: medium_quality
  - else: true
    then: low_quality
```

Each branch has: `when` (condition string, optional), `then` or `goto` (target node, optional), `else` (bool, marks fallback branch).

---

## Catch Block

Two forms:

### Simple (jump to node on any error)

```yaml
catch: fallback_node
```

### Conditional branches

```yaml
catch:
  - when: true
    then: recovery_node
  - do: skip
    with: { default: "fallback value" }
  - else: true
    then: final_fallback
```

Each catch branch has: `when` (condition, optional), `do` (inline tool to run, optional), `with` (parameters for inline tool, optional), `then` (target node, optional), `else` (bool, marks fallback).

A branch matches if: `when` is absent, or `when` is `true` (bool), or `when` is `"true"` (string), or `else: true`.

---

## Retry

```yaml
retry:
  times: 3                    # Required. Total retry attempts (not including first try)
  delay: "1s"                 # Optional. Delay between retries
  backoff: exponential         # Optional. "exponential", "fixed", or "linear"
  max_delay: "30s"            # Optional. Cap on delay
  on:                          # Optional. Only retry on these error types
    - timeout
    - rate_limit
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `times` | u32 | Yes | Number of retry attempts |
| `delay` | string | No | Delay duration (e.g. `"1s"`, `"500ms"`) |
| `backoff` | string | No | Strategy: `"exponential"`, `"fixed"`, or `"linear"` |
| `max_delay` | string | No | Maximum delay cap (e.g. `"30s"`) |
| `on` | string[] | No | Error types to retry on. If absent, retries on any retryable error. |

---

## Repeat

Loop a node until a condition is met:

```yaml
repeat:
  until: "{{results.done == true}}"   # Required. Condition to stop looping
  max: 10                              # Required. Maximum iterations (safety cap)
  as: "iteration"                      # Optional. Variable name for current iteration
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `until` | string | Yes | Template expression evaluated after each iteration |
| `max` | u32 | Yes | Maximum iterations before forced stop |
| `as` | string | No | Loop variable name accessible in templates |

---

## Join

Wait for multiple parallel branches to complete before executing this node:

```yaml
join:
  wait_for: [fetch_a, fetch_b]    # Required. Node IDs to wait for
  policy: all                       # Optional. "all", "any", or "quorum"
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `wait_for` | string[] | Yes | List of node IDs this node depends on |
| `policy` | string | No | Wait policy: `"all"` (default), `"any"`, or `"quorum"` |

---

## Stream

Controls streaming behavior for a node's output:

```yaml
# Boolean form
stream: true

# String form
stream: passthrough    # also: "pass", "stream"
```

When `true` or set to `"passthrough"` / `"pass"` / `"stream"`, the node's output is streamed to the workflow's stream receiver as it is produced. Default behavior (false or absent): output is buffered.

---

## Template Expressions

Values in `with:` can reference workflow input and other nodes' outputs using `{{expression}}` syntax.

| Reference | Meaning | Example |
|-----------|---------|---------|
| `{{input.field}}` | Workflow input field | `{{input.message}}` |
| `{{node_id.field}}` | Output field from a completed node | `{{search.results}}` |
| `{{node_id}}` | Entire output of a completed node | `{{fetch}}` |
| `{{node_id.a.b}}` | Nested field access | `{{response.data.items}}` |

> **Important:** Workflow input is stored under the key `input`. To reference a workflow argument named `message`, use `{{input.message}}`, not `{{message}}`.

---

## Entry Node

By default, the **first node** in the `nodes:` map is the entry node. To override this, set `entry: true` on any node:

```yaml
nodes:
  setup:
    do: echo
    then: main
  main:
    entry: true
    do: ai:complete
    with:
      prompt: "{{input.message}}"
    return: true
```

---

## Examples

### Simple Sequential

```yaml
workflow: chat
description: "Search memory then respond"
capabilities: ["memory:search", "ai:complete"]
nodes:
  search:
    do: memory:search
    with:
      query: "{{input.message}}"
    then: respond
  respond:
    do: ai:complete
    with:
      prompt: "{{input.message}}"
      context: "{{search.results}}"
    return: true
```

### Parallel Fan-Out with Join

```yaml
workflow: parallel_search
capabilities: ["http:get", "memory:search", "template"]
nodes:
  start:
    do: echo
    with:
      message: "Searching..."
    then: [web, memory]
  web:
    do: http:get
    with:
      url: "https://api.example.com/search?q={{input.query}}"
    then: combine
  memory:
    do: memory:search
    with:
      query: "{{input.query}}"
    then: combine
  combine:
    do: template
    with:
      template: "Web: {{web.body}}\nMemory: {{memory.results}}"
      vars: {}
    join:
      wait_for: [web, memory]
      policy: all
    return: true
```

### Error Handling with Retry

```yaml
workflow: safe_fetch
capabilities: ["http:get", "echo"]
nodes:
  fetch:
    do: http:get
    with:
      url: "{{input.url}}"
    then: process
    catch: fallback
    retry:
      times: 3
      delay: "1s"
      backoff: exponential
      max_delay: "10s"
    timeout: "30s"
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

### Conditional Branching

```yaml
workflow: route_by_score
capabilities: ["ai:complete", "echo"]
nodes:
  analyze:
    do: ai:complete
    with:
      prompt: "Score this: {{input.text}}"
    then:
      - when: "{{analyze.score > 0.8}}"
        then: high
      - when: "{{analyze.score > 0.5}}"
        then: medium
      - else: true
        then: low
  high:
    do: echo
    with: { result: "high quality" }
    return: true
  medium:
    do: echo
    with: { result: "medium quality" }
    return: true
  low:
    do: echo
    with: { result: "low quality" }
    return: true
```

### Parallel Do Block

```yaml
workflow: parallel_fetch
capabilities: ["http:get"]
nodes:
  fetch_both:
    do:
      - page_a:
          tool: http:get
          with:
            url: "https://example.com/a"
      - page_b:
          tool: http:get
          with:
            url: "https://example.com/b"
    return: true
```

### Repeat Loop

```yaml
workflow: poll_until_done
capabilities: ["http:get"]
nodes:
  poll:
    do: http:get
    with:
      url: "{{input.status_url}}"
    repeat:
      until: "{{poll.status == 'complete'}}"
      max: 20
      as: "attempt"
    timeout: "10s"
    return: true
```

### Registered as Tool (Workflow Composition)

```yaml
workflow: summarize
description: "Summarize a document into key points"
register_as_tool: true
capabilities: ["ai:complete"]
input_schema:
  type: object
  properties:
    document: { type: string }
    max_points: { type: integer, default: 5 }
  required: [document]
nodes:
  analyze:
    do: ai:complete
    with:
      prompt: "Extract {{input.max_points}} key points from: {{input.document}}"
    return: true
```

Other workflows can call this as `workflow:summarize`:

```yaml
nodes:
  get_summary:
    do: workflow:summarize
    with:
      document: "{{input.text}}"
      max_points: 3
    return: true
```

### Catch with Conditional Branches

```yaml
workflow: resilient_fetch
capabilities: ["http:get", "echo"]
nodes:
  fetch:
    do: http:get
    with:
      url: "{{input.url}}"
    catch:
      - when: true
        do: echo
        with: { error: "request failed" }
        then: use_cache
      - else: true
        then: abort
    return: true
  use_cache:
    do: echo
    with: { source: "cache" }
    return: true
  abort:
    do: echo
    with: { error: "unrecoverable" }
    return: true
```

### Pipe Transform Chain

```yaml
workflow: fetch_and_extract
capabilities: ["http:get"]
nodes:
  fetch:
    do: http:get
    with:
      url: "{{input.url}}"
    pipe:
      - json_get
      - { do: template, with: { template: "Title: {{vars.title}}", vars: {} } }
    return: true
```

### Credentials and Grant

```yaml
workflow: secure_fetch
capabilities: ["http:get"]
nodes:
  fetch:
    do: http:get
    with:
      url: "{{input.url}}"
    credentials:
      Authorization: "Bearer {{secret.api_token}}"
    grant:
      - "http:get"
    return: true
```

---

## DAG Execution

Nodes form a DAG via `then` edges. The engine:

1. Starts with the entry node (first in YAML order, or the node with `entry: true`)
2. When a node completes, evaluates its `then` targets
3. If a target's `join` policy is satisfied (all/any/quorum predecessors done), executes it
4. Nodes without shared dependencies run in parallel, bounded by `EngineConfig.max_concurrent_nodes` (default: 50)
5. Continues until all reachable nodes complete or an unhandled error occurs
6. The node with `return` provides the workflow output

---

## Related Specs

- [engine](../architecture/engine.md) — Engine execution model, streaming, capability validation
- [tools](../architecture/tools.md) — Tool names and schemas
- [overview](../architecture/overview.md) — Workflows as composable tools
