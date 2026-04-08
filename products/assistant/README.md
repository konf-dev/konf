# Personal Assistant

A reference product demonstrating a personal assistant with memory, LLM chat, and tool use.

## What It Does

- Remembers conversations via graph memory (smrti/Postgres)
- Searches memory for relevant context before responding
- Responds using an LLM (configurable provider and model)
- Streams responses via SSE

## Running

```bash
# With Docker (includes Postgres)
docker compose up -d

# From source (requires running Postgres with pgvector)
KONF_CONFIG_DIR=products/assistant/config cargo run --bin konf-backend

# Chat
curl -X POST http://localhost:8000/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello, remember that I like hiking"}'
```

## Configuration

| File | Purpose |
|------|---------|
| `config/tools.yaml` | Memory backend, LLM provider, enabled tools |
| `config/models.yaml` | Model selection and parameters |
| `config/project.yaml` | Product metadata, capabilities |
| `config/workflows/chat.yaml` | Main chat workflow with memory search |
| `prompts/system.md` | Assistant personality and instructions |

## Customization

This product is meant to be forked and adapted. Common changes:

- **Switch LLM provider:** Edit `config/models.yaml`
- **Change personality:** Edit `prompts/system.md`
- **Add tools:** Add MCP servers to `config/tools.yaml`
- **Add workflows:** Create new YAML files in `config/workflows/`
