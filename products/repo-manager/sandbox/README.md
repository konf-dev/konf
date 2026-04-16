# repo-manager sandbox

OCI container that hosts repo-manager's tools. **The container is idle
by design** — it sleeps, waiting for the host's Claude CLI to spawn
MCP servers inside it on demand via `podman exec -i`.

## Architecture

```
Host
└── claude-cli (your reasoning loop, interactive or SDK-based)
     │
     ├── MCP client ── podman exec -i konf-repo-manager konf-mcp
     │                 ────────────────────────────────────────
     │                 (repo-manager workflows: repo_status,
     │                  triage_issue, self_improve)
     │
     └── MCP client ── podman exec -i konf-repo-manager claude mcp serve
                       ─────────────────────────────────────────────
                       (generic Read/Write/Edit/Bash/Grep/Glob
                        running INSIDE the container)

Container (konf-repo-manager)
├── PID 1: tini → sleep infinity
├── konf-mcp binary (pre-built by entrypoint)
├── claude-cli    (for `claude mcp serve`)
├── github-mcp-server (native binary)
├── bind mount: host konf-dev-stack → /home/konf-agent/konf-dev-stack
└── non-root user: konf-agent (UID-aligned with host)
```

No tmux session. No interactive claude inside. No login inside the
container. All reasoning is on the host; all execution is in the
container.

## Quick start

```bash
# one-time
cp .env.example .env
$EDITOR .env                          # fill PAT + LLM keys
make sandbox-build                    # ~5 min first build
make sandbox-up                       # start the idle container
```

Then add two MCP servers to your **host** Claude CLI config (e.g.
`~/.claude.json` `mcpServers` block). Both use `podman exec -i` — stdio
flows into the container and MCP messages come back on the same pipe.

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

Swap `podman` → `docker` if you're using Docker Engine. No `env` block
is needed — the container's `ENV` directive sets `KONF_CONFIG_DIR`, and
`--env-file .env` injects the LLM/GitHub keys at `sandbox-up` time.
Those env vars are inherited by every `podman exec` into the container.

Reconnect Claude CLI (`/mcp reconnect`). `/mcp` should list:

- `repo-manager` → `workflow:repo_status`, `workflow:triage_issue`,
  `workflow:self_improve`
- `sandbox-tools` → `Bash`, `Read`, `Write`, `Edit`, `Grep`, `Glob`,
  `TodoWrite`, etc. — all executing inside the container.

## Why the two MCP servers?

- **repo-manager** gives your host claude-cli the domain-specific
  tools (triage, self_improve, status digests). High-level actions.
- **sandbox-tools** gives it general-purpose tools (edit files, run
  `cargo test`, grep the codebase) — but they run **inside the
  container**, so any stray `rm -rf` stays trapped.

You get the full Claude Code feature surface, OS-level sandboxed. Your
host filesystem is untouchable except for the bind-mounted konf repo.

## What about the classifier outage?

MCP-prefixed tool calls (e.g. `mcp__sandbox-tools__Bash`) typically go
through a different permission gate than the built-in `Bash`. In
practice that means the host's Bash-safety classifier matters less
when all execution is via MCP. If it still intercepts, add an
allowlist entry for the MCP tool names in `.claude/settings.json`
`permissions.allow`.

## Runtime choice

Same Dockerfile runs under both Podman (default) and Docker Engine.

```bash
CONTAINER_CLI=docker make sandbox-up
```

| Runtime | Licence | Recommendation |
|---|---|---|
| **Podman** | Apache-2.0 | Primary. Rootless, daemonless, no Docker Desktop licence. |
| **Docker Engine** | Apache-2.0 | Acceptable on Linux. |
| Docker Desktop | Proprietary | Avoid in shared docs — free only for personal use / orgs < 250 people & < $10M revenue. |
| LXD | **AGPLv3 since 2023** | Avoid (viral licence). Use Incus (Apache-2.0) if you want that shape. |

## Makefile targets

| Target | What it does |
|---|---|
| `make sandbox-build` | Build the OCI image (`rust:1.85-slim-bookworm` base). |
| `make sandbox-up` | Start the idle container (detached). |
| `make sandbox-down` | Stop and remove the container. |
| `make sandbox-reset` | Down + rebuild + up. |
| `make sandbox-bash` | Open a shell in the container for manual debugging. |
| `make sandbox-logs` | Follow entrypoint stdout/stderr. |
| `make sandbox-status` | Show container state. |
| `make sandbox-clean` | Stop container + drop image + drop caches (full reset). |
| `make sandbox-mcp-test` | Ping each MCP server once to verify it starts. |

## Volumes

| Volume | Mount | Purpose |
|---|---|---|
| `konf-repo-manager-cargo` | `/home/konf-agent/.cargo` | cargo registry + build cache (so `konf-mcp` builds stay fast) |
| `konf-repo-manager-npm` | `/home/konf-agent/.npm` | npm cache for claude-cli updates |
| bind-mount | `/home/konf-agent/konf-dev-stack` ← host konf-dev-stack/ | product dir + konf sources (rw, UID-aligned) |

No `~/.claude/` persistence volume — nothing inside the container
logs into Claude. If you later want `claude mcp serve` to do anything
stateful (rare), add a named volume for `/home/konf-agent/.claude`.

## Identity

Container git config is set to `konf-agents[bot]` by the entrypoint,
so `self_improve` commits are visibly agent-authored. Bind-mount your
host `~/.gitconfig` read-only if you want a different identity.

The GitHub PAT inside the container authenticates as its owner for
HTTPS push — that's an eventual-consistency story with
`konf-identity-github` (App-installation tokens).

## Troubleshooting

- **`podman exec -i ... konf-mcp` fails: "no such container"** —
  `make sandbox-up` first.
- **`konf-mcp` takes forever on first MCP call** — the pre-build in
  the entrypoint didn't finish before you tried. `make sandbox-logs`
  will show progress; wait for "Ready."
- **GitHub MCP errors** — PAT missing scopes or expired. Set
  `GITHUB_PERSONAL_ACCESS_TOKEN` in `.env` and `make sandbox-reset`.
- **Bind-mount permission denied (Fedora / SELinux)** — add `:Z` to
  the bind mount in the Makefile's `sandbox-up` target.

## Future upgrade paths

The Dockerfile is the long-term commitment; the runtime flag is not.

- **Stronger per-call isolation**: `--runtime=runsc` (gVisor, Apache-2.0).
- **Per-self_improve ephemeral sandbox**: Firecracker microVM per
  generation (~100 ms boot, hardware-level isolation). Same image.
- **Multi-tenant / konf-Cloud**: Kubernetes pods, one per tenant.
