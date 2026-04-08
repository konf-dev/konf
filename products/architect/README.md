# Konf Architect

The first product built on Konf — an AI that builds other Konf products.

## Status: Experimental

The architect is not a traditional product with hardcoded workflows. It's a **capability set** exposed via MCP that any AI client (Claude Code, Gemini CLI, OpenCode, Cursor) can connect to and use.

## How It Works

```
Your AI client (Claude Code, etc.)
    │
    └── MCP ──► konf-mcp ──► All registered tools
                              ├── yaml:validate_workflow
                              ├── system:introspect
                              ├── config:reload
                              ├── shell:exec (sandboxed)
                              ├── memory:search (if configured)
                              ├── ai:complete
                              └── http:get/post
```

The AI client provides the reasoning loop. Konf provides the tools and enforces safety (capability lattice, namespace isolation, resource limits).

## Running

1. Start the sandbox container:
   ```bash
   docker compose -f sandbox/docker-compose.yml up -d
   ```

2. Initialize git in the workspace (for checkpointing):
   ```bash
   docker exec konf-sandbox bash -c "cd /workspace/config && git init && git add -A && git commit -m 'initial'"
   ```

3. Start Konf:
   ```bash
   KONF_CONFIG_DIR=products/architect/config cargo run --bin konf-mcp
   ```

4. Connect your AI client via MCP (e.g., add to Claude Code's MCP settings).

## What the AI Can Do

- Inspect available tools (`system:introspect`)
- Generate and validate workflow YAML (`yaml:validate_workflow`)
- Write files to the sandbox (`shell:exec`)
- Hot-reload config after changes (`config:reload`)
- Checkpoint and rollback via git (`shell:exec` with git commands)

## What the AI Cannot Do

- Modify Rust code or the engine binary
- Escalate permissions beyond what's granted
- Access the network (sandbox starts with `--network none`)
- Access files outside the workspace volume
