# Product Configuration Reference

> Scope: all config files inside a product directory.

## Directory Layout

```
products/<name>/
├── config/
│   ├── project.yaml          # Product identity and triggers
│   ├── tools.yaml            # Available tools and MCP servers
│   ├── models.yaml           # LLM provider settings
│   └── workflows/            # Workflow definitions (one per file)
│       └── chat.yaml
├── prompts/                  # Markdown prompt templates
│   └── system.md
└── README.md
```

## project.yaml

Defines the product identity and entry points.

```yaml
name: assistant
description: "Personal assistant with memory and tool use"
version: "0.1.0"

triggers:
  chat:
    workflow: chat
    capabilities:
      - "memory:*"
      - "ai:complete"
      - "http:get"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Unique product identifier |
| `description` | no | Human-readable description |
| `version` | no | Semver string |
| `triggers` | yes | Map of entry point name to workflow + capabilities |

Each trigger grants **at most** the listed capabilities to the workflow it invokes. Capabilities attenuate down — a workflow cannot exceed its trigger's grants.

## tools.yaml

Declares which tools the product can use.

```yaml
tools:
  memory:
    backend: smrti
    config:
      dsn: "${DATABASE_URL:-postgresql://postgres:konf@localhost/konf}"
  llm:
    provider: openai
    model: "${KONF_MODEL:-qwen3:8b}"
  http:
    enabled: true
  embed:
    enabled: true

mcp_servers:
  github:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_TOKEN: "${GITHUB_TOKEN}"
```

- `tools.*` — built-in tool configuration. Omit a key to disable that tool family.
- `mcp_servers.*` — external MCP server definitions. See [tools-reference.md](tools-reference.md).
- **Environment variables:** `tools.yaml` supports `${VAR}` and `${VAR:-default}` interpolation. Missing variables without a default resolve to an empty string. `konf.toml` also supports env var overrides via the `KONF_` prefix.
- `tool_guards.*` — deny/allow rules per tool. See below.
- `roles.*` — role → capability mapping for auth scoping. See below.

### Roles

Roles define capability grants for different actor types. The runtime uses these to create scoped contexts for authenticated users, ensuring least-privilege access and strict namespace isolation.

```yaml
roles:
  admin:
    capabilities: ["*"] # Full access to all tools and operations
  agent:
    capabilities:
      - "memory:*"       # All memory operations (search, store, delete)
      - "state:*"        # All session state operations
      - "ai:complete"    # LLM completion
      - "http:get"       # Read-only HTTP requests
      - "schedule:create" # Create scheduled workflows
      - "cancel:schedule" # Cancel scheduled workflows
      - "secret:get"      # Access allowed secrets
      - "secret:list"     # List allowed secrets
    namespace_suffix: "agents" # Appended to the product namespace
  guest:
    capabilities:
      - "memory:search"  # Read-only memory access
      - "ai:complete"    # LLM completion
    namespace_suffix: "guest"
```

**Key concepts:**
- **`roles`**: Top-level key under which role definitions reside.
- **Role name**: The identifier for the role (e.g., `admin`, `agent`, `guest`).
- **`capabilities`**: A list of capability patterns that define what actions a role is permitted to perform. Patterns support wildcards (e.g., `memory:*`, `*`). Capabilities granted to a role are the **maximum** permissions that can be given to any workflow or tool run under that role's context.
- **`namespace_suffix`**: An optional string appended to the base product namespace. This allows you to isolate data (e.g., `konf:assistant:agents` vs `konf:assistant:guest`) without changing the tool implementation.

See [runtime.md](../architecture/runtime.md#auth-scoping) for how roles are used in the capability lattice and [multi-tenancy.md](../architecture/multi-tenancy.md) for more on namespace isolation.

### Tool Guards

Tool guards define deny/allow rules that are evaluated before every tool invocation. They provide a powerful way to enforce security policies and prevent agents from performing dangerous or unintended actions.

```yaml
tool_guards:
  # Guards for a specific tool, e.g., shell:exec
  shell:exec:
    rules:
      # Deny execution if the command contains "sudo"
      - action: deny
        predicate:
          type: contains
          path: "command"
          value: "sudo"
        message: "Tool Guard: 'sudo' is not allowed for shell:exec."
      # Deny execution if the command matches a pattern (e.g., rm -rf)
      - action: deny
        predicate:
          type: matches
          path: "command"
          value: ".*rm -rf.*"
        message: "Tool Guard: 'rm -rf' is not allowed for shell:exec."
    default: allow  # Explicitly allow other commands; default is 'deny' (fail-closed)

  # Example: Guard for memory:store
  memory:store:
    rules:
      # Deny storing nodes with a specific type
      - action: deny
        predicate:
          type: equals
          path: "node_type"
          value: "sensitive_info"
        message: "Tool Guard: Cannot store sensitive_info nodes in memory."
    default: allow

  # Example: Global guard (applies to all tools)
  "*":
    rules:
      - action: deny
        predicate: { type: exists, path: "_debug_force_fail" }
        message: "Tool Guard: Debug force fail enabled."
    default: allow
```

**Key concepts:**
- **`tool_guards`**: The top-level key under which all guard definitions reside.
- **Tool name**: Each key under `tool_guards` specifies the tool to guard (e.g., `shell:exec`, `memory:store`). Use `"*"` for a global guard that applies to all tools.
- **`rules`**: A list of rule objects. Each rule defines an `action` and a `predicate`.
- **`action`**: `deny` or `allow`. If a rule matches, this action is taken.
- **`predicate`**: The condition to check against the tool's input. Predicate types:
  - `contains`: Checks if a string value contains a substring.
  - `matches`: Checks if a string value matches a regex pattern.
  - `equals`: Checks if a value equals another value.
  - `exists`: Checks if a field exists in the input.
  - `not`: Inverts another predicate.
  - `all`: All sub-predicates must be true.
  - `any`: Any sub-predicate must be true.
- **`path`**: A JSON pointer path to the field in the tool's input to check (e.g., `command`, `key`, `node_type`).
- **`value`**: The value to compare against for `contains`, `matches`, `equals` predicates.
- **`message`**: A custom error message returned if the guard denies the action.
- **`default`**: `allow` or `deny`. If no rules match, this action is taken. The default is `deny` (fail-closed).

Tool guards are hot-reloadable via `config:reload`. See [runtime.md](../architecture/runtime.md#tool-guards) for the full reference and implementation details.
## models.yaml

LLM provider and generation settings.

```yaml
default:
  provider: openai
  model: "qwen3:8b"
  temperature: 0.7
  max_tokens: 4096
```

| Field | Default | Description |
|-------|---------|-------------|
| `provider` | `openai` | LLM provider (openai-compatible API) |
| `model` | — | Model name or path |
| `temperature` | `0.7` | Sampling temperature |
| `max_tokens` | `4096` | Maximum tokens in the response |

### Provider Switching Guide

To use a local Ollama instance (OpenAI-compatible):
```yaml
default:
  provider: openai
  model: "qwen3:8b"
```
```bash
export OPENAI_API_KEY=ollama
export OPENAI_BASE_URL=http://localhost:11434/v1
```

To use Anthropic:
```yaml
default:
  provider: anthropic
  model: claude-3-5-sonnet-latest
```
```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

## prompts/*.md

Markdown files containing prompt templates. Referenced by workflows:

```markdown
You are a helpful personal assistant. You have access to a persistent
memory system that stores knowledge about your conversations.

When responding:
- Be concise and direct
- Reference relevant context from memory when available
```

Place prompt files in `prompts/` at the product root (not inside `config/`).

## workflows/*.yaml

Each file in `config/workflows/` defines one workflow. See the [creating a product](creating-a-product.md) guide for the full schema.

```yaml
workflow: chat
description: "Search memory for context, then respond with LLM"
capabilities: ["memory:search", "memory:store", "ai:complete"]

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

| Node field | Required | Description |
|------------|----------|-------------|
| `do` | yes | Tool to invoke |
| `with` | no | Arguments — supports static values and `{{expr}}` templates |
| `then` | no | Next node(s) — string or `[a, b]` for parallel fan-out |
| `return` | no | `true` marks this node's output as the workflow result |

The first node in the YAML is the entry node. All other nodes must be reachable via `then:` edges. Parallel branches are resolved concurrently by the engine.
