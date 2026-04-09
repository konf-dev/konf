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
      - "memory_*"
      - "ai_complete"
      - "http_get"
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
- `roles.*` — role → capability mapping for auth scoping. See [runtime.md](../architecture/runtime.md#auth-scoping).

### Tool Guards

Define deny/allow rules that are evaluated before every tool invocation:

```yaml
tool_guards:
  shell_exec:
    rules:
      - action: deny
        predicate:
          type: contains
          path: "command"
          value: "sudo"
        message: "sudo is not allowed"
    default: allow  # explicit — default is deny (fail-closed)
```

Guards are hot-reloadable via `config_reload`. See [runtime.md](../architecture/runtime.md#tool-guards) for the full reference.

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
capabilities: ["memory_search", "memory_store", "ai_complete"]

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

| Node field | Required | Description |
|------------|----------|-------------|
| `do` | yes | Tool to invoke |
| `with` | no | Arguments — supports static values and `{{expr}}` templates |
| `then` | no | Next node(s) — string or `[a, b]` for parallel fan-out |
| `return` | no | `true` marks this node's output as the workflow result |

The first node in the YAML is the entry node. All other nodes must be reachable via `then:` edges. Parallel branches are resolved concurrently by the engine.
