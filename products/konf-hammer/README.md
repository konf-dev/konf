# konf-hammer

Authors workflow YAML (and, later, whole products) for *other* konf products.

Named after Valve's Hammer Editor — which authors maps for the Source engine.
`konf-hammer` authors workflows for konf products.

## Scope

- **Target repo:** any konf product's config directory.
- **Shell target:** the konf working tree at
  `/home/bert/Work/orgs/konf-dev-stack/konf/`.
- **v0 mutations:** only writes into a target product's
  `config/workflows/auto_<slug>.yaml`. Never overwrites.

## Why it exists

- `repo-manager`'s `self_improve` needs an LLM but repo-manager should stay
  usable without cloud keys. Externalize authoring so repo-manager remains
  Anthropic-key-free.
- Prove that konf is **provider-configurable**: the same `draft_workflow.yaml`
  runs against Ollama (default) or Gemini by swapping `tools.yaml`. No code
  change.
- Route the friction-free tier (Ollama) at routine drafts; escalate to Gemini
  only when quality matters.

## v0 workflows

| Tool (MCP name) | What it does |
|---|---|
| `workflow:echo` | Round-trip smoke test — send a prompt to the configured LLM, return its text. |

Slice 3+ adds `draft_workflow`, `validate_and_iterate`, `commit_and_reload`,
and eventually `draft_product` (full product scaffolding).

## Quick start (host mode)

### 1. Install `konf-mcp`

Already installed from the parent repo. If you need to reinstall:

```bash
cd /home/bert/Work/orgs/konf-dev-stack/konf
cargo install --path crates/konf-mcp
```

### 2. Pull the default model (Ollama)

```bash
ollama pull qwen3-coder:30b   # ~18 GB, fits RTX 3090 Ti alone
```

Other useful models (optional):

```bash
ollama pull devstral:24b             # alternative code model
ollama pull deepseek-r1:32b          # reasoning-tuned
ollama pull nomic-embed-text         # embeddings (for future memory_search)
```

### 3. Set up env with direnv (optional for Ollama-only use)

```bash
cd products/konf-hammer
cp .envrc.example .envrc
$EDITOR .envrc                      # only needed if escalating to Gemini
direnv allow
```

With the default Ollama config, no API key is required. Gemini key is only
needed if you swap in `tools.gemini.yaml`.

### 4. Wire MCP in `~/.claude.json`

Add under `projects["/home/bert/Work/orgs/konf-dev-stack"].mcpServers`:

```jsonc
"konf-hammer": {
  "command": "konf-mcp",
  "args": [
    "--config",
    "/home/bert/Work/orgs/konf-dev-stack/konf/products/konf-hammer/config"
  ],
  "env": {
    "KONF_MODEL": "qwen3-coder:30b",
    "RUST_LOG": "info,konf=debug"
  }
}
```

Restart or reconnect MCP:

```
/mcp reconnect
```

You should see `konf-hammer` appear with one tool: `echo`.

## Invocation

```
# In Claude Code, via MCP tool calls:

workflow:echo { "message": "Say hi in 5 words." }
```

## Swapping providers (configurability proof)

When Slice 6 lands, a `tools.gemini.yaml` variant ships alongside
`tools.yaml`. To switch the live provider:

```bash
cd products/konf-hammer/config
cp tools.yaml tools.ollama.yaml.bak   # if not already present
cp tools.gemini.yaml tools.yaml
# in Claude Code:
/mcp reconnect
```

No code change. Same workflows. Different backend.

## Design notes

- **Tool guards.** `tools.yaml` enforces shell safety at the substrate level.
  Same denylist as `repo-manager`: `sudo`, `rm -rf /`, `git config`, pushes
  to `main`/`master`.
- **Narrow agent role.** The `agent` role in `project.yaml` lists exactly
  which tool patterns drafted workflows can call. No wildcards.
- **Memory, private per product.** `memory.db/` lives under this product's
  dir and is gitignored. Authoring runs can store past drafts and retrieval
  patterns for iterative refinement.

## Deferred to later slices

- Slice 3: `draft_workflow` — NL description → YAML text (no writes).
- Slice 4: `validate_and_iterate` — YAML → validate → repair loop.
- Slice 5: `commit_and_reload` — write to target, git commit, hot-reload.
- Slice 6: `tools.gemini.yaml` alternate config + swap docs.
- Beyond: whole-product drafting (project.yaml + tools.yaml + prompts).

## Rollback

Every drafted workflow lands as its own commit in the target product's repo.

```bash
cd <target-product-repo>
git log --oneline -5 <target>/config/workflows/
git revert <sha>
# /mcp reconnect in Claude Code — picks up reverted target config
```
