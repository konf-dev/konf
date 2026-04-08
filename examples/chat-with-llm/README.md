# Chat with LLM Example

Chat with a local LLM running on ollama. Fully local — nothing leaves your machine.

## Prerequisites

```bash
ollama serve
ollama pull qwen3:8b
```

## Run

```bash
# Point rig's OpenAI client at ollama
export OPENAI_API_KEY=ollama
export OPENAI_BASE_URL=http://localhost:11434/v1

KONF_CONFIG_DIR=examples/chat-with-llm/config KONF_DEV_MODE=true cargo run --bin konf-backend
```

## Test

```bash
curl -N -X POST http://localhost:8000/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "What is Rust? Answer in one sentence."}'
```

## Using Anthropic or OpenAI instead

```yaml
# In tools.yaml, change provider and model:
tools:
  llm:
    provider: anthropic
    model: claude-sonnet-4-20250514
```

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

## What it proves

- LLM tool registers from tools.yaml
- ai_complete calls ollama via OpenAI-compatible API (rig-core)
- SSE streams response with tool_start, text_delta, tool_end, done events
- Works with ollama (local), Anthropic, or OpenAI
