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

## 10. (Optional) Add Tool Guards and Roles

For production systems, you can add security boundaries in `tools.yaml`.

**Tool Guards** deny specific tool invocations based on input parameters. For example, to prevent `shell:exec` from running `rm -rf` or `sudo`:

```yaml
# config/tools.yaml
tool_guards:
  shell:exec:
    rules:
      - action: deny
        predicate: { type: contains, path: "command", value: "sudo" }
        message: "sudo is not allowed"
      - action: deny
        predicate: { type: matches, path: "command", value: "rm -rf" }
        message: "recursive force delete is not allowed"
    default: allow
```

**Roles** define capability sets for different user types, which are then enforced by the runtime for authenticated requests.

```yaml
# config/tools.yaml
roles:
  admin:
    capabilities: ["*"] # Full access
  agent:
    capabilities: ["memory:*", "ai:complete", "http:get"]
  guest:
    capabilities: ["memory:search"]
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

## Example: Autonomous Agent

You can create autonomous agents that run on a schedule. This example uses the `nightwatch.yaml` workflow from the `devkit` product, which periodically runs a health check.

**1. The workflow (`nightwatch.yaml`):**

This workflow runs another workflow (`workflow:dev_status`) and logs the result to a file.

```yaml
workflow: nightwatch
description: "Health check agent. Runs dev_status and logs the result."
register_as_tool: true
capabilities: ["shell:exec", "workflow:dev_status"]
nodes:
  health_check:
    do: workflow:dev_status
    then: log_result
  log_result:
    do: shell:exec
    with:
      command: "echo \"[$(date -Iseconds)] OK\" >> /tmp/health.log"
    return: true
```

**2. The scheduler workflow (`schedule_nightwatch.yaml`):**

This workflow uses the `schedule:create` tool to run the `nightwatch` workflow every 60 seconds.

```yaml
workflow: schedule_nightwatch
description: "Schedule the nightwatch agent to run periodically"
capabilities: ["schedule:create"]
nodes:
  schedule:
    do: schedule:create
    with:
      workflow: "nightwatch"
      delay_seconds: 60
      repeat: true
    return: true
```

**3. Trigger it:**

To start the agent, invoke the `schedule_nightwatch` workflow once. It will then run indefinitely.

```bash
curl -X POST http://localhost:8000/v1/invoke/schedule_nightwatch
```

The agent is now running. You can monitor its health log at `/tmp/health.log`. To stop it, you would need a `cancel:schedule` workflow.

This pattern (a workflow that does work + a workflow that schedules it) is the foundation for all autonomous behavior in Konf.


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
