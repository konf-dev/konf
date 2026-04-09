# Tools Reference

> Scope: built-in tools and how to enable them.

## Built-in Tools

### echo

| Field | Value |
|-------|-------|
| **Name** | `echo` |
| **Description** | Returns its input unchanged. Useful for testing workflows. |
| **Input** | `message: string` |
| **Enable** | Always available (no `tools.yaml` entry needed) |

```yaml
nodes:
  greet:
    do: echo
    with:
      message: "Hello from Konf!"
```

### Builtin Logic Tools

In addition to `echo`, the following built-in tools are always available without configuration:

- `json_get` — Extracts a value from a JSON object using a JSON pointer path. Input: `data: object`, `path: string`.
- `concat` — Concatenates an array of strings. Input: `strings: string[]`, `separator: string` (optional).
- `log` — Logs a message to the engine tracing output. Input: `message: string`, `level: string` (optional).
- `template` — Renders a MiniJinja template. Input: `template: string`, `context: object` (optional).

### memory:store

| Field | Value |
|-------|-------|
| **Name** | `memory:store` |
| **Description** | Store a piece of knowledge in the memory backend. |
| **Input** | `content: string`, `metadata: object` (optional), `namespace: string` (injected) |
| **Enable** | `tools.memory` section in `tools.yaml` |

### memory:search

| Field | Value |
|-------|-------|
| **Name** | `memory:search` |
| **Description** | Semantic search over stored memories. Returns ranked results. |
| **Input** | `query: string`, `limit: int` (default 10), `min_similarity: float` (default 0.7) |
| **Enable** | `tools.memory` section in `tools.yaml` |

### memory:delete

| Field | Value |
|-------|-------|
| **Name** | `memory:delete` |
| **Description** | Delete a memory entry by ID. |
| **Input** | `id: string` |
| **Enable** | `tools.memory` section in `tools.yaml` |

### memory:traverse

| Field | Value |
|-------|-------|
| **Name** | `memory:traverse` |
| **Description** | Graph traversal over related memory entries. |
| **Input** | `start_id: string`, `depth: int` (default 2), `relation_type: string` (optional) |
| **Enable** | `tools.memory` section in `tools.yaml` |

### ai:complete

| Field | Value |
|-------|-------|
| **Name** | `ai:complete` |
| **Description** | LLM completion with capability-enforced tool-calling (ReAct loop). The kernel owns the loop — tools are resolved dynamically from the live registry, filtered by the caller's capabilities. |
| **Input** | `prompt: string`, `system: string` (optional), `messages: array` (optional, multi-turn history), `tools: string[]` (optional, explicit tool whitelist — AND with capabilities), `model: string` (optional, override), `temperature: float` (optional, override), `max_tokens: int` (optional, override), `max_iterations: int` (optional, override, default 10) |
| **Output** | `{ text: string, _meta: { tool, provider, model, duration_ms, iterations, tool_calls } }` |
| **Enable** | `tools.llm` section in `tools.yaml` |
| **Streaming** | Emits `ToolStart`, `ToolEnd`, `TextDelta`, `Status` events during the ReAct loop |
| **Security** | Inner tools inherit the caller's capabilities. `ai:complete` excluded from inner tools unless explicitly whitelisted. Empty capabilities deny all tools. |

### http:get

| Field | Value |
|-------|-------|
| **Name** | `http:get` |
| **Description** | Make an HTTP GET request. |
| **Input** | `url: string`, `headers: object` (optional) |
| **Enable** | `tools.http` section in `tools.yaml` |

### http:post

| Field | Value |
|-------|-------|
| **Name** | `http:post` |
| **Description** | Make an HTTP POST request. |
| **Input** | `url: string`, `body: object`, `headers: object` (optional) |
| **Enable** | `tools.http` section in `tools.yaml` |

### ai:embed

| Field | Value |
|-------|-------|
| **Name** | `ai:embed` |
| **Description** | Generate an embedding vector for the given text. |
| **Input** | `text: string`, `model: string` (optional, uses default) |
| **Enable** | `tools.embed` section in `tools.yaml` |

### schedule:create

| Field | Value |
|-------|-------|
| **Name** | `schedule:create` |
| **Description** | Schedule a workflow to run after a delay. Can be repeating. |
| **Input** | `workflow: string` (name), `input: object` (workflow args), `delay_seconds: int` (1-604800), `repeat: bool` (default false) |
| **Output** | `{ schedule_id: string }` |
| **Enable** | Always available |
| **Capability** | `schedule:create` or `schedule:*` |

### cancel:schedule

| Field | Value |
|-------|-------|
| **Name** | `cancel:schedule` |
| **Description** | Cancel a previously scheduled workflow. |
| **Input** | `schedule_id: string` |
| **Output** | `{ cancelled: bool }` |
| **Enable** | Always available |
| **Capability** | `cancel:schedule` or `schedule:*` |

## MCP Tools (`mcp:*`)

MCP (Model Context Protocol) servers expose external tools via a standardized protocol. Any tool from an MCP server appears as `mcp:<server>:<tool_name>`.

### Enabling MCP Servers

Add an `mcp_servers` section to `tools.yaml`:

```yaml
mcp_servers:
  github:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_TOKEN: "${GITHUB_TOKEN}"

  filesystem:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/data"]
```

Each server key becomes the namespace prefix. The `github` server's `create_issue` tool is invoked as `mcp:github:create_issue`.

### Using MCP Tools in Workflows

```yaml
nodes:
  list_issues:
    do: mcp:github:list_issues
    with:
      repo: "my-org/my-repo"
      state: "open"
    then: summarize
```

### MCP Server Configuration Fields

| Field | Required | Description |
|-------|----------|-------------|
| `command` | yes | Executable to start the server |
| `args` | no | Command-line arguments |
| `env` | no | Environment variables passed to the server process |

MCP servers run as child processes managed by the Konf runtime. They are started on demand and terminated when the product stops.

## Namespace Injection

All `memory:*` tools receive `namespace` automatically from the runtime scope. The LLM and workflow author never need to specify it — this prevents cross-tenant data access. See [security](../admin-guide/security.md) for details.

## tools.yaml Summary

```yaml
tools:
  memory:                    # enables memory:* tools
    backend: smrti
    config:
      dsn: "postgresql://..."
  llm:                       # enables ai:complete
    provider: openai
    model: "qwen3:8b"
  http:                      # enables http:get, http:post
    enabled: true
  embed:                     # enables ai:embed
    enabled: true

mcp_servers:                 # enables mcp:* tools
  server_name:
    command: "..."
    args: [...]
    env: {}
```
