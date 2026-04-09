# Core Concepts

> Scope: mental model for anyone new to Konf.

## Products

A **product** is a directory of YAML configuration that defines an AI agent. No code required — just config files:

```
products/my-assistant/
├── config/
│   ├── project.yaml       # Name, triggers, capabilities
│   ├── tools.yaml         # Which tools are available
│   ├── models.yaml        # LLM provider settings
│   └── workflows/
│       └── chat.yaml      # DAG of tool calls
├── prompts/
│   └── system.md          # System prompt (markdown)
└── README.md
```

Different products share the same Konf runtime. Shipping a new agent means shipping a new directory.

## Workflows

A **workflow** is a directed acyclic graph (DAG) of tool calls, defined in YAML:

```yaml
workflow: chat
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

Nodes execute in dependency order. Parallel branches run concurrently. The engine enforces step limits, timeouts, and retry policies.

## Tools

A **tool** is a single action the agent can take. Tools are namespaced:

| Namespace | Examples                         |
|-----------|----------------------------------|
| `memory`  | `memory_search`, `memory_store`  |
| `ai`      | `ai_complete`                    |
| `http`    | `http_get`, `http_post`          |
| `embed`   | `embed_text`                     |
| `mcp`     | `mcp:*` (external MCP servers)   |
| builtin   | `echo`                           |

Tools are enabled per-product in `tools.yaml`. A tool that is not listed is not available.

## Namespaces

Namespaces provide **hierarchical isolation**. Every resource (memory entry, workflow run, tool invocation) belongs to a namespace:

```
konf                          # platform root
├── assistant                 # product
│   ├── user_123              # end user
│   └── user_456
└── support-bot
    └── org_789
```

A scope can only access resources at or below its own level. Cross-namespace access is audited and requires explicit grants.

## Capabilities

Capabilities are **permissions that attenuate down the hierarchy**. A parent can only grant a subset of its own capabilities to children — never more.

```yaml
# project.yaml — product admin grants these to the chat trigger
triggers:
  chat:
    workflow: chat
    capabilities:
      - "memory_*"       # all memory operations
      - "ai_complete"    # LLM calls
      - "http_get"       # read-only HTTP
```

The end user inherits at most these capabilities. The LLM never sees namespace or capability metadata — it only sees the tool interface.

## Four-Layer Model

Konf separates concerns into four layers, each with decreasing privilege:

| Layer          | Controls                              | Example actor        |
|----------------|---------------------------------------|----------------------|
| **Infra**      | Database, secrets, binary deployment  | Platform operator    |
| **Admin**      | Platform config (`konf.toml`), MCP    | DevOps / admin       |
| **Product**    | Product YAML, prompts, workflows      | Product builder      |
| **End User**   | Chat input, personal memory           | Human or API client  |

Each layer can only configure what the layer above permits. This is the OS analogy: infra is the kernel, admin is root, product is an installed app, end user is a logged-in session.
