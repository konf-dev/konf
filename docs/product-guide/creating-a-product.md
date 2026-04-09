# Creating a Product

> Scope: step-by-step guide for product builders.

## Prerequisites

- Konf binary built or running via Docker
- A Postgres instance with pgvector (for memory tools)

## 1. Copy the Template

```bash
cp -r products/_template products/my-product
```

You get:

```
products/my-product/
├── config/
│   ├── tools.yaml
│   └── workflows/
│       └── hello.yaml
└── README.md
```

## 2. Add Prompts Directory

```bash
mkdir -p products/my-product/prompts
```

## 3. Configure Tools (`config/tools.yaml`)

Enable the tools your agent needs:

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
```

Each key under `tools` enables a tool namespace. Remove any you do not need.

To add external MCP servers:

```yaml
tools:
  llm:
    provider: openai
    model: gpt-4o

mcp_servers:
  github:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_TOKEN: "${GITHUB_TOKEN}"
```

## 4. Configure the Model (`config/models.yaml`)

```yaml
default:
  provider: openai
  model: "qwen3:8b"
  temperature: 0.7
  max_tokens: 4096
```

## 5. Define the Project (`config/project.yaml`)

```yaml
name: my-product
description: "My custom AI agent"
version: "0.1.0"

triggers:
  chat:
    workflow: chat
    capabilities:
      - "memory:*"
      - "ai:complete"
      - "http:get"
```

Triggers map an entry point name to a workflow and the maximum capabilities that workflow receives.

## 6. Write a Workflow (`config/workflows/chat.yaml`)

The search-then-respond pattern:

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

- `do:` — the tool to invoke.
- `with:` — arguments passed to the tool. `{{input.expr}}` interpolates from workflow input; `{{node.field}}` references prior node output.
- `then:` — next node(s). Omit for terminal nodes, or use `return: true`.
- Nodes without dependencies run in parallel.

## 7. Write a System Prompt (`prompts/system.md`)

```markdown
You are a helpful assistant with access to persistent memory.

When responding:
- Be concise and direct
- Reference relevant context from memory when available
- Ask clarifying questions when the request is ambiguous
```

## 8. Run It

From source:

```bash
KONF_CONFIG_DIR=products/my-product/config cargo run --bin konf-backend
```

Or with Docker Compose (edit `docker-compose.yml` to mount your product):

```yaml
volumes:
  - ./products/my-product/config:/config:ro
```

Then:

```bash
docker compose up
```

## 9. Test

Health check:

```bash
curl http://localhost:8000/v1/health
```

Send a message:

```bash
curl -X POST http://localhost:8000/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello, what can you help me with?"}'
```

Stream a response:

```bash
curl -N -X POST http://localhost:8000/v1/chat/stream \
  -H "Content-Type: application/json" \
  -d '{"message": "Tell me about yourself"}'
```

## Final Structure

```
products/my-product/
├── config/
│   ├── models.yaml
│   ├── project.yaml
│   ├── tools.yaml
│   └── workflows/
│       └── chat.yaml
├── prompts/
│   └── system.md
└── README.md
```
