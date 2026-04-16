# repo-manager

The single owner of the `konf/` codebase. You talk to it from Claude CLI over
MCP; it inspects, triages, and extends itself via `self_improve`.

## Scope

- **Target repo:** `konf-dev/konf` only.
- **Shell target:** the konf working tree at
  `/home/bert/Work/orgs/konf-dev-stack/konf/`.
- **v0 mutations:** only `self_improve` writes to disk (into its own product
  dir). All GitHub-facing workflows are read-only.

## v0 workflows

| Tool (MCP name) | What it does |
|---|---|
| `workflow:repo_status` | Read-only snapshot — branch, dirty files, compile status, clippy warnings, open PRs and issues. |
| `workflow:triage_issue` | Reads an issue + comments, returns classification + suggested label + suggested markdown reply. No GitHub writes. |
| `workflow:self_improve` | Given plain-English intent + a name hint, drafts a new `auto_<hint>.yaml` workflow, validates it, commits it, hot-reloads. Caller smoke-tests manually; rollback is a single `git revert`. |

## Quick start (host mode)

v0 runs directly on the host — no container. The sandboxed harness is
archived at `konf-genesis/scratchpad/sandbox-v01-deferred/` and comes back in
v0.1 when isolation is actually needed (generated code, background agents,
webhooks). See [DEFERRED.md](../../../konf-genesis/scratchpad/sandbox-v01-deferred/DEFERRED.md)
for why and when.

### 1. Install `konf-mcp`

```bash
cd /home/bert/Work/orgs/konf-dev-stack/konf
cargo install --path crates/konf-mcp
command -v konf-mcp   # should resolve to ~/.cargo/bin/konf-mcp
```

### 2. Install `github-mcp-server`

```bash
mkdir -p ~/.local/bin
curl -fsSL -o /tmp/gh-mcp.tgz \
  https://github.com/github/github-mcp-server/releases/download/v1.0.0/github-mcp-server_Linux_x86_64.tar.gz
tar -xzf /tmp/gh-mcp.tgz -C ~/.local/bin github-mcp-server
chmod +x ~/.local/bin/github-mcp-server
rm /tmp/gh-mcp.tgz
github-mcp-server --version   # expect "Version: 1.0.0"
```

(`~/.local/bin` should already be on your `PATH`; otherwise add it to your
shell rc.)

### 3. Set up secrets with direnv

```bash
# Install direnv (one-time)
sudo pacman -S --noconfirm direnv
# Hook it into your shell (add to ~/.bashrc or ~/.zshrc if missing):
#   eval "$(direnv hook bash)"     # or zsh

cd products/repo-manager
cp .envrc.example .envrc
$EDITOR .envrc                      # PAT + LLM keys
direnv allow
```

Required env vars:

- `GITHUB_PERSONAL_ACCESS_TOKEN` — classic PAT with `repo` + `read:org`, or
  fine-grained scoped to `konf-dev/konf` (Contents/Issues/PRs = read-only for
  v0; writes come later).
- `GEMINI_API_KEY` — default LLM provider.
- `ANTHROPIC_API_KEY` — required only for `self_improve.draft` (Claude Sonnet
  4.6). Workflows `repo_status` and `triage_issue` work without it.
- `KONF_CONFIG_DIR` — absolute path to this product's `config/`. The shipped
  `.envrc` sets it to `"$(pwd)/config"`.
- `KONF_MODEL` — default provider identifier (`gemini-2.5-pro`).

`.envrc` is gitignored — never commit it.

### 4. Wire MCP in `~/.claude.json`

Add under `projects["/home/bert/Work/orgs/konf-dev-stack"].mcpServers`:

```jsonc
"repo-manager": {
  "command": "konf-mcp",
  "args": ["--config", "/home/bert/Work/orgs/konf-dev-stack/konf/products/repo-manager/config"],
  "cwd": "/home/bert/Work/orgs/konf-dev-stack/konf/products/repo-manager"
}
```

- `--config` is required. `konf-mcp` doesn't read `KONF_CONFIG_DIR` from env;
  without the flag it falls back to `./config` relative to cwd.
- `cwd` pins the runtime `memory.db` (rocksdb KV store) to the product dir
  — `tools.yaml` sets `memory.config.path: ./memory.db`, which is resolved
  against cwd. Already `.gitignore`d here.

The `GITHUB_PERSONAL_ACCESS_TOKEN` / `GEMINI_API_KEY` / `ANTHROPIC_API_KEY`
variables flow in from direnv, so long as you launched Claude Code from a
shell where direnv had already loaded them (i.e., from `products/repo-manager/`
or any ancestor directory after running `direnv allow`).

If that's inconvenient, bake the three keys directly into the `env:` block
above. Trade-off: secrets end up in `~/.claude.json`, which is harder to
rotate and gets caught up in editor/sync backups.

Restart or reconnect MCP:

```
/mcp
```

You should see `repo-manager` with three tools: `repo_status`, `triage_issue`,
`self_improve`.

## Invocation

```
# In Claude Code, via tool calls or slash commands:

workflow:repo_status
workflow:triage_issue  { "issue_number": 42 }
workflow:self_improve  {
  "intent":       "Report the current konf git branch and last commit SHA.",
  "name_hint":    "report_branch",
  "sample_input": {}
}
```

`self_improve` writes
`konf/products/repo-manager/config/workflows/auto_report_branch.yaml` and
creates a single git commit on the host tree. After `/mcp reconnect`, the new
tool `workflow:auto_report_branch` is live.

## Rollback

Every `self_improve` generation is one commit on the host tree.

```bash
cd /home/bert/Work/orgs/konf-dev-stack/konf
git log --oneline -5 products/repo-manager/config/workflows/
git revert <sha>
# /mcp reconnect   in Claude Code — respawns konf-mcp, picks up reverted tree
```

v0 does not auto-smoke-test generated workflows. If the first call on a fresh
generation misbehaves, revert and regenerate with a sharper intent.

Worst case (konf-mcp won't start because of a bad generated workflow):

```bash
rm products/repo-manager/config/workflows/auto_*.yaml
# or: git reset --hard <last-known-good-sha>
```

Hand-authored workflows (`repo_status`, `triage_issue`, `self_improve`) never
live under `auto_*`, so these commands leave them alone.

## Design notes

- **Identity seam.** `project.yaml` declares `identity: github:pat`. This is a
  semantic label — today a PAT is read from env. When `konf-identity-github`
  ships, it mints installation tokens from the `konf-agents` App and
  populates the same env var. Flipping the label to `github:app:konf-agents`
  becomes cosmetic — this product does not change.

- **Capability surface.** The `agent` role in `tools.yaml` enumerates every
  tool pattern available inside workflows. No wildcards. The `owner` role
  (MCP session) has `"*"` because you can do anything interactively.

- **Tool guards.** `tools.yaml` enforces shell safety at the substrate level —
  `sudo`, `rm -rf`, `git config --global`, and pushes to `main` are blocked
  regardless of what prompts say.

- **Provider strategy.** Gemini 2.5 Pro by default (cheap, 2 M context,
  adequate for triage). Claude Sonnet 4.6 for `self_improve.draft` only —
  persisted executable config is the one place where stronger
  instruction-following pays off.

## Deferred to v0.1+

- Sandbox / container isolation (see `sandbox-v01-deferred/DEFERRED.md`)
- Webhook receiver + GitHub-event triggers
- Mutating GitHub workflows (`repo_comment`, `apply_label`, `create_pr`)
- Scheduled `enforce_rules`
- Budget cells / per-workflow cost tuning
- `konf-identity-github` plugin (installation-token side of the identity seam)
- `fix_and_pr` code-editing agent
- Pattern extraction to a generic repo-agent template
- Auto smoke-test + auto-rollback inside `self_improve`

See `konf-genesis/scratchpad/STAGE_11_HANDOFF.md` for the full roadmap.
