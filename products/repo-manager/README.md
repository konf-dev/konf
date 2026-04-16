# repo-manager

The single owner of the `konf/` codebase. You talk to it from Claude CLI
over MCP; it inspects, triages, and extends itself via `self_improve`.

## Scope

- **Target repo:** `konf-dev/konf` only.
- **Shell target:** the konf working tree at
  `/home/bert/Work/orgs/konf-dev-stack/konf/`.
- **v0 mutations:** only `self_improve` writes to disk (into its own
  product dir). All GitHub-facing workflows are read-only.

---

## Recommended: sandboxed MCP serve

The intended deployment for v0 is **host claude-cli driving MCP
servers that run inside an OCI container**. No interactive claude-cli
inside the container, no tmux session, no login inside. Just an idle
container that the host spawns MCP servers in on-demand via
`podman exec -i`.

Full setup + architecture diagram: [sandbox/README.md](sandbox/README.md).

```
Host claude-cli
   ├── MCP → podman exec -i konf-repo-manager konf-mcp           (domain workflows)
   └── MCP → podman exec -i konf-repo-manager claude mcp serve   (generic tools, run in container)
```

All execution happens inside the container — host filesystem is
untouchable except for the bind-mounted konf repo. `self_improve`
commits land in your host git tree via the bind mount.

### Quick start

```bash
cd sandbox
cp .env.example .env
$EDITOR .env                        # PAT + LLM keys
make sandbox-build                  # ~5 min first time
make sandbox-up                     # start idle container
make sandbox-mcp-test               # verify MCP binaries respond
```

Add this to your host `~/.claude.json` (or the equivalent MCP config
location for your Claude CLI setup):

```jsonc
{
  "mcpServers": {
    "repo-manager": {
      "command": "podman",
      "args": ["exec", "-i", "konf-repo-manager", "konf-mcp"]
    },
    "sandbox-tools": {
      "command": "podman",
      "args": ["exec", "-i", "konf-repo-manager", "claude", "mcp", "serve"]
    }
  }
}
```

No `env` block needed — `KONF_CONFIG_DIR` is baked into the container's
ENV directive and the `.env` file (PAT + LLM keys) is injected at
`sandbox-up` time via `--env-file`.

Reconnect Claude CLI → `/mcp` shows both servers.

### Runtime choice

Same Dockerfile runs under Podman and Docker Engine.
`CONTAINER_CLI=docker make …` to switch.

| Runtime | License | When to pick |
|---|---|---|
| **Podman** | Apache-2.0 | **Default.** Rootless, daemonless, no Docker Desktop licence. |
| **Docker Engine** | Apache-2.0 | Acceptable on Linux (free). |
| Docker Desktop | Proprietary | **Avoid**: free only for personal use / orgs < 250 people & < $10M revenue. |
| LXD | AGPLv3 | **Avoid** (viral licence). Use Incus (Apache-2.0 fork). |

---

## v0 workflows

| Tool (MCP name) | What it does |
|---|---|
| `workflow:repo_status` | Read-only snapshot — branch, dirty files, compile status, clippy warnings, open PRs and issues. |
| `workflow:triage_issue` | Reads an issue + comments, returns classification + suggested label + suggested markdown reply. No GitHub writes. |
| `workflow:self_improve` | Given plain-English intent + a name hint, drafts a new `auto_<hint>.yaml` workflow, validates it, commits it, hot-reloads. Caller smoke-tests manually; rollback is a single `git revert`. |

---

## Prerequisites (sandboxed path — the documented one)

1. **Podman** (or Docker Engine) installed on the host.
2. **API keys / PAT** in `sandbox/.env` (copy from `sandbox/.env.example`):
   ```env
   GITHUB_PERSONAL_ACCESS_TOKEN=ghp_xxx
   GEMINI_API_KEY=...
   ANTHROPIC_API_KEY=...    # only needed for self_improve.draft
   ```
3. **konf repo bind-mount** is wired up by the Makefile automatically
   (`sandbox/../../../..` → `/home/konf-agent/konf-dev-stack` inside the
   container).
4. **Clean tree** before calling `self_improve` — it refuses to run
   on top of uncommitted changes, by design.

The bare-host path (host-installed `konf-mcp` + `github-mcp-server`
on PATH, no container) works but isn't documented here. If you need
it, point your MCP config at the local binary path instead of
`podman exec -i …` and install `github-mcp-server` from
[github.com/github/github-mcp-server/releases](https://github.com/github/github-mcp-server/releases).

---

## Invocation examples

All three tools are plain MCP calls. Claude CLI picks them up
automatically; you can also call them explicitly.

### Status snapshot

```
/mcp call repo-manager workflow:repo_status
```

Returns structured git state, build/clippy status, open PR/issue lists,
and a markdown digest.

### Triage an issue

```
/mcp call repo-manager workflow:triage_issue '{"issue_number": 42}'
```

Returns a JSON object with classification, urgency, suggested label,
suggested comment, and reasoning. **Does not post anything.**

### Self-improve — add a new workflow

```
/mcp call repo-manager workflow:self_improve '{
  "intent": "Report the current konf git branch and the SHA of the last commit.",
  "name_hint": "report_branch",
  "sample_input": {}
}'
```

On success: new file at
`konf/products/repo-manager/config/workflows/auto_report_branch.yaml`,
committed (inside the container, persists via bind mount), hot-reloaded.
Reconnect MCP to pick up the new tool:

```
/mcp
```

`workflow:auto_report_branch` is now live.

---

## Rollback cheat-sheet

Every `self_improve` generation is one git commit (on the host's git
tree — the bind mount writes straight through). To undo:

```bash
cd /home/bert/Work/orgs/konf-dev-stack/konf
git log --oneline -5 products/repo-manager/config/workflows/   # find the sha
git revert <sha>
# Then force the host's Claude CLI to respawn konf-mcp:
#   /mcp reconnect   (new 'podman exec -i ... konf-mcp' process picks up the reverted tree)
```

v0 does not auto-smoke-test generated workflows (deferred — template
expansion inside `do:` is not proven for dynamic `workflow:auto_<slug>`
dispatch). If a generated workflow misbehaves on your first call,
revert manually with `git revert` + `/mcp reconnect`.

Worst case (konf-mcp won't start because of a bad generated workflow):

```bash
cd /home/bert/Work/orgs/konf-dev-stack/konf
rm products/repo-manager/config/workflows/auto_*.yaml
# or
git reset --hard <last-known-good-sha>
```

The hand-authored workflows (`repo_status`, `triage_issue`,
`self_improve`) never live under `auto_*`, so they're safe to keep.

---

## Design notes

- **Identity seam**: `project.yaml` declares `identity: github:pat`.
  This is a semantic label only — today, a PAT is read from the env
  var. When `konf-identity-github` ships, it will mint installation
  tokens from the `konf-agents` App and populate the same env var
  before launch. Flipping the label to `github:app:konf-agents` is
  cosmetic — this product does not change.

- **Capability surface**: the `agent` role explicitly enumerates the
  tool patterns available inside workflows. No wildcards. `owner`
  (the MCP session) has `"*"` because you can do anything
  interactively.

- **Tool guards** in `tools.yaml` enforce shell safety at the kernel
  level — prompts cannot override them.

- **Provider strategy**: Gemini 2.5 Pro by default (cheap, 2M
  context, adequate for triage). Claude Sonnet 4.6 for
  `self_improve.draft` only — persisted executable configuration is
  the one place where stronger instruction-following pays off.

---

## What's deferred

(See also `konf-genesis/scratchpad/STAGE_11_HANDOFF.md` for the full
roadmap.)

- Webhook receiver + GitHub-event triggers
- Mutating GitHub workflows (`repo_comment`, `apply_label`, `create_pr`)
- Scheduled `enforce_rules`
- Budget cells / per-workflow cost tuning
- `konf-identity-github` plugin (the App-token minting side of the
  identity seam)
- `fix_and_pr` code-editing agent
- Pattern extraction to a generic repo-agent template
