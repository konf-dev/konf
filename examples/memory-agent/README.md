# Memory Agent Example

Store and search memories in a persistent knowledge graph (Postgres + pgvector).

## Prerequisites

```bash
# Start Postgres with pgvector
docker run -d --name konf-pg -p 5432:5432 \
  -e POSTGRES_PASSWORD=konf -e POSTGRES_DB=konf \
  pgvector/pgvector:pg17

# Start ollama (for LLM-powered recall)
ollama serve && ollama pull qwen3:8b
```

## Run

```bash
export OPENAI_API_KEY=ollama
export OPENAI_BASE_URL=http://localhost:11434/v1

KONF_CONFIG_DIR=examples/memory-agent/config KONF_DEV_MODE=true cargo run --bin konf-backend
```

## Test

```bash
# Store a memory (uses the 'remember' workflow since it's first alphabetically)
curl -N -X POST http://localhost:8000/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "I learned that Konf uses a capability lattice for security"}'

# Search memories (switch to recall workflow — needs endpoint for workflow selection)
# For now, memories can be searched via the memory:search tool in the LLM chat workflow
```

## What it proves

- Persistent memory across server restarts (Postgres-backed)
- smrti memory backend registered from tools.yaml
- memory:store and memory:search tools work
- Namespace isolation (each user gets their own memory space)
- Event journal records all operations
