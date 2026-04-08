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
| **Description** | Generate a completion from the configured LLM. |
| **Input** | `prompt: string`, `context: string` (optional), `system: string` (optional), `temperature: float` (optional) |
| **Enable** | `tools.llm` section in `tools.yaml` |

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

### embed:text

| Field | Value |
|-------|-------|
| **Name** | `embed:text` |
| **Description** | Generate an embedding vector for the given text. |
| **Input** | `text: string`, `model: string` (optional, uses default) |
| **Enable** | `tools.embed` section in `tools.yaml` |

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
    input:
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

All memory tools receive `namespace` automatically from the runtime scope. The LLM and workflow author never need to specify it — this prevents cross-tenant data access. See [security](../admin-guide/security.md) for details.

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
  embed:                     # enables embed:text
    enabled: true

mcp_servers:                 # enables mcp:* tools
  server_name:
    command: "..."
    args: [...]
    env: {}
```
